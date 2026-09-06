use std::collections::{BTreeMap, BTreeSet};

use agent_core::{
    GraphBranchId, GraphNodeId, GraphNodeKind, GraphRecoveryMode, GraphTransitionKey,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_GRAPH_NODES: usize = 64;
pub const MAX_GRAPH_EDGES: usize = 128;
const MAX_DECISION_BRANCHES: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Retrieve,
    Model,
    Action,
    Verify,
    Decision { branches: Vec<GraphBranchId> },
    Complete,
    Fail,
}

impl NodeKind {
    #[must_use]
    pub const fn durable_kind(&self) -> GraphNodeKind {
        match self {
            Self::Retrieve => GraphNodeKind::Retrieve,
            Self::Model => GraphNodeKind::Model,
            Self::Action => GraphNodeKind::Action,
            Self::Verify => GraphNodeKind::Verify,
            Self::Decision { .. } => GraphNodeKind::Decision,
            Self::Complete => GraphNodeKind::Complete,
            Self::Fail => GraphNodeKind::Fail,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeDefinition {
    id: GraphNodeId,
    kind: NodeKind,
    recovery: GraphRecoveryMode,
}

impl NodeDefinition {
    #[must_use]
    pub const fn new(id: GraphNodeId, kind: NodeKind, recovery: GraphRecoveryMode) -> Self {
        Self { id, kind, recovery }
    }

    #[must_use]
    pub const fn id(&self) -> &GraphNodeId {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> &NodeKind {
        &self.kind
    }

    #[must_use]
    pub const fn recovery(&self) -> GraphRecoveryMode {
        self.recovery
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    from: GraphNodeId,
    transition: GraphTransitionKey,
    to: GraphNodeId,
}

impl Edge {
    #[must_use]
    pub const fn new(from: GraphNodeId, transition: GraphTransitionKey, to: GraphNodeId) -> Self {
        Self {
            from,
            transition,
            to,
        }
    }

    #[must_use]
    pub const fn from(&self) -> &GraphNodeId {
        &self.from
    }

    #[must_use]
    pub const fn transition(&self) -> &GraphTransitionKey {
        &self.transition
    }

    #[must_use]
    pub const fn to(&self) -> &GraphNodeId {
        &self.to
    }
}

#[derive(Clone, Debug)]
pub struct GraphDefinition {
    start: GraphNodeId,
    nodes: BTreeMap<GraphNodeId, NodeDefinition>,
    edges: BTreeMap<(GraphNodeId, GraphTransitionKey), GraphNodeId>,
    digest: [u8; 32],
}

impl GraphDefinition {
    pub fn new(
        start: GraphNodeId,
        nodes: Vec<NodeDefinition>,
        edges: Vec<Edge>,
    ) -> Result<Self, GraphDefinitionError> {
        if nodes.is_empty() || nodes.len() > MAX_GRAPH_NODES || edges.len() > MAX_GRAPH_EDGES {
            return Err(GraphDefinitionError::SizeLimit);
        }
        let mut node_map = BTreeMap::new();
        for node in nodes {
            validate_node(&node)?;
            if node_map.insert(node.id.clone(), node).is_some() {
                return Err(GraphDefinitionError::DuplicateNode);
            }
        }
        if !node_map.contains_key(&start) {
            return Err(GraphDefinitionError::UnknownNode);
        }
        let mut edge_map = BTreeMap::new();
        for edge in edges {
            if !node_map.contains_key(&edge.from) || !node_map.contains_key(&edge.to) {
                return Err(GraphDefinitionError::UnknownNode);
            }
            if edge_map
                .insert((edge.from, edge.transition), edge.to)
                .is_some()
            {
                return Err(GraphDefinitionError::DuplicateTransition);
            }
        }
        validate_transitions(&node_map, &edge_map)?;
        validate_reachability_and_cycles(&start, &node_map, &edge_map)?;
        let digest = definition_digest(&start, &node_map, &edge_map);
        Ok(Self {
            start,
            nodes: node_map,
            edges: edge_map,
            digest,
        })
    }

    #[must_use]
    pub const fn start(&self) -> &GraphNodeId {
        &self.start
    }

    #[must_use]
    pub const fn digest(&self) -> [u8; 32] {
        self.digest
    }

    #[must_use]
    pub fn node(&self, id: &GraphNodeId) -> Option<&NodeDefinition> {
        self.nodes.get(id)
    }

    #[must_use]
    pub fn transition(&self, from: &GraphNodeId, key: &GraphTransitionKey) -> Option<&GraphNodeId> {
        self.edges.get(&(from.clone(), key.clone()))
    }
}

fn validate_node(node: &NodeDefinition) -> Result<(), GraphDefinitionError> {
    let recovery_valid = matches!(
        (node.kind.durable_kind(), node.recovery),
        (GraphNodeKind::Retrieve, GraphRecoveryMode::FreshRetrieval)
            | (
                GraphNodeKind::Decision,
                GraphRecoveryMode::DeterministicBoundary
            )
            | (
                GraphNodeKind::Model
                    | GraphNodeKind::Action
                    | GraphNodeKind::Verify
                    | GraphNodeKind::Complete
                    | GraphNodeKind::Fail,
                GraphRecoveryMode::Never
            )
    );
    if !recovery_valid {
        return Err(GraphDefinitionError::InvalidRecoveryMode);
    }
    if let NodeKind::Decision { branches } = &node.kind {
        let unique: BTreeSet<_> = branches.iter().collect();
        if branches.is_empty()
            || branches.len() > MAX_DECISION_BRANCHES
            || unique.len() != branches.len()
        {
            return Err(GraphDefinitionError::InvalidDecisionBranches);
        }
    }
    Ok(())
}

fn validate_transitions(
    nodes: &BTreeMap<GraphNodeId, NodeDefinition>,
    edges: &BTreeMap<(GraphNodeId, GraphTransitionKey), GraphNodeId>,
) -> Result<(), GraphDefinitionError> {
    for node in nodes.values() {
        let actual: BTreeSet<_> = edges
            .keys()
            .filter(|(from, _)| from == node.id())
            .map(|(_, transition)| transition.clone())
            .collect();
        let expected = match node.kind() {
            NodeKind::Retrieve | NodeKind::Model | NodeKind::Action => {
                BTreeSet::from([GraphTransitionKey::Succeeded])
            }
            NodeKind::Verify => BTreeSet::from([
                GraphTransitionKey::VerificationPassed,
                GraphTransitionKey::VerificationFailed,
            ]),
            NodeKind::Decision { branches } => branches
                .iter()
                .cloned()
                .map(GraphTransitionKey::Branch)
                .collect(),
            NodeKind::Complete | NodeKind::Fail => BTreeSet::new(),
        };
        if actual != expected {
            return Err(
                if matches!(node.kind(), NodeKind::Complete | NodeKind::Fail) && !actual.is_empty()
                {
                    GraphDefinitionError::TerminalOutgoingEdge
                } else {
                    GraphDefinitionError::InvalidTransition
                },
            );
        }
    }
    Ok(())
}

fn validate_reachability_and_cycles(
    start: &GraphNodeId,
    nodes: &BTreeMap<GraphNodeId, NodeDefinition>,
    edges: &BTreeMap<(GraphNodeId, GraphTransitionKey), GraphNodeId>,
) -> Result<(), GraphDefinitionError> {
    fn visit(
        node: &GraphNodeId,
        edges: &BTreeMap<(GraphNodeId, GraphTransitionKey), GraphNodeId>,
        visiting: &mut BTreeSet<GraphNodeId>,
        visited: &mut BTreeSet<GraphNodeId>,
    ) -> Result<(), GraphDefinitionError> {
        if visiting.contains(node) {
            return Err(GraphDefinitionError::Cycle);
        }
        if visited.contains(node) {
            return Ok(());
        }
        visiting.insert(node.clone());
        for target in edges
            .iter()
            .filter(|((from, _), _)| from == node)
            .map(|(_, target)| target)
        {
            visit(target, edges, visiting, visited)?;
        }
        visiting.remove(node);
        visited.insert(node.clone());
        Ok(())
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    visit(start, edges, &mut visiting, &mut visited)?;
    if visited.len() != nodes.len() {
        return Err(GraphDefinitionError::UnreachableNode);
    }
    if !nodes
        .values()
        .any(|node| matches!(node.kind(), NodeKind::Complete | NodeKind::Fail))
    {
        return Err(GraphDefinitionError::NoTerminal);
    }
    Ok(())
}

fn definition_digest(
    start: &GraphNodeId,
    nodes: &BTreeMap<GraphNodeId, NodeDefinition>,
    edges: &BTreeMap<(GraphNodeId, GraphTransitionKey), GraphNodeId>,
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"enterprise-local-agent:graph:v1");
    update_text(&mut digest, start.as_str());
    for node in nodes.values() {
        digest.update([0x01]);
        update_text(&mut digest, node.id.as_str());
        digest.update([node_kind_tag(&node.kind), recovery_tag(node.recovery)]);
        if let NodeKind::Decision { branches } = &node.kind {
            let mut branches = branches.clone();
            branches.sort();
            for branch in branches {
                update_text(&mut digest, branch.as_str());
            }
        }
    }
    for ((from, transition), to) in edges {
        digest.update([0x02]);
        update_text(&mut digest, from.as_str());
        match transition {
            GraphTransitionKey::Succeeded => digest.update([0]),
            GraphTransitionKey::VerificationPassed => digest.update([1]),
            GraphTransitionKey::VerificationFailed => digest.update([2]),
            GraphTransitionKey::Branch(branch) => {
                digest.update([3]);
                update_text(&mut digest, branch.as_str());
            }
        }
        update_text(&mut digest, to.as_str());
    }
    digest.finalize().into()
}

const fn node_kind_tag(kind: &NodeKind) -> u8 {
    match kind {
        NodeKind::Retrieve => 0,
        NodeKind::Model => 1,
        NodeKind::Action => 2,
        NodeKind::Verify => 3,
        NodeKind::Decision { .. } => 4,
        NodeKind::Complete => 5,
        NodeKind::Fail => 6,
    }
}

const fn recovery_tag(recovery: GraphRecoveryMode) -> u8 {
    match recovery {
        GraphRecoveryMode::Never => 0,
        GraphRecoveryMode::FreshRetrieval => 1,
        GraphRecoveryMode::DeterministicBoundary => 2,
    }
}

fn update_text(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum GraphDefinitionError {
    #[error("graph exceeds a structural limit")]
    SizeLimit,
    #[error("graph contains a duplicate node")]
    DuplicateNode,
    #[error("graph references an unknown node")]
    UnknownNode,
    #[error("graph contains a duplicate transition")]
    DuplicateTransition,
    #[error("graph transition is invalid for its node kind")]
    InvalidTransition,
    #[error("terminal graph node has an outgoing edge")]
    TerminalOutgoingEdge,
    #[error("graph contains an unreachable node")]
    UnreachableNode,
    #[error("graph contains a cycle")]
    Cycle,
    #[error("graph has no terminal node")]
    NoTerminal,
    #[error("graph node recovery mode is invalid")]
    InvalidRecoveryMode,
    #[error("decision branches are invalid")]
    InvalidDecisionBranches,
}

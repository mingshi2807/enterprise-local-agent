use std::time::Duration;

use agent_core::{
    AgentEvent, AgentEventKind, ApprovalRequestId, BudgetUsage, CURRENT_EVENT_SCHEMA_VERSION,
    CapabilityKind, EventSequence, KnowledgeRetrievalId, KnowledgeRouteMetadata, LoopDecisionKind,
    LoopEventKind, LoopPhase, MAX_DURABLE_EVIDENCE_REFERENCES, MAX_DURABLE_KNOWLEDGE_BACKENDS,
    ModelCallId, RunOutcome, RunStatus, ToolCallId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::time::Instant;

use crate::{
    CURRENT_CHECKPOINT_SCHEMA_VERSION, CURRENT_STORE_SCHEMA_VERSION, LoadedRun, RecoveryContract,
    RunContext, RunKey, RunRecord, persistence::ContextSnapshot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum DurableLoopPosition {
    NotStarted,
    Ready,
    PhaseReady(LoopPhase),
    PhaseRunning(LoopPhase),
    ReflectCompleted,
    ReflectDecided(LoopDecisionKind),
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ContinuationState {
    InitialBoundary,
    RestartableBoundary,
    TransientStateRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PendingEffect {
    Model {
        model_call_id: ModelCallId,
    },
    Approval {
        approval_request_id: ApprovalRequestId,
    },
    Tool {
        tool_call_id: ToolCallId,
        capability: CapabilityKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingKnowledgeRetrieval {
    retrieval_id: KnowledgeRetrievalId,
    route: KnowledgeRouteMetadata,
    query_digest: [u8; 32],
    query_bytes: u16,
}

impl PendingKnowledgeRetrieval {
    #[must_use]
    pub const fn retrieval_id(&self) -> KnowledgeRetrievalId {
        self.retrieval_id
    }

    #[must_use]
    pub const fn route(&self) -> &KnowledgeRouteMetadata {
        &self.route
    }

    #[must_use]
    pub const fn query_digest(&self) -> [u8; 32] {
        self.query_digest
    }

    #[must_use]
    pub const fn query_bytes(&self) -> u16 {
        self.query_bytes
    }

    fn matches_request(
        &self,
        route: &KnowledgeRouteMetadata,
        query_digest: [u8; 32],
        query_bytes: u16,
    ) -> bool {
        self.route == *route && self.query_digest == query_digest && self.query_bytes == query_bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableKnowledgeManifest {
    retrieval_id: KnowledgeRetrievalId,
    evidence_count: u8,
    manifest_digest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRunState {
    context: ContextSnapshot,
    last_sequence: Option<EventSequence>,
    event_chain_digest: [u8; 32],
    current_iteration: Option<u32>,
    completed_iterations: u32,
    loop_position: DurableLoopPosition,
    continuation: ContinuationState,
    pending_effects: Vec<PendingEffect>,
    pending_knowledge_retrieval: Option<PendingKnowledgeRetrieval>,
    restartable_knowledge_retrieval: Option<PendingKnowledgeRetrieval>,
    latest_knowledge_manifest: Option<DurableKnowledgeManifest>,
}

impl DurableRunState {
    #[must_use]
    pub(crate) fn initial(record: &RunRecord) -> Self {
        Self {
            context: ContextSnapshot {
                key: record.key(),
                budget: *record.budget(),
                usage: BudgetUsage::default(),
                status: RunStatus::Pending,
                audit_degraded: false,
                started_at_unix_millis: None,
            },
            last_sequence: None,
            event_chain_digest: [0; 32],
            current_iteration: None,
            completed_iterations: 0,
            loop_position: DurableLoopPosition::NotStarted,
            continuation: ContinuationState::InitialBoundary,
            pending_effects: Vec::new(),
            pending_knowledge_retrieval: None,
            restartable_knowledge_retrieval: None,
            latest_knowledge_manifest: None,
        }
    }

    pub(crate) fn set_started_at(&mut self, value: u64) {
        self.context.started_at_unix_millis = Some(value);
    }

    #[must_use]
    pub(crate) const fn started_at_unix_millis(&self) -> Option<u64> {
        self.context.started_at_unix_millis
    }

    pub(crate) fn set_audit_degraded(&mut self) {
        self.context.audit_degraded = true;
    }

    pub(crate) fn apply(&mut self, event: &AgentEvent) -> Result<(), TransitionError> {
        if event.schema_version() != CURRENT_EVENT_SCHEMA_VERSION
            || event.run_id() != self.context.key.run_id()
        {
            return Err(TransitionError::IdentityOrVersion);
        }
        let expected = self
            .last_sequence
            .map_or(EventSequence::new(0), |sequence| {
                sequence
                    .checked_next()
                    .unwrap_or(EventSequence::new(u64::MAX))
            });
        if event.sequence() != expected {
            return Err(TransitionError::Sequence);
        }

        self.apply_kind(event.kind())?;
        let bytes = serde_json::to_vec(event).map_err(|_| TransitionError::Encoding)?;
        let mut digest = Sha256::new();
        digest.update(self.event_chain_digest);
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
        self.event_chain_digest = digest.finalize().into();
        self.last_sequence = Some(event.sequence());
        Ok(())
    }

    fn apply_kind(&mut self, kind: &AgentEventKind) -> Result<(), TransitionError> {
        match kind {
            AgentEventKind::RunStarted {
                started_at_unix_millis,
            } => {
                if !matches!(self.context.status, RunStatus::Pending) {
                    return Err(TransitionError::Lifecycle);
                }
                if self
                    .context
                    .started_at_unix_millis
                    .is_some_and(|current| current != *started_at_unix_millis)
                {
                    return Err(TransitionError::Lifecycle);
                }
                self.context.started_at_unix_millis = Some(*started_at_unix_millis);
                self.context.status = RunStatus::Running;
            }
            AgentEventKind::RunFinished { outcome } => {
                if !matches!(self.context.status, RunStatus::Running) {
                    return Err(TransitionError::Lifecycle);
                }
                self.context.status = RunStatus::Finished(outcome.clone());
            }
            AgentEventKind::AuditDegraded => self.context.audit_degraded = true,
            AgentEventKind::ModelInvocationStarted {
                model_call_id,
                usage,
                limit,
            } => {
                if *limit != self.context.budget.max_model_calls()
                    || *usage != self.context.usage.model_calls().saturating_add(1)
                {
                    return Err(TransitionError::Budget);
                }
                self.context.usage = BudgetUsage::with_approval_requests(
                    *usage,
                    self.context.usage.tool_calls(),
                    self.context.usage.iterations(),
                    self.context.usage.approval_requests(),
                );
                self.pending_effects.push(PendingEffect::Model {
                    model_call_id: *model_call_id,
                });
                self.continuation = ContinuationState::TransientStateRequired;
            }
            AgentEventKind::ModelInvocationCompleted { model_call_id, .. }
            | AgentEventKind::ModelInvocationFailed { model_call_id } => {
                self.remove_pending(PendingEffectKey::Model(*model_call_id))?;
            }
            AgentEventKind::ApprovalRequested {
                approval_request_id,
                usage,
                limit,
                ..
            } => {
                if *limit != self.context.budget.max_approval_requests()
                    || *usage != self.context.usage.approval_requests().saturating_add(1)
                {
                    return Err(TransitionError::Budget);
                }
                self.context.usage = BudgetUsage::with_approval_requests(
                    self.context.usage.model_calls(),
                    self.context.usage.tool_calls(),
                    self.context.usage.iterations(),
                    *usage,
                );
                self.pending_effects.push(PendingEffect::Approval {
                    approval_request_id: *approval_request_id,
                });
                self.continuation = ContinuationState::TransientStateRequired;
            }
            AgentEventKind::ApprovalGranted {
                approval_request_id,
            }
            | AgentEventKind::ApprovalDenied {
                approval_request_id,
            }
            | AgentEventKind::ApprovalFailed {
                approval_request_id,
                ..
            } => self.remove_pending(PendingEffectKey::Approval(*approval_request_id))?,
            AgentEventKind::ToolInvocationStarted {
                tool_call_id,
                capability,
                usage,
                limit,
                ..
            } => {
                if *limit != self.context.budget.max_tool_calls()
                    || *usage != self.context.usage.tool_calls().saturating_add(1)
                {
                    return Err(TransitionError::Budget);
                }
                self.context.usage = BudgetUsage::with_approval_requests(
                    self.context.usage.model_calls(),
                    *usage,
                    self.context.usage.iterations(),
                    self.context.usage.approval_requests(),
                );
                self.pending_effects.push(PendingEffect::Tool {
                    tool_call_id: *tool_call_id,
                    capability: *capability,
                });
                self.continuation = ContinuationState::TransientStateRequired;
            }
            AgentEventKind::ToolInvocationCompleted { tool_call_id }
            | AgentEventKind::ToolInvocationDomainFailed { tool_call_id, .. }
            | AgentEventKind::ToolInvocationAdapterFailed { tool_call_id } => {
                self.remove_pending(PendingEffectKey::Tool(*tool_call_id))?;
            }
            AgentEventKind::KnowledgeRetrievalStarted {
                retrieval_id,
                route,
                query_digest,
                query_bytes,
            } => {
                self.start_knowledge_retrieval(
                    *retrieval_id,
                    route.clone(),
                    *query_digest,
                    *query_bytes,
                )?;
            }
            AgentEventKind::KnowledgeRetrievalRestarted {
                previous_retrieval_id,
                retrieval_id,
                route,
                query_digest,
                query_bytes,
            } => {
                let pending = self
                    .pending_knowledge_retrieval
                    .as_ref()
                    .ok_or(TransitionError::Effect)?;
                if pending.retrieval_id != *previous_retrieval_id
                    || !pending.matches_request(route, *query_digest, *query_bytes)
                {
                    return Err(TransitionError::Effect);
                }
                self.pending_knowledge_retrieval = None;
                self.start_knowledge_retrieval(
                    *retrieval_id,
                    route.clone(),
                    *query_digest,
                    *query_bytes,
                )?;
            }
            AgentEventKind::KnowledgeRetrievalCompleted {
                retrieval_id,
                snapshots,
                evidence_references,
                evidence_count,
                manifest_digest,
                ..
            } => {
                let route = self
                    .restartable_knowledge_retrieval
                    .as_ref()
                    .map(|attempt| attempt.route.clone())
                    .ok_or(TransitionError::Effect)?;
                if usize::from(*evidence_count) != evidence_references.len()
                    || evidence_references.len() > MAX_DURABLE_EVIDENCE_REFERENCES
                    || snapshots.len() > MAX_DURABLE_KNOWLEDGE_BACKENDS
                    || evidence_references.iter().any(|item| !item.is_valid())
                    || snapshots.iter().any(|item| !item.is_valid())
                    || evidence_references
                        .iter()
                        .any(|item| !route.backends().contains(&item.backend()))
                    || snapshots
                        .iter()
                        .any(|item| !route.backends().contains(&item.backend()))
                    || snapshots.iter().enumerate().any(|(index, item)| {
                        snapshots[..index]
                            .iter()
                            .any(|prior| prior.backend() == item.backend())
                    })
                {
                    return Err(TransitionError::Effect);
                }
                self.finish_knowledge_retrieval(*retrieval_id)?;
                self.latest_knowledge_manifest = Some(DurableKnowledgeManifest {
                    retrieval_id: *retrieval_id,
                    evidence_count: *evidence_count,
                    manifest_digest: *manifest_digest,
                });
            }
            AgentEventKind::KnowledgeRetrievalFailed { retrieval_id, .. } => {
                self.finish_knowledge_retrieval(*retrieval_id)?;
                self.latest_knowledge_manifest = None;
            }
            AgentEventKind::ModelGroundingBound {
                retrieval_id,
                model_call_id,
                evidence_count,
                manifest_digest,
                ..
            } => {
                let manifest = self
                    .latest_knowledge_manifest
                    .as_ref()
                    .ok_or(TransitionError::Effect)?;
                if manifest.retrieval_id != *retrieval_id
                    || manifest.evidence_count != *evidence_count
                    || manifest.manifest_digest != *manifest_digest
                    || !self.pending_effects.iter().any(|effect| {
                        matches!(
                            effect,
                            PendingEffect::Model {
                                model_call_id: pending
                            } if pending == model_call_id
                        )
                    })
                {
                    return Err(TransitionError::Effect);
                }
                self.continuation = ContinuationState::TransientStateRequired;
            }
            AgentEventKind::Loop { event } => self.apply_loop(*event)?,
            AgentEventKind::ActionProposed { .. }
            | AgentEventKind::ActionValidated { .. }
            | AgentEventKind::ActionRejected { .. }
            | AgentEventKind::ActionExecutionBound { .. }
            | AgentEventKind::ToolPolicyDenied { .. }
            | AgentEventKind::ContainmentFailed { .. } => {
                self.continuation = ContinuationState::TransientStateRequired;
            }
        }
        Ok(())
    }

    fn apply_loop(&mut self, event: LoopEventKind) -> Result<(), TransitionError> {
        match event {
            LoopEventKind::IterationStarted {
                iteration,
                usage,
                limit,
            } => {
                if !matches!(
                    self.loop_position,
                    DurableLoopPosition::Ready | DurableLoopPosition::NotStarted
                ) || usage != self.context.usage.iterations().saturating_add(1)
                    || limit != self.context.budget.max_iterations()
                    || iteration != usage
                {
                    return Err(TransitionError::Loop);
                }
                self.context.usage = BudgetUsage::with_approval_requests(
                    self.context.usage.model_calls(),
                    self.context.usage.tool_calls(),
                    usage,
                    self.context.usage.approval_requests(),
                );
                self.current_iteration = Some(iteration);
                self.loop_position = DurableLoopPosition::PhaseReady(LoopPhase::Observe);
                self.continuation = ContinuationState::TransientStateRequired;
            }
            LoopEventKind::PhaseEntered { iteration, phase } => {
                if self.current_iteration != Some(iteration)
                    || self.loop_position != DurableLoopPosition::PhaseReady(phase)
                {
                    return Err(TransitionError::Loop);
                }
                self.loop_position = DurableLoopPosition::PhaseRunning(phase);
            }
            LoopEventKind::PhaseCompleted { iteration, phase } => {
                if self.current_iteration != Some(iteration)
                    || self.loop_position != DurableLoopPosition::PhaseRunning(phase)
                {
                    return Err(TransitionError::Loop);
                }
                if phase == LoopPhase::Retrieve && self.pending_knowledge_retrieval.is_some() {
                    return Err(TransitionError::Effect);
                }
                self.loop_position = match phase {
                    LoopPhase::Observe => DurableLoopPosition::PhaseReady(LoopPhase::Retrieve),
                    LoopPhase::Retrieve => DurableLoopPosition::PhaseReady(LoopPhase::Plan),
                    LoopPhase::Plan => DurableLoopPosition::PhaseReady(LoopPhase::Act),
                    LoopPhase::Act => DurableLoopPosition::PhaseReady(LoopPhase::Verify),
                    LoopPhase::Verify => DurableLoopPosition::PhaseReady(LoopPhase::Reflect),
                    LoopPhase::Reflect => DurableLoopPosition::ReflectCompleted,
                };
                if phase == LoopPhase::Retrieve {
                    self.restartable_knowledge_retrieval = None;
                }
            }
            LoopEventKind::ReflectDecision {
                iteration,
                decision,
            } => {
                if self.current_iteration != Some(iteration)
                    || self.loop_position != DurableLoopPosition::ReflectCompleted
                {
                    return Err(TransitionError::Loop);
                }
                self.completed_iterations = self.completed_iterations.saturating_add(1);
                self.loop_position = DurableLoopPosition::ReflectDecided(decision);
            }
            LoopEventKind::IterationCompleted { iteration } => {
                if self.current_iteration != Some(iteration) {
                    return Err(TransitionError::Loop);
                }
                match self.loop_position {
                    DurableLoopPosition::ReflectDecided(LoopDecisionKind::Continue) => {
                        self.current_iteration = None;
                        self.loop_position = DurableLoopPosition::Ready;
                        self.continuation = ContinuationState::RestartableBoundary;
                    }
                    DurableLoopPosition::ReflectDecided(
                        LoopDecisionKind::Complete | LoopDecisionKind::Fail,
                    ) => self.loop_position = DurableLoopPosition::Finished,
                    _ => return Err(TransitionError::Loop),
                }
            }
            LoopEventKind::LoopCompleted {
                completed_iterations,
            } => {
                if completed_iterations != self.completed_iterations {
                    return Err(TransitionError::Loop);
                }
                self.loop_position = DurableLoopPosition::Finished;
            }
            LoopEventKind::LoopFailed { iteration, .. } => {
                if self.current_iteration != Some(iteration) {
                    return Err(TransitionError::Loop);
                }
                self.loop_position = DurableLoopPosition::Finished;
            }
        }
        Ok(())
    }

    fn remove_pending(&mut self, key: PendingEffectKey) -> Result<(), TransitionError> {
        let position = self
            .pending_effects
            .iter()
            .position(|effect| key.matches(*effect))
            .ok_or(TransitionError::Effect)?;
        self.pending_effects.remove(position);
        Ok(())
    }

    fn start_knowledge_retrieval(
        &mut self,
        retrieval_id: KnowledgeRetrievalId,
        route: KnowledgeRouteMetadata,
        query_digest: [u8; 32],
        query_bytes: u16,
    ) -> Result<(), TransitionError> {
        if self.pending_knowledge_retrieval.is_some()
            || !route.is_valid()
            || query_bytes == 0
            || usize::from(query_bytes) > agent_knowledge::MAX_QUERY_BYTES
            || self.loop_position != DurableLoopPosition::PhaseRunning(LoopPhase::Retrieve)
        {
            return Err(TransitionError::Effect);
        }
        let pending = PendingKnowledgeRetrieval {
            retrieval_id,
            route,
            query_digest,
            query_bytes,
        };
        self.pending_knowledge_retrieval = Some(pending.clone());
        self.restartable_knowledge_retrieval = Some(pending);
        self.latest_knowledge_manifest = None;
        self.continuation = ContinuationState::TransientStateRequired;
        Ok(())
    }

    fn finish_knowledge_retrieval(
        &mut self,
        retrieval_id: KnowledgeRetrievalId,
    ) -> Result<(), TransitionError> {
        let pending = self
            .pending_knowledge_retrieval
            .as_ref()
            .ok_or(TransitionError::Effect)?;
        if pending.retrieval_id != retrieval_id {
            return Err(TransitionError::Effect);
        }
        self.pending_knowledge_retrieval = None;
        Ok(())
    }

    #[must_use]
    pub const fn key(&self) -> RunKey {
        self.context.key
    }

    #[must_use]
    pub const fn budget(&self) -> &agent_core::RunBudget {
        &self.context.budget
    }

    #[must_use]
    pub const fn usage(&self) -> BudgetUsage {
        self.context.usage
    }

    #[must_use]
    pub const fn status(&self) -> &RunStatus {
        &self.context.status
    }

    #[must_use]
    pub const fn audit_degraded(&self) -> bool {
        self.context.audit_degraded
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<EventSequence> {
        self.last_sequence
    }

    #[must_use]
    pub const fn event_chain_digest(&self) -> [u8; 32] {
        self.event_chain_digest
    }

    #[must_use]
    pub const fn loop_position(&self) -> DurableLoopPosition {
        self.loop_position
    }

    #[must_use]
    pub const fn current_iteration(&self) -> Option<u32> {
        self.current_iteration
    }

    #[must_use]
    pub const fn completed_iterations(&self) -> u32 {
        self.completed_iterations
    }

    #[must_use]
    pub const fn continuation(&self) -> ContinuationState {
        self.continuation
    }

    #[must_use]
    pub fn pending_effects(&self) -> &[PendingEffect] {
        &self.pending_effects
    }

    #[must_use]
    pub const fn pending_knowledge_retrieval(&self) -> Option<&PendingKnowledgeRetrieval> {
        self.pending_knowledge_retrieval.as_ref()
    }

    #[must_use]
    pub const fn restartable_knowledge_retrieval(&self) -> Option<&PendingKnowledgeRetrieval> {
        self.restartable_knowledge_retrieval.as_ref()
    }

    #[must_use]
    pub const fn is_quiescent(&self) -> bool {
        self.pending_effects.is_empty() && self.pending_knowledge_retrieval.is_none()
    }
}

#[derive(Clone, Copy)]
enum PendingEffectKey {
    Model(ModelCallId),
    Approval(ApprovalRequestId),
    Tool(ToolCallId),
}

impl PendingEffectKey {
    fn matches(self, effect: PendingEffect) -> bool {
        match (self, effect) {
            (
                Self::Model(left),
                PendingEffect::Model {
                    model_call_id: right,
                },
            ) => left == right,
            (
                Self::Approval(left),
                PendingEffect::Approval {
                    approval_request_id: right,
                },
            ) => left == right,
            (
                Self::Tool(left),
                PendingEffect::Tool {
                    tool_call_id: right,
                    ..
                },
            ) => left == right,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TransitionError {
    #[error("event identity or schema version is invalid")]
    IdentityOrVersion,
    #[error("event sequence is invalid")]
    Sequence,
    #[error("event lifecycle transition is invalid")]
    Lifecycle,
    #[error("event budget transition is invalid")]
    Budget,
    #[error("event loop transition is invalid")]
    Loop,
    #[error("event effect transition is invalid")]
    Effect,
    #[error("event encoding failed")]
    Encoding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualReconciliationReason {
    UnresolvedEffect(PendingEffect),
    NonRestartableProgram,
    TransientStateUnavailable,
}

pub struct RecoveredRun {
    context: RunContext,
    state: DurableRunState,
    recovery_contract: RecoveryContract,
}

impl RecoveredRun {
    #[must_use]
    pub const fn context(&self) -> &RunContext {
        &self.context
    }

    #[must_use]
    pub const fn state(&self) -> &DurableRunState {
        &self.state
    }

    #[must_use]
    pub const fn recovery_contract(&self) -> RecoveryContract {
        self.recovery_contract
    }

    #[must_use]
    pub fn into_context(self) -> RunContext {
        self.context
    }

    #[must_use]
    pub fn into_parts(self) -> (RunContext, DurableRunState, RecoveryContract) {
        (self.context, self.state, self.recovery_contract)
    }
}

pub enum RecoveryDisposition {
    Completed {
        state: DurableRunState,
    },
    TerminalFailure {
        state: DurableRunState,
        outcome: RunOutcome,
    },
    Resumable(Box<RecoveredRun>),
    ManualReconciliationRequired {
        state: DurableRunState,
        reason: ManualReconciliationReason,
    },
}

pub(crate) fn recover_loaded_run(
    loaded: LoadedRun,
    now_unix_millis: u64,
    now: Instant,
) -> Result<RecoveryDisposition, RecoveryError> {
    let record = loaded.record();
    if record.store_schema_version() != CURRENT_STORE_SCHEMA_VERSION
        || record.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
        || loaded.checkpoint().checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
        || loaded.checkpoint().state().key() != record.key()
        || loaded.checkpoint().state().budget() != record.budget()
    {
        return Err(RecoveryError::UnsupportedOrInconsistentSchema);
    }

    let checkpoint_sequence = loaded.checkpoint().state().last_sequence();
    let mut expected = EventSequence::new(0);
    let mut checkpoint_state = DurableRunState::initial(record);
    for event in loaded.events() {
        if event.run_id() != record.key().run_id()
            || event.schema_version() != CURRENT_EVENT_SCHEMA_VERSION
            || event.sequence() != expected
        {
            return Err(RecoveryError::CorruptLog);
        }
        if checkpoint_sequence.is_some_and(|sequence| event.sequence().get() <= sequence.get()) {
            checkpoint_state
                .apply(event)
                .map_err(|_| RecoveryError::CorruptLog)?;
            if checkpoint_sequence == Some(event.sequence())
                && &checkpoint_state != loaded.checkpoint().state()
            {
                return Err(RecoveryError::CorruptCheckpoint);
            }
        }
        expected = expected.checked_next().ok_or(RecoveryError::CorruptLog)?;
    }
    if checkpoint_sequence.is_some_and(|sequence| {
        loaded
            .events()
            .get(sequence.get() as usize)
            .map(AgentEvent::sequence)
            != Some(sequence)
    }) {
        return Err(RecoveryError::CorruptCheckpoint);
    }
    if checkpoint_sequence.is_none()
        && loaded.checkpoint().state() != &DurableRunState::initial(record)
    {
        return Err(RecoveryError::CorruptCheckpoint);
    }

    let mut state = loaded.checkpoint().clone().into_state();
    let tail_start = checkpoint_sequence.map_or(0, |sequence| sequence.get().saturating_add(1));
    let tail_start = usize::try_from(tail_start).map_err(|_| RecoveryError::CorruptLog)?;
    for event in loaded
        .events()
        .get(tail_start..)
        .ok_or(RecoveryError::CorruptLog)?
    {
        state.apply(event).map_err(|_| RecoveryError::CorruptLog)?;
    }

    if let Some(effect) = state.pending_effects().first().copied() {
        return Ok(RecoveryDisposition::ManualReconciliationRequired {
            state,
            reason: ManualReconciliationReason::UnresolvedEffect(effect),
        });
    }
    match state.status().clone() {
        RunStatus::Finished(RunOutcome::Completed) => Ok(RecoveryDisposition::Completed { state }),
        RunStatus::Finished(outcome) => Ok(RecoveryDisposition::TerminalFailure { state, outcome }),
        RunStatus::Pending => Ok(RecoveryDisposition::ManualReconciliationRequired {
            state,
            reason: ManualReconciliationReason::TransientStateUnavailable,
        }),
        RunStatus::Running => {
            let started = state
                .context
                .started_at_unix_millis
                .ok_or(RecoveryError::CorruptCheckpoint)?;
            let elapsed_millis = now_unix_millis
                .checked_sub(started)
                .ok_or(RecoveryError::ClockAmbiguous)?;
            let elapsed = Duration::from_millis(elapsed_millis);
            if elapsed >= state.budget().max_elapsed() {
                return Ok(RecoveryDisposition::TerminalFailure {
                    state,
                    outcome: RunOutcome::BudgetExceeded {
                        dimension: agent_core::BudgetDimension::Elapsed,
                    },
                });
            }
            if matches!(record.recovery_contract(), RecoveryContract::NonRestartable) {
                return Ok(RecoveryDisposition::ManualReconciliationRequired {
                    state,
                    reason: ManualReconciliationReason::NonRestartableProgram,
                });
            }
            let retrieval_restart = matches!(
                record.recovery_contract(),
                RecoveryContract::RestartableRetrieval { .. }
            ) && state.loop_position()
                == DurableLoopPosition::PhaseRunning(LoopPhase::Retrieve)
                && state.restartable_knowledge_retrieval().is_some();
            if !retrieval_restart
                && !matches!(
                    state.continuation(),
                    ContinuationState::InitialBoundary | ContinuationState::RestartableBoundary
                )
            {
                return Ok(RecoveryDisposition::ManualReconciliationRequired {
                    state,
                    reason: ManualReconciliationReason::TransientStateUnavailable,
                });
            }
            let context =
                RunContext::from_recovery(&state, elapsed, now, record.recovery_contract())?;
            Ok(RecoveryDisposition::Resumable(Box::new(RecoveredRun {
                context,
                state,
                recovery_contract: record.recovery_contract(),
            })))
        }
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum RecoveryError {
    #[error("durable run schemas are unsupported or inconsistent")]
    UnsupportedOrInconsistentSchema,
    #[error("durable event log is corrupt")]
    CorruptLog,
    #[error("durable checkpoint is corrupt")]
    CorruptCheckpoint,
    #[error("wall clock moved before the durable run start")]
    ClockAmbiguous,
    #[error("recovered runtime state is invalid")]
    InvalidRuntime,
}

impl From<crate::RunContextError> for RecoveryError {
    fn from(_: crate::RunContextError) -> Self {
        Self::InvalidRuntime
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_core::{
        ActionProposalId, ApprovalRequestId, KnowledgeBackendId, KnowledgeEvidenceReference,
        KnowledgeFailureKind, KnowledgeRouteMetadata, RunBudget, RunId, SessionId, ToolName,
    };

    use super::*;
    use crate::{DurableCheckpoint, LoadedRun};

    const STARTED_AT: u64 = 1_000;

    fn record(contract: RecoveryContract) -> RunRecord {
        RunRecord::new(
            RunKey::new(RunId::new(), SessionId::new()),
            RunBudget::new(4, 4, 4, Duration::from_secs(60))
                .expect("budget must be valid")
                .with_max_approval_requests(4),
            contract,
        )
    }

    fn event(record: &RunRecord, sequence: u64, kind: AgentEventKind) -> AgentEvent {
        AgentEvent::new(record.key().run_id(), EventSequence::new(sequence), kind)
    }

    fn started(record: &RunRecord) -> AgentEvent {
        event(
            record,
            0,
            AgentEventKind::RunStarted {
                started_at_unix_millis: STARTED_AT,
            },
        )
    }

    fn recover(
        record: RunRecord,
        checkpoint: DurableCheckpoint,
        events: Vec<AgentEvent>,
    ) -> Result<RecoveryDisposition, RecoveryError> {
        recover_loaded_run(
            LoadedRun::new(record, checkpoint, events),
            STARTED_AT + 1,
            Instant::now(),
        )
    }

    fn retrieval_prefix(record: &RunRecord) -> Vec<AgentEvent> {
        vec![
            started(record),
            event(
                record,
                1,
                AgentEventKind::Loop {
                    event: LoopEventKind::IterationStarted {
                        iteration: 1,
                        usage: 1,
                        limit: 4,
                    },
                },
            ),
            event(
                record,
                2,
                AgentEventKind::Loop {
                    event: LoopEventKind::PhaseEntered {
                        iteration: 1,
                        phase: LoopPhase::Observe,
                    },
                },
            ),
            event(
                record,
                3,
                AgentEventKind::Loop {
                    event: LoopEventKind::PhaseCompleted {
                        iteration: 1,
                        phase: LoopPhase::Observe,
                    },
                },
            ),
            event(
                record,
                4,
                AgentEventKind::Loop {
                    event: LoopEventKind::PhaseEntered {
                        iteration: 1,
                        phase: LoopPhase::Retrieve,
                    },
                },
            ),
        ]
    }

    #[test]
    fn interrupted_read_only_retrieval_is_resumable_only_under_explicit_contract() {
        for (contract, expected_resumable) in [
            (RecoveryContract::Restartable { version: 7 }, false),
            (RecoveryContract::RestartableRetrieval { version: 7 }, true),
        ] {
            let record = record(contract);
            let retrieval_id = KnowledgeRetrievalId::new();
            let mut events = retrieval_prefix(&record);
            events.push(event(
                &record,
                5,
                AgentEventKind::KnowledgeRetrievalStarted {
                    retrieval_id,
                    route: KnowledgeRouteMetadata::Single {
                        backend: KnowledgeBackendId::StandardsMcp,
                    },
                    query_digest: [9; 32],
                    query_bytes: 12,
                },
            ));
            let disposition = recover(record.clone(), DurableCheckpoint::initial(&record), events)
                .expect("recovery classification");
            if expected_resumable {
                let RecoveryDisposition::Resumable(recovered) = disposition else {
                    panic!("explicit retrieval recovery must be resumable");
                };
                assert_eq!(
                    recovered
                        .state()
                        .pending_knowledge_retrieval()
                        .map(PendingKnowledgeRetrieval::retrieval_id),
                    Some(retrieval_id)
                );
            } else {
                assert!(matches!(
                    disposition,
                    RecoveryDisposition::ManualReconciliationRequired {
                        reason: ManualReconciliationReason::TransientStateUnavailable,
                        ..
                    }
                ));
            }
        }
    }

    #[test]
    fn completed_retrieval_with_lost_transient_evidence_can_be_freshly_retrieved() {
        let record = record(RecoveryContract::RestartableRetrieval { version: 7 });
        let retrieval_id = KnowledgeRetrievalId::new();
        let mut events = retrieval_prefix(&record);
        events.extend([
            event(
                &record,
                5,
                AgentEventKind::KnowledgeRetrievalStarted {
                    retrieval_id,
                    route: KnowledgeRouteMetadata::Single {
                        backend: KnowledgeBackendId::StandardsMcp,
                    },
                    query_digest: [3; 32],
                    query_bytes: 8,
                },
            ),
            event(
                &record,
                6,
                AgentEventKind::KnowledgeRetrievalCompleted {
                    retrieval_id,
                    snapshots: Vec::new(),
                    evidence_references: vec![
                        KnowledgeEvidenceReference::new(
                            KnowledgeBackendId::StandardsMcp,
                            "ISO15118-20:chunk-1".to_owned(),
                        )
                        .expect("reference"),
                    ],
                    evidence_count: 1,
                    truncated: false,
                    degraded: false,
                    manifest_digest: [4; 32],
                },
            ),
        ]);
        let disposition = recover(record.clone(), DurableCheckpoint::initial(&record), events)
            .expect("recovery classification");

        let RecoveryDisposition::Resumable(recovered) = disposition else {
            panic!("completed retrieval at an unfinished Retrieve phase must be resumable");
        };
        assert!(recovered.state().pending_knowledge_retrieval().is_none());
        assert!(
            recovered
                .state()
                .restartable_knowledge_retrieval()
                .is_some()
        );
    }

    #[test]
    fn retrieval_failure_is_terminal_metadata_not_an_unresolved_effect() {
        let record = record(RecoveryContract::RestartableRetrieval { version: 7 });
        let retrieval_id = KnowledgeRetrievalId::new();
        let mut events = retrieval_prefix(&record);
        events.extend([
            event(
                &record,
                5,
                AgentEventKind::KnowledgeRetrievalStarted {
                    retrieval_id,
                    route: KnowledgeRouteMetadata::Single {
                        backend: KnowledgeBackendId::OcppRagKag,
                    },
                    query_digest: [1; 32],
                    query_bytes: 4,
                },
            ),
            event(
                &record,
                6,
                AgentEventKind::KnowledgeRetrievalFailed {
                    retrieval_id,
                    kind: KnowledgeFailureKind::Unavailable,
                },
            ),
        ]);

        assert!(matches!(
            recover(record.clone(), DurableCheckpoint::initial(&record), events)
                .expect("recovery classification"),
            RecoveryDisposition::Resumable(_)
        ));
    }

    #[test]
    fn full_log_restores_budget_audit_and_terminal_state() {
        let record = record(RecoveryContract::NonRestartable);
        let model_call_id = ModelCallId::new();
        let events = vec![
            started(&record),
            event(
                &record,
                1,
                AgentEventKind::ModelInvocationStarted {
                    model_call_id,
                    usage: 1,
                    limit: 4,
                },
            ),
            event(
                &record,
                2,
                AgentEventKind::ModelInvocationCompleted {
                    model_call_id,
                    token_usage: None,
                },
            ),
            event(&record, 3, AgentEventKind::AuditDegraded),
            event(
                &record,
                4,
                AgentEventKind::RunFinished {
                    outcome: RunOutcome::Completed,
                },
            ),
        ];
        let disposition = recover(record.clone(), DurableCheckpoint::initial(&record), events)
            .expect("full log must recover");
        let RecoveryDisposition::Completed { state } = disposition else {
            panic!("completed run must restore as completed");
        };
        assert_eq!(state.usage().model_calls(), 1);
        assert!(state.audit_degraded());
        assert_eq!(state.last_sequence(), Some(EventSequence::new(4)));
    }

    #[test]
    fn checkpoint_and_tail_use_the_same_reducer() {
        let record = record(RecoveryContract::Restartable { version: 7 });
        let start = started(&record);
        let mut checkpoint_state = DurableRunState::initial(&record);
        checkpoint_state.apply(&start).expect("start must reduce");
        let model_call_id = ModelCallId::new();
        let events = vec![
            start,
            event(
                &record,
                1,
                AgentEventKind::ModelInvocationStarted {
                    model_call_id,
                    usage: 1,
                    limit: 4,
                },
            ),
        ];
        let disposition = recover(record, DurableCheckpoint::new(checkpoint_state), events)
            .expect("checkpoint and tail must recover");
        assert!(matches!(
            disposition,
            RecoveryDisposition::ManualReconciliationRequired {
                reason: ManualReconciliationReason::UnresolvedEffect(PendingEffect::Model { .. }),
                ..
            }
        ));
    }

    #[test]
    fn gaps_duplicates_wrong_run_and_checkpoint_chain_fail_closed() {
        let record = record(RecoveryContract::NonRestartable);
        for events in [
            vec![event(&record, 1, AgentEventKind::AuditDegraded)],
            vec![started(&record), started(&record)],
            vec![AgentEvent::new(
                RunId::new(),
                EventSequence::new(0),
                AgentEventKind::AuditDegraded,
            )],
        ] {
            assert_eq!(
                recover(record.clone(), DurableCheckpoint::initial(&record), events,).err(),
                Some(RecoveryError::CorruptLog)
            );
        }

        let mut wrong = DurableRunState::initial(&record);
        wrong.apply(&started(&record)).expect("start must reduce");
        wrong.event_chain_digest[0] ^= 1;
        assert_eq!(
            recover(
                record.clone(),
                DurableCheckpoint::new(wrong),
                vec![started(&record)],
            )
            .err(),
            Some(RecoveryError::CorruptCheckpoint)
        );
    }

    #[test]
    fn unresolved_external_effects_always_require_reconciliation() {
        let name = ToolName::new("workspace_write_file").expect("name must be valid");
        let cases = [
            AgentEventKind::ModelInvocationStarted {
                model_call_id: ModelCallId::new(),
                usage: 1,
                limit: 4,
            },
            AgentEventKind::ApprovalRequested {
                approval_request_id: ApprovalRequestId::new(),
                action_proposal_id: ActionProposalId::new(),
                tool_call_id: ToolCallId::new(),
                tool_name: name.clone(),
                capability: CapabilityKind::LocalWrite,
                usage: 1,
                limit: 4,
            },
            AgentEventKind::ToolInvocationStarted {
                tool_call_id: ToolCallId::new(),
                tool_name: name,
                capability: CapabilityKind::LocalWrite,
                usage: 1,
                limit: 4,
            },
        ];
        for kind in cases {
            let record = record(RecoveryContract::Restartable { version: 1 });
            let disposition = recover(
                record.clone(),
                DurableCheckpoint::initial(&record),
                vec![started(&record), event(&record, 1, kind)],
            )
            .expect("uncertain effect must classify");
            assert!(matches!(
                disposition,
                RecoveryDisposition::ManualReconciliationRequired {
                    reason: ManualReconciliationReason::UnresolvedEffect(_),
                    ..
                }
            ));
        }
    }

    #[test]
    fn every_running_loop_phase_is_non_resumable_without_transient_state() {
        for stopped_phase in [
            LoopPhase::Observe,
            LoopPhase::Retrieve,
            LoopPhase::Plan,
            LoopPhase::Act,
            LoopPhase::Verify,
            LoopPhase::Reflect,
        ] {
            let record = record(RecoveryContract::Restartable { version: 1 });
            let mut events = vec![started(&record)];
            let mut sequence = 1;
            events.push(event(
                &record,
                sequence,
                AgentEventKind::Loop {
                    event: LoopEventKind::IterationStarted {
                        iteration: 1,
                        usage: 1,
                        limit: 4,
                    },
                },
            ));
            sequence += 1;
            for phase in [
                LoopPhase::Observe,
                LoopPhase::Retrieve,
                LoopPhase::Plan,
                LoopPhase::Act,
                LoopPhase::Verify,
                LoopPhase::Reflect,
            ] {
                events.push(event(
                    &record,
                    sequence,
                    AgentEventKind::Loop {
                        event: LoopEventKind::PhaseEntered {
                            iteration: 1,
                            phase,
                        },
                    },
                ));
                sequence += 1;
                if phase == stopped_phase {
                    break;
                }
                events.push(event(
                    &record,
                    sequence,
                    AgentEventKind::Loop {
                        event: LoopEventKind::PhaseCompleted {
                            iteration: 1,
                            phase,
                        },
                    },
                ));
                sequence += 1;
            }
            assert!(matches!(
                recover(record.clone(), DurableCheckpoint::initial(&record), events,)
                    .expect("phase must classify"),
                RecoveryDisposition::ManualReconciliationRequired {
                    reason: ManualReconciliationReason::TransientStateUnavailable,
                    ..
                }
            ));
        }
    }

    #[test]
    fn non_restartable_state_is_never_reconstructed_and_clock_rollback_fails() {
        let record = record(RecoveryContract::NonRestartable);
        assert!(matches!(
            recover(
                record.clone(),
                DurableCheckpoint::initial(&record),
                vec![started(&record)],
            )
            .expect("run must classify"),
            RecoveryDisposition::ManualReconciliationRequired {
                reason: ManualReconciliationReason::NonRestartableProgram,
                ..
            }
        ));
        assert_eq!(
            recover_loaded_run(
                LoadedRun::new(
                    record.clone(),
                    DurableCheckpoint::initial(&record),
                    vec![started(&record)],
                ),
                STARTED_AT - 1,
                Instant::now(),
            )
            .err(),
            Some(RecoveryError::ClockAmbiguous)
        );
    }
}

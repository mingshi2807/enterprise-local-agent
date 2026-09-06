use agent_core::{
    GraphFailureKind, GraphNodeAttemptId, GraphNodeId, GraphRecoveryMode, GraphTransitionKey,
};
use agent_harness::{
    DurableGraphPosition, ExecutionHarness, RecoveredRun, RecoveryContract, RunContext,
};

use crate::{
    ActionEffects, DecisionContext, GraphDefinition, GraphError, GraphProgram, ModelEffects,
    NodeKind, RestartableGraphProgram, RetrieveEffects, VerificationOutcome, VerifyEffects,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphTerminalOutcome {
    Complete,
    Fail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphRunSummary {
    steps: u32,
    terminal: GraphTerminalOutcome,
}

impl GraphRunSummary {
    #[must_use]
    pub const fn steps(self) -> u32 {
        self.steps
    }

    #[must_use]
    pub const fn terminal(self) -> GraphTerminalOutcome {
        self.terminal
    }
}

#[derive(Clone)]
struct ResumeNode {
    node_id: GraphNodeId,
    previous_attempt: Option<GraphNodeAttemptId>,
}

pub struct GraphEngine<'a> {
    definition: &'a GraphDefinition,
}

impl<'a> GraphEngine<'a> {
    #[must_use]
    pub const fn new(definition: &'a GraphDefinition) -> Self {
        Self { definition }
    }

    pub async fn run<P: GraphProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        state: &mut P::WorkingState,
    ) -> Result<GraphRunSummary, GraphError> {
        let start = self
            .definition
            .node(self.definition.start())
            .ok_or(GraphError::TransitionUnavailable)?;
        harness
            .start_graph(
                context,
                self.definition.digest(),
                start.id().clone(),
                start.kind().durable_kind(),
                start.recovery(),
            )
            .await?;
        self.execute(
            harness,
            context,
            program,
            state,
            ResumeNode {
                node_id: start.id().clone(),
                previous_attempt: None,
            },
            false,
        )
        .await
    }

    pub async fn resume<P: RestartableGraphProgram>(
        &self,
        harness: &ExecutionHarness,
        recovered: RecoveredRun,
        program: &mut P,
    ) -> Result<(GraphRunSummary, P::WorkingState), GraphError> {
        let (mut context, durable, contract) = recovered.into_parts();
        if contract
            != (RecoveryContract::Graph {
                program_version: P::RECOVERY_VERSION,
                definition_digest: self.definition.digest(),
            })
        {
            return Err(GraphError::RecoveryMismatch);
        }
        let graph = durable.graph().ok_or(GraphError::RecoveryMismatch)?;
        if graph.definition_digest() != self.definition.digest() {
            return Err(GraphError::RecoveryMismatch);
        }
        let resume = if let Some(anchor) = graph.restart_anchor() {
            ResumeNode {
                node_id: anchor.node_id().clone(),
                previous_attempt: Some(anchor.attempt_id()),
            }
        } else {
            match graph.position() {
                DurableGraphPosition::Ready {
                    node_id, recovery, ..
                } if *recovery != GraphRecoveryMode::Never => ResumeNode {
                    node_id: node_id.clone(),
                    previous_attempt: None,
                },
                DurableGraphPosition::Running {
                    attempt_id,
                    node_id,
                    recovery,
                    ..
                } if *recovery != GraphRecoveryMode::Never => ResumeNode {
                    node_id: node_id.clone(),
                    previous_attempt: Some(*attempt_id),
                },
                _ => return Err(GraphError::RecoveryMismatch),
            }
        };
        let mut state = program.restore_working_state(&durable)?;
        let summary = self
            .execute(harness, &mut context, program, &mut state, resume, true)
            .await?;
        Ok((summary, state))
    }

    async fn execute<P: GraphProgram>(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        program: &mut P,
        state: &mut P::WorkingState,
        mut current: ResumeNode,
        mut restarting: bool,
    ) -> Result<GraphRunSummary, GraphError> {
        loop {
            let node = self
                .definition
                .node(&current.node_id)
                .ok_or(GraphError::TransitionUnavailable)?;
            let attempt = if restarting {
                restarting = false;
                harness
                    .restart_graph_node(
                        context,
                        current.previous_attempt,
                        node.id().clone(),
                        node.kind().durable_kind(),
                        node.recovery(),
                    )
                    .await?
            } else {
                harness
                    .enter_graph_node(
                        context,
                        node.id().clone(),
                        node.kind().durable_kind(),
                        node.recovery(),
                    )
                    .await?
            };

            let transition = match node.kind() {
                NodeKind::Complete => {
                    harness
                        .complete_graph(context, attempt, node.id().clone())
                        .await?;
                    return Ok(GraphRunSummary {
                        steps: context.usage().graph_steps(),
                        terminal: GraphTerminalOutcome::Complete,
                    });
                }
                NodeKind::Fail => {
                    harness
                        .fail_graph(
                            context,
                            attempt,
                            node.id().clone(),
                            GraphFailureKind::ExplicitTerminal,
                        )
                        .await?;
                    return Ok(GraphRunSummary {
                        steps: context.usage().graph_steps(),
                        terminal: GraphTerminalOutcome::Fail,
                    });
                }
                NodeKind::Retrieve => {
                    if let Err(error) = program
                        .retrieve(node.id(), state, RetrieveEffects::new(harness, context))
                        .await
                    {
                        self.fail_callback(harness, context, attempt, node.id().clone())
                            .await;
                        return Err(error.into());
                    }
                    GraphTransitionKey::Succeeded
                }
                NodeKind::Model => {
                    if let Err(error) = program
                        .model(node.id(), state, ModelEffects::new(harness, context))
                        .await
                    {
                        self.fail_callback(harness, context, attempt, node.id().clone())
                            .await;
                        return Err(error.into());
                    }
                    GraphTransitionKey::Succeeded
                }
                NodeKind::Action => {
                    if let Err(error) = program
                        .action(node.id(), state, ActionEffects::new(harness, context))
                        .await
                    {
                        self.fail_callback(harness, context, attempt, node.id().clone())
                            .await;
                        return Err(error.into());
                    }
                    GraphTransitionKey::Succeeded
                }
                NodeKind::Verify => {
                    match program.verify(node.id(), state, VerifyEffects::new()).await {
                        Ok(VerificationOutcome::Passed) => GraphTransitionKey::VerificationPassed,
                        Ok(VerificationOutcome::Failed) => GraphTransitionKey::VerificationFailed,
                        Err(error) => {
                            self.fail_callback(harness, context, attempt, node.id().clone())
                                .await;
                            return Err(error.into());
                        }
                    }
                }
                NodeKind::Decision { .. } => match program
                    .decide(node.id(), state, DecisionContext::new())
                    .await
                {
                    Ok(branch) => GraphTransitionKey::Branch(branch),
                    Err(error) => {
                        self.fail_callback(harness, context, attempt, node.id().clone())
                            .await;
                        return Err(error.into());
                    }
                },
            };
            let Some(next_id) = self.definition.transition(node.id(), &transition).cloned() else {
                self.fail_transition(harness, context, attempt, node.id().clone())
                    .await;
                return Err(GraphError::TransitionUnavailable);
            };
            let Some(next) = self.definition.node(&next_id) else {
                self.fail_transition(harness, context, attempt, node.id().clone())
                    .await;
                return Err(GraphError::TransitionUnavailable);
            };
            harness
                .complete_graph_node(
                    context,
                    attempt,
                    node.id().clone(),
                    transition,
                    next.id().clone(),
                    next.kind().durable_kind(),
                    next.recovery(),
                )
                .await?;
            current = ResumeNode {
                node_id: next_id,
                previous_attempt: None,
            };
        }
    }

    async fn fail_callback(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        attempt: GraphNodeAttemptId,
        node_id: GraphNodeId,
    ) {
        let _ = harness
            .fail_graph(context, attempt, node_id, GraphFailureKind::Callback)
            .await;
    }

    async fn fail_transition(
        &self,
        harness: &ExecutionHarness,
        context: &mut RunContext,
        attempt: GraphNodeAttemptId,
        node_id: GraphNodeId,
    ) {
        let _ = harness
            .fail_graph(
                context,
                attempt,
                node_id,
                GraphFailureKind::InvalidTransition,
            )
            .await;
    }
}

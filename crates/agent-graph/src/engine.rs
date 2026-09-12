use agent_core::{
    GraphFailureKind, GraphNodeAttemptId, GraphNodeId, GraphRecoveryMode, GraphTransitionKey,
};
use agent_harness::{
    DurableActionContext, DurableApprovalWait, DurableGraphPosition, DurableLocalWriteResume,
    ExecutionHarness, GraphDefinitionDigest, RecoveredRun, RecoveredWaitingRun, RecoveryContract,
    RunContext,
};

use crate::{
    ActionEffects, DecisionContext, GraphDefinition, GraphError, GraphProgram, ModelEffects,
    NodeKind, RestartableGraphProgram, RetrieveEffects, VerificationOutcome, VerifyEffects,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphTerminalOutcome {
    Complete,
    Fail,
    Waiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphRunSummary {
    steps: u32,
    terminal: GraphTerminalOutcome,
    waiting: Option<DurableApprovalWait>,
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

    #[must_use]
    pub const fn waiting(self) -> Option<DurableApprovalWait> {
        self.waiting
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

    pub async fn resume_waiting<P: RestartableGraphProgram>(
        &self,
        harness: &ExecutionHarness,
        recovered: RecoveredWaitingRun,
        wait_id: agent_core::DurableApprovalWaitId,
        program: &mut P,
    ) -> Result<(GraphRunSummary, P::WorkingState), GraphError> {
        if recovered.recovery_contract()
            != (RecoveryContract::Graph {
                program_version: P::RECOVERY_VERSION,
                definition_digest: self.definition.digest(),
            })
        {
            return Err(GraphError::RecoveryMismatch);
        }
        let durable = recovered.state().clone();
        let waiting_node = match durable
            .graph()
            .map(agent_harness::DurableGraphState::position)
        {
            Some(DurableGraphPosition::Waiting {
                node_id,
                wait_id: current,
                ..
            }) if *current == wait_id => node_id.clone(),
            _ => return Err(GraphError::RecoveryMismatch),
        };
        let mut state = program.restore_working_state(&durable)?;
        let resumed = harness
            .resume_durable_local_write(recovered, wait_id)
            .await?;
        let (mut context, attempt_id, node_id, transition) = match resumed {
            DurableLocalWriteResume::Executed {
                context,
                attempt_id,
                node_id,
                ..
            } => (context, attempt_id, node_id, GraphTransitionKey::Succeeded),
            DurableLocalWriteResume::Denied {
                context,
                attempt_id,
                node_id,
            } => (
                context,
                attempt_id,
                node_id,
                GraphTransitionKey::ApprovalDenied,
            ),
        };
        if node_id != waiting_node {
            return Err(GraphError::RecoveryMismatch);
        }
        let next_id = self
            .definition
            .transition(&node_id, &transition)
            .cloned()
            .ok_or(GraphError::TransitionUnavailable)?;
        let next = self
            .definition
            .node(&next_id)
            .ok_or(GraphError::TransitionUnavailable)?;
        harness
            .complete_graph_node(
                &mut context,
                attempt_id,
                node_id,
                transition,
                next.id().clone(),
                next.kind().durable_kind(),
                next.recovery(),
            )
            .await?;
        let summary = self
            .execute(
                harness,
                &mut context,
                program,
                &mut state,
                ResumeNode {
                    node_id: next_id,
                    previous_attempt: None,
                },
                false,
            )
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
                        waiting: None,
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
                        waiting: None,
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
                NodeKind::DurableLocalWriteAction => {
                    let program_version = match context.recovery_contract() {
                        RecoveryContract::Graph {
                            program_version,
                            definition_digest,
                        } if definition_digest == self.definition.digest() => program_version,
                        _ => return Err(GraphError::RecoveryMismatch),
                    };
                    let wait = match program
                        .durable_local_write_action(
                            node.id(),
                            state,
                            ActionEffects::new_durable(
                                harness,
                                context,
                                DurableActionContext {
                                    program_version,
                                    graph_digest: GraphDefinitionDigest::from_bytes(
                                        self.definition.digest(),
                                    ),
                                    node_id: node.id().clone(),
                                    attempt_id: attempt,
                                },
                            ),
                        )
                        .await
                    {
                        Ok(wait) => wait,
                        Err(error) => {
                            self.fail_callback(harness, context, attempt, node.id().clone())
                                .await;
                            return Err(error.into());
                        }
                    };
                    return Ok(GraphRunSummary {
                        steps: context.usage().graph_steps(),
                        terminal: GraphTerminalOutcome::Waiting,
                        waiting: Some(wait),
                    });
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

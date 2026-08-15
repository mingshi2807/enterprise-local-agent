use std::{collections::VecDeque, sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, BudgetDimension, CapabilityKind, LoopEventKind, LoopFailureKind, LoopPhase,
    ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole, RunBudget,
    RunFailureKind, RunId, RunOutcome, RunStatus, SessionId, ToolCall, ToolCallId, ToolDefinition,
    ToolInput, ToolName, ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, HarnessError, M0ReadOnlyPolicy,
    ModelPort, ModelPortError, RunCancellationHandle, RunContext, ToolPort, ToolRegistry,
    testing::{FailingAuditSink, FakeModelPort, FakeToolPort, InMemoryAuditSink},
};

use crate::{
    LoopEffects, LoopEngine, LoopError, LoopFuture, LoopPosition, LoopProgram, LoopState,
    LoopStepError, ReflectDecision, TerminalLoopDecision, VerificationResult,
};

const PROMPT_SENTINEL: &str = "m2-raw-prompt-sentinel";
const MODEL_SENTINEL: &str = "m2-raw-model-output-sentinel";
const TOOL_INPUT_SENTINEL: &str = "m2-raw-tool-input-sentinel";
const TOOL_OUTPUT_SENTINEL: &str = "m2-raw-tool-output-sentinel";

struct WorkingState {
    model_response: Option<ModelResponse>,
    tool_result: Option<ToolResult>,
}

impl WorkingState {
    const fn new() -> Self {
        Self {
            model_response: None,
            tool_result: None,
        }
    }
}

struct ScriptedProgram {
    phases: Vec<LoopPhase>,
    decisions: VecDeque<ReflectDecision>,
    verification: VerificationResult,
    request: ModelRequest,
    call: ToolCall,
    invoke_effects: bool,
    swallow_model_error: bool,
    fail_phase: Option<LoopPhase>,
    cancellation: Option<(LoopPhase, RunCancellationHandle)>,
    delay: Option<(LoopPhase, Duration)>,
}

impl ScriptedProgram {
    fn new(call: ToolCall, decisions: Vec<ReflectDecision>) -> Self {
        Self {
            phases: Vec::new(),
            decisions: decisions.into(),
            verification: VerificationResult::Passed,
            request: ModelRequest::new(vec![ModelMessage::new(ModelRole::User, PROMPT_SENTINEL)]),
            call,
            invoke_effects: true,
            swallow_model_error: false,
            fail_phase: None,
            cancellation: None,
            delay: None,
        }
    }

    fn without_effects(mut self) -> Self {
        self.invoke_effects = false;
        self
    }

    fn record_phase(&mut self, phase: LoopPhase) -> Result<(), LoopStepError> {
        self.phases.push(phase);
        if self.fail_phase == Some(phase) {
            Err(LoopStepError::Program)
        } else {
            Ok(())
        }
    }

    async fn phase_boundary_action(&self, phase: LoopPhase) {
        if let Some((cancel_phase, handle)) = &self.cancellation
            && *cancel_phase == phase
        {
            handle.request_cancel();
        }
        if let Some((delay_phase, duration)) = self.delay
            && delay_phase == phase
        {
            tokio::time::sleep(duration).await;
        }
    }
}

impl LoopProgram for ScriptedProgram {
    type WorkingState = WorkingState;

    fn observe<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let result = self.record_phase(LoopPhase::Observe);
        Box::pin(async move {
            self.phase_boundary_action(LoopPhase::Observe).await;
            result
        })
    }

    fn retrieve<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let result = self.record_phase(LoopPhase::Retrieve);
        Box::pin(async move {
            self.phase_boundary_action(LoopPhase::Retrieve).await;
            result
        })
    }

    fn plan<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let result = self.record_phase(LoopPhase::Plan);
        let request = self.request.clone();
        let invoke_effects = self.invoke_effects;
        let swallow_model_error = self.swallow_model_error;
        Box::pin(async move {
            self.phase_boundary_action(LoopPhase::Plan).await;
            result?;
            if invoke_effects {
                match effects.invoke_model(request).await {
                    Ok(response) => working_state.model_response = Some(response),
                    Err(_) if swallow_model_error => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        })
    }

    fn act<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let result = self.record_phase(LoopPhase::Act);
        let call = self.call.clone();
        let invoke_effects = self.invoke_effects;
        Box::pin(async move {
            self.phase_boundary_action(LoopPhase::Act).await;
            result?;
            if invoke_effects {
                working_state.tool_result = Some(effects.invoke_tool(call).await?);
            }
            Ok(())
        })
    }

    fn verify<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>> {
        let result = self.record_phase(LoopPhase::Verify);
        let verification = self.verification;
        Box::pin(async move {
            self.phase_boundary_action(LoopPhase::Verify).await;
            result?;
            Ok(verification)
        })
    }

    fn reflect<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _verification: VerificationResult,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<ReflectDecision, LoopStepError>> {
        let result = self.record_phase(LoopPhase::Reflect);
        let decision = self
            .decisions
            .pop_front()
            .unwrap_or(ReflectDecision::Complete);
        Box::pin(async move {
            self.phase_boundary_action(LoopPhase::Reflect).await;
            result?;
            Ok(decision)
        })
    }
}

struct RuntimeFixture {
    harness: ExecutionHarness,
    model: Arc<FakeModelPort>,
    tool: Arc<FakeToolPort>,
    call: ToolCall,
}

fn runtime(
    model_calls: u32,
    tool_calls: u32,
    iterations: u32,
    elapsed: Duration,
    model_results: Vec<Result<ModelResponse, ModelPortError>>,
    audit: Arc<dyn AuditSink>,
    policy: AuditFailurePolicy,
) -> RuntimeFixture {
    let model = Arc::new(FakeModelPort::scripted(model_results));
    let model_port: Arc<dyn ModelPort> = model.clone();
    let tool_name = ToolName::new("m2_lookup").expect("test tool name must be valid");
    let tool_definition = ToolDefinition::new(
        tool_name.clone(),
        "deterministic M2 read-only lookup",
        CapabilityKind::ReadOnly,
        ToolSchema::new(serde_json::json!({"type": "object"})).expect("test schema must be valid"),
    )
    .expect("test definition must be valid");
    let tool = Arc::new(FakeToolPort::succeeding_times(
        tool_definition,
        ToolOutput::new(serde_json::json!({"value": TOOL_OUTPUT_SENTINEL})),
        tool_calls.max(1) as usize,
    ));
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    let mut registry = ToolRegistry::new();
    registry
        .register(tool_port)
        .expect("test tool must register");
    let config = HarnessConfig::new(policy, Duration::from_secs(1))
        .expect("test harness config must be valid");
    let harness = ExecutionHarness::new(
        model_port,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        audit,
        config,
    );
    let call = ToolCall::new(
        ToolCallId::new(),
        tool_name,
        ToolInput::new(serde_json::json!({"value": TOOL_INPUT_SENTINEL})),
    );
    let _ = RunBudget::new(model_calls, tool_calls, iterations, elapsed)
        .expect("test budget must be valid");
    RuntimeFixture {
        harness,
        model,
        tool,
        call,
    }
}

fn context(model_calls: u32, tool_calls: u32, iterations: u32, elapsed: Duration) -> RunContext {
    let budget = RunBudget::new(model_calls, tool_calls, iterations, elapsed)
        .expect("test budget must be valid");
    RunContext::new(RunId::new(), SessionId::new(), budget)
}

fn response() -> ModelResponse {
    ModelResponse::new(vec![ModelOutputPart::Text(MODEL_SENTINEL.to_owned())], None)
}

fn expected_iteration() -> Vec<LoopPhase> {
    vec![
        LoopPhase::Observe,
        LoopPhase::Retrieve,
        LoopPhase::Plan,
        LoopPhase::Act,
        LoopPhase::Verify,
        LoopPhase::Reflect,
    ]
}

#[tokio::test]
async fn one_full_iteration_uses_harness_model_and_read_only_tool_then_completes() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let fixture = runtime(
        1,
        1,
        2,
        Duration::from_secs(30),
        vec![Ok(response())],
        audit,
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 1, 2, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(fixture.call.clone(), vec![ReflectDecision::Complete]);
    let mut working_state = WorkingState::new();

    let summary = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect("loop must complete");

    assert_eq!(program.phases, expected_iteration());
    assert_eq!(fixture.model.invocation_count(), 1);
    assert_eq!(fixture.tool.invocation_count(), 1);
    assert!(working_state.model_response.is_some());
    assert!(working_state.tool_result.is_some());
    assert_eq!(summary.reserved_iterations(), 1);
    assert_eq!(summary.completed_iterations(), 1);
    assert_eq!(summary.terminal_decision(), TerminalLoopDecision::Complete);
    assert_eq!(
        summary.final_status(),
        &RunStatus::Finished(RunOutcome::Completed)
    );
    assert_eq!(run.usage().model_calls(), 1);
    assert_eq!(run.usage().tool_calls(), 1);
    assert_eq!(run.usage().iterations(), 1);
}

#[tokio::test]
async fn reflect_continue_starts_exactly_one_new_ordered_iteration() {
    let fixture = runtime(
        2,
        2,
        2,
        Duration::from_secs(30),
        vec![Ok(response()), Ok(response())],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(2, 2, 2, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(
        fixture.call.clone(),
        vec![ReflectDecision::Continue, ReflectDecision::Complete],
    );
    let mut working_state = WorkingState::new();

    let summary = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect("second iteration must complete");

    let mut expected = expected_iteration();
    expected.extend(expected_iteration());
    assert_eq!(program.phases, expected);
    assert_eq!(run.usage().iterations(), 2);
    assert_eq!(summary.completed_iterations(), 2);
}

#[tokio::test]
async fn reflect_complete_does_not_reserve_another_iteration() {
    let fixture = runtime(
        0,
        0,
        3,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 3, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    let mut working_state = WorkingState::new();

    LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect("loop must complete");

    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(program.phases, expected_iteration());
}

#[tokio::test]
async fn reflect_fail_is_a_normal_typed_terminal_decision() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(
        fixture.call,
        vec![ReflectDecision::Fail {
            kind: LoopFailureKind::VerificationFailed,
        }],
    )
    .without_effects();
    program.verification = VerificationResult::Failed;
    let mut working_state = WorkingState::new();

    let summary = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect("typed fail is a normal loop result");

    let failure = RunFailureKind::Loop {
        kind: LoopFailureKind::VerificationFailed,
    };
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::Failed { kind: failure })
    );
    assert_eq!(
        summary.terminal_decision(),
        TerminalLoopDecision::Fail {
            kind: LoopFailureKind::VerificationFailed,
        }
    );
    assert!(audit.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::Loop {
            event: LoopEventKind::LoopFailed { kind, .. }
        } if *kind == failure
    )));
}

#[tokio::test]
async fn zero_iteration_budget_runs_no_phase_and_terminalizes_iterations() {
    let fixture = runtime(
        0,
        0,
        0,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 0, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(matches!(
        result,
        Err(LoopError::Harness(HarnessError::BudgetExceeded(_)))
    ));
    assert!(program.phases.is_empty());
    assert_eq!(run.usage().iterations(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Iterations,
        })
    );
}

#[tokio::test]
async fn continue_cannot_begin_after_iteration_budget_is_consumed() {
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Continue]).without_effects();
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(matches!(
        result,
        Err(LoopError::Harness(HarnessError::BudgetExceeded(_)))
    ));
    assert_eq!(program.phases, expected_iteration());
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Iterations,
        })
    );
}

#[tokio::test]
async fn started_iteration_remains_consumed_after_program_failure() {
    let fixture = runtime(
        0,
        0,
        2,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 2, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    program.fail_phase = Some(LoopPhase::Observe);
    let mut working_state = WorkingState::new();

    let error = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect_err("program failure must fail the run");

    assert!(matches!(
        error,
        LoopError::Program {
            phase: LoopPhase::Observe
        }
    ));
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::Failed {
            kind: RunFailureKind::Loop {
                kind: LoopFailureKind::Program {
                    phase: LoopPhase::Observe,
                },
            },
        })
    );
}

#[tokio::test]
async fn swallowed_terminal_harness_error_cannot_advance_after_plan() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]);
    program.swallow_model_error = true;
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(matches!(result, Err(LoopError::RunTerminal { .. })));
    assert_eq!(
        program.phases,
        vec![LoopPhase::Observe, LoopPhase::Retrieve, LoopPhase::Plan]
    );
    assert_eq!(fixture.model.invocation_count(), 0);
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ModelCalls,
        })
    );
    assert!(!audit.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::Loop {
            event: LoopEventKind::PhaseCompleted {
                phase: LoopPhase::Plan,
                ..
            }
        }
    )));
}

#[tokio::test]
async fn fail_closed_iteration_audit_consumes_slot_and_observe_never_runs() {
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(matches!(
        result,
        Err(LoopError::Harness(HarnessError::Audit { .. }))
    ));
    assert!(program.phases.is_empty());
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::Failed {
            kind: RunFailureKind::AuditUnavailable,
        })
    );
}

#[tokio::test]
async fn fail_open_iteration_audit_marks_degraded_and_allows_observe() {
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        Arc::new(FailingAuditSink::on_attempt(2)),
        AuditFailurePolicy::FailOpen,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    let mut working_state = WorkingState::new();

    LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect("fail-open audit must allow the loop");

    assert_eq!(program.phases.first(), Some(&LoopPhase::Observe));
    assert!(run.audit_degraded());
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Completed));
}

#[tokio::test]
async fn illegal_internal_transition_terminalizes_the_active_run() {
    let fixture = runtime(
        0,
        0,
        2,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 2, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut invalid_state = LoopState::new();
    invalid_state
        .begin_iteration(99)
        .expect("test state setup must succeed");
    assert!(matches!(
        invalid_state.position(),
        LoopPosition::PhaseReady(_)
    ));
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    let mut working_state = WorkingState::new();

    let error = LoopEngine::new()
        .run_with_state(
            &fixture.harness,
            &mut run,
            &mut program,
            &mut working_state,
            invalid_state,
        )
        .await
        .expect_err("illegal transition must be fatal");

    assert!(matches!(error, LoopError::Transition(_)));
    assert!(program.phases.is_empty());
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::Failed {
            kind: RunFailureKind::Loop {
                kind: LoopFailureKind::InvariantViolation,
            },
        })
    );
}

#[tokio::test]
async fn terminal_context_emits_no_additional_loop_metadata() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    fixture
        .harness
        .cancel_run(&mut run)
        .await
        .expect("run must cancel");
    let event_count = audit.events().len();
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(result.is_err());
    assert_eq!(audit.events().len(), event_count);
    assert!(program.phases.is_empty());
}

#[tokio::test]
async fn hard_model_budget_exhaustion_stops_before_act() {
    let fixture = runtime(
        0,
        1,
        1,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 1, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]);
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(result.is_err());
    assert_eq!(
        program.phases,
        vec![LoopPhase::Observe, LoopPhase::Retrieve, LoopPhase::Plan]
    );
    assert_eq!(fixture.model.invocation_count(), 0);
    assert_eq!(fixture.tool.invocation_count(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ModelCalls,
        })
    );
}

#[tokio::test]
async fn non_terminal_model_error_remains_typed_and_fails_without_retry() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let fixture = runtime(
        1,
        1,
        1,
        Duration::from_secs(30),
        vec![Err(ModelPortError::Rejected)],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 1, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]);
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(matches!(
        result,
        Err(LoopError::Harness(HarnessError::ModelPort(
            ModelPortError::Rejected
        )))
    ));
    assert_eq!(fixture.model.invocation_count(), 1);
    assert_eq!(fixture.tool.invocation_count(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::Failed {
            kind: RunFailureKind::Model,
        })
    );
    assert!(audit.events().iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::Loop {
            event: LoopEventKind::LoopFailed {
                kind: RunFailureKind::Model,
                ..
            }
        }
    )));
}

#[tokio::test]
async fn hard_tool_budget_exhaustion_stops_before_verify() {
    let fixture = runtime(
        1,
        0,
        1,
        Duration::from_secs(30),
        vec![Ok(response())],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 0, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]);
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(result.is_err());
    assert_eq!(
        program.phases,
        vec![
            LoopPhase::Observe,
            LoopPhase::Retrieve,
            LoopPhase::Plan,
            LoopPhase::Act,
        ]
    );
    assert_eq!(fixture.model.invocation_count(), 1);
    assert_eq!(fixture.tool.invocation_count(), 0);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ToolCalls,
        })
    );
}

#[tokio::test]
async fn cancellation_between_phases_is_terminal_and_consumes_iteration() {
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(30),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(30));
    let cancellation = fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    program.cancellation = Some((LoopPhase::Observe, cancellation));
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(result.is_err());
    assert_eq!(program.phases, vec![LoopPhase::Observe]);
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
}

#[tokio::test(start_paused = true)]
async fn elapsed_deadline_between_phases_is_terminal_and_consumes_iteration() {
    let fixture = runtime(
        0,
        0,
        1,
        Duration::from_secs(1),
        vec![],
        Arc::new(InMemoryAuditSink::new()),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(0, 0, 1, Duration::from_secs(1));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program =
        ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]).without_effects();
    program.delay = Some((LoopPhase::Observe, Duration::from_secs(2)));
    let mut working_state = WorkingState::new();

    let result = LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await;

    assert!(result.is_err());
    assert_eq!(program.phases, vec![LoopPhase::Observe]);
    assert_eq!(run.usage().iterations(), 1);
    assert_eq!(
        run.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Elapsed,
        })
    );
}

#[tokio::test]
async fn loop_and_event_ordering_is_deterministic_and_payload_free() {
    let audit = Arc::new(InMemoryAuditSink::new());
    let fixture = runtime(
        1,
        1,
        1,
        Duration::from_secs(30),
        vec![Ok(response())],
        audit.clone(),
        AuditFailurePolicy::FailClosed,
    );
    let mut run = context(1, 1, 1, Duration::from_secs(30));
    fixture
        .harness
        .start_run(&mut run)
        .await
        .expect("run must start");
    let mut program = ScriptedProgram::new(fixture.call, vec![ReflectDecision::Complete]);
    let mut working_state = WorkingState::new();

    LoopEngine::new()
        .run(&fixture.harness, &mut run, &mut program, &mut working_state)
        .await
        .expect("loop must complete");

    let events = audit.events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence().get())
            .collect::<Vec<_>>(),
        (0..events.len() as u64).collect::<Vec<_>>()
    );
    assert!(events.iter().all(|event| event.schema_version().get() == 5));
    assert!(matches!(
        events.last().map(agent_core::AgentEvent::kind),
        Some(AgentEventKind::RunFinished {
            outcome: RunOutcome::Completed,
        })
    ));
    let loop_events = events
        .iter()
        .filter_map(|event| match event.kind() {
            AgentEventKind::Loop { event } => Some(*event),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        loop_events,
        vec![
            LoopEventKind::IterationStarted {
                iteration: 1,
                usage: 1,
                limit: 1,
            },
            LoopEventKind::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Observe,
            },
            LoopEventKind::PhaseCompleted {
                iteration: 1,
                phase: LoopPhase::Observe,
            },
            LoopEventKind::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Retrieve,
            },
            LoopEventKind::PhaseCompleted {
                iteration: 1,
                phase: LoopPhase::Retrieve,
            },
            LoopEventKind::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Plan,
            },
            LoopEventKind::PhaseCompleted {
                iteration: 1,
                phase: LoopPhase::Plan,
            },
            LoopEventKind::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Act,
            },
            LoopEventKind::PhaseCompleted {
                iteration: 1,
                phase: LoopPhase::Act,
            },
            LoopEventKind::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Verify,
            },
            LoopEventKind::PhaseCompleted {
                iteration: 1,
                phase: LoopPhase::Verify,
            },
            LoopEventKind::PhaseEntered {
                iteration: 1,
                phase: LoopPhase::Reflect,
            },
            LoopEventKind::PhaseCompleted {
                iteration: 1,
                phase: LoopPhase::Reflect,
            },
            LoopEventKind::ReflectDecision {
                iteration: 1,
                decision: agent_core::LoopDecisionKind::Complete,
            },
            LoopEventKind::IterationCompleted { iteration: 1 },
            LoopEventKind::LoopCompleted {
                completed_iterations: 1,
            },
        ]
    );
    let serialized = serde_json::to_string(&events).expect("events must serialize");
    for sentinel in [
        PROMPT_SENTINEL,
        MODEL_SENTINEL,
        TOOL_INPUT_SENTINEL,
        TOOL_OUTPUT_SENTINEL,
    ] {
        assert!(!serialized.contains(sentinel));
    }
}

#[test]
fn production_sources_and_manifest_do_not_leak_direct_ports_or_runtime_dependencies() {
    let sources = concat!(
        include_str!("engine.rs"),
        include_str!("error.rs"),
        include_str!("program.rs"),
        include_str!("state.rs"),
    );
    for forbidden in [
        "dyn ModelPort",
        "dyn ToolPort",
        "AuditSink",
        "CancellationToken",
        "tokio::",
        "serde_json::Value",
    ] {
        assert!(
            !sources.contains(forbidden),
            "forbidden dependency: {forbidden}"
        );
    }

    let manifest = include_str!("../Cargo.toml");
    let production = manifest
        .split("[dev-dependencies]")
        .next()
        .expect("manifest must have a production section");
    for forbidden in ["tokio", "serde", "rig", "petgraph", "reqwest"] {
        assert!(
            !production.lines().any(|line| line.starts_with(forbidden)),
            "forbidden production dependency: {forbidden}"
        );
    }
}

use std::{env, sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, CapabilityKind, LoopEventKind, LoopFailureKind, ModelMessage, ModelRequest,
    ModelResponse, ModelRole, RunBudget, RunId, SessionId, ToolCall, ToolCallId, ToolDefinition,
    ToolInput, ToolName, ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy, ModelPort,
    RunContext, ToolPort, ToolRegistry,
    testing::{FakeToolPort, InMemoryAuditSink},
};
use agent_loop::{
    LoopEffects, LoopEngine, LoopFuture, LoopProgram, LoopStepError, ReflectDecision,
    VerificationResult,
};
use agent_provider_rig::{
    BearerCredential, OpenAiCompatibleConfig, ProviderLabel, RigModelAdapter,
    build_openai_compatible_model_port, testing::FakeRigModel,
};
use anyhow::{Context, bail};

const LIVE_MODE_FLAG: &str = "--live-openai-compatible";
const LIVE_BASE_URL_ENV: &str = "ELA_OPENAI_COMPAT_BASE_URL";
const LIVE_MODEL_ENV: &str = "ELA_OPENAI_COMPAT_MODEL";
const LIVE_API_KEY_ENV: &str = "ELA_OPENAI_COMPAT_API_KEY";
const LIVE_LABEL_ENV: &str = "ELA_OPENAI_COMPAT_LABEL";

enum CliMode {
    Deterministic,
    LiveOpenAiCompatible,
}

struct ModelComposition {
    port: Arc<dyn ModelPort>,
    provider_label: String,
}

struct DemoWorkingState {
    model_response: Option<ModelResponse>,
    tool_result: Option<ToolResult>,
}

struct DemoProgram {
    model_request: ModelRequest,
    tool_call: ToolCall,
}

impl LoopProgram for DemoProgram {
    type WorkingState = DemoWorkingState;

    fn observe<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn retrieve<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn plan<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let request = self.model_request.clone();
        Box::pin(async move {
            working_state.model_response = Some(effects.invoke_model(request).await?);
            Ok(())
        })
    }

    fn act<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        mut effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<(), LoopStepError>> {
        let call = self.tool_call.clone();
        Box::pin(async move {
            working_state.tool_result = Some(effects.invoke_tool(call).await?);
            Ok(())
        })
    }

    fn verify<'a>(
        &'a mut self,
        _iteration: u32,
        working_state: &'a mut Self::WorkingState,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>> {
        let has_text = working_state
            .model_response
            .as_ref()
            .is_some_and(|response| {
                response
                    .output()
                    .iter()
                    .any(|part| matches!(part, agent_core::ModelOutputPart::Text(text) if !text.is_empty()))
            });
        let result = if has_text
            && matches!(
                working_state.tool_result,
                Some(ToolResult::Succeeded { .. })
            ) {
            VerificationResult::Passed
        } else {
            VerificationResult::Failed
        };
        Box::pin(std::future::ready(Ok(result)))
    }

    fn reflect<'a>(
        &'a mut self,
        _iteration: u32,
        _working_state: &'a mut Self::WorkingState,
        verification: VerificationResult,
        _effects: LoopEffects<'a>,
    ) -> LoopFuture<'a, Result<ReflectDecision, LoopStepError>> {
        let decision = match verification {
            VerificationResult::Passed => ReflectDecision::Complete,
            VerificationResult::Failed => ReflectDecision::Fail {
                kind: LoopFailureKind::VerificationFailed,
            },
        };
        Box::pin(std::future::ready(Ok(decision)))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mode = parse_mode()?;
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing: {error}"))?;

    let budget = RunBudget::new(1, 1, 1, Duration::from_secs(5))
        .context("failed to construct the M4 run budget")?;
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);

    let model = compose_model(mode)?;

    let tool_call_id = ToolCallId::new();
    let tool_name = ToolName::new("local_lookup")?;
    let definition = ToolDefinition::new(
        tool_name.clone(),
        "deterministic read-only lookup",
        CapabilityKind::ReadOnly,
        ToolSchema::new(serde_json::json!({"type": "object"}))?,
    )?;
    let tool = Arc::new(FakeToolPort::scripted(
        definition,
        vec![Ok(ToolResult::Succeeded {
            call_id: tool_call_id,
            output: ToolOutput::new(serde_json::json!({"status": "available"})),
        })],
    ));
    let tool_port: Arc<dyn ToolPort> = tool;
    let mut tools = ToolRegistry::new();
    tools.register(tool_port)?;

    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let config = HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))?;
    let harness = ExecutionHarness::new(
        model.port,
        tools,
        Arc::new(M0ReadOnlyPolicy),
        audit_sink,
        config,
    );

    let mut program = DemoProgram {
        model_request: ModelRequest::new(vec![ModelMessage::new(
            ModelRole::User,
            "sentinel prompt stays outside audit events",
        )]),
        tool_call: ToolCall::new(
            tool_call_id,
            tool_name,
            ToolInput::new(serde_json::json!({"sentinel": "tool input"})),
        ),
    };
    let mut working_state = DemoWorkingState {
        model_response: None,
        tool_result: None,
    };

    let _cancellation = harness.start_run(&mut context).await?;
    LoopEngine::new()
        .run(&harness, &mut context, &mut program, &mut working_state)
        .await?;

    println!("RunId: {}", context.run_id());
    println!("Provider: {}", model.provider_label);
    println!("Run start");
    for event in audit.events() {
        let AgentEventKind::Loop { event } = event.kind() else {
            continue;
        };
        match event {
            LoopEventKind::IterationStarted { iteration, .. } => {
                println!("Iteration {iteration} started");
            }
            LoopEventKind::PhaseEntered { iteration, phase } => {
                println!("Iteration {iteration}: {phase:?} entered");
            }
            LoopEventKind::PhaseCompleted { iteration, phase } => {
                println!("Iteration {iteration}: {phase:?} completed");
            }
            LoopEventKind::ReflectDecision {
                iteration,
                decision,
            } => println!("Iteration {iteration}: Reflect decision {decision:?}"),
            LoopEventKind::IterationCompleted { iteration } => {
                println!("Iteration {iteration} completed");
            }
            LoopEventKind::LoopCompleted {
                completed_iterations,
            } => println!("Loop completed after {completed_iterations} iteration(s)"),
            LoopEventKind::LoopFailed { iteration, kind } => {
                println!("Loop failed in iteration {iteration}: {kind:?}");
            }
        }
    }
    println!("Final status: {:?}", context.status());
    println!("ModelCalls usage: {}", context.usage().model_calls());
    println!("ToolCalls usage: {}", context.usage().tool_calls());
    println!("Iterations usage: {}", context.usage().iterations());
    println!("Audit degraded: {}", context.audit_degraded());

    Ok(())
}

fn parse_mode() -> anyhow::Result<CliMode> {
    let mut arguments = env::args().skip(1);
    match (arguments.next(), arguments.next()) {
        (None, None) => Ok(CliMode::Deterministic),
        (Some(flag), None) if flag == LIVE_MODE_FLAG => Ok(CliMode::LiveOpenAiCompatible),
        _ => bail!("usage: agent-cli [{LIVE_MODE_FLAG}]"),
    }
}

fn compose_model(mode: CliMode) -> anyhow::Result<ModelComposition> {
    match mode {
        CliMode::Deterministic => {
            let port: Arc<dyn ModelPort> = Arc::new(RigModelAdapter::new(
                FakeRigModel::scripted_text("deterministic fake response"),
            ));
            Ok(ModelComposition {
                port,
                provider_label: "deterministic-rig-fake".to_owned(),
            })
        }
        CliMode::LiveOpenAiCompatible => compose_live_model(),
    }
}

fn compose_live_model() -> anyhow::Result<ModelComposition> {
    let base_url = required_live_environment(LIVE_BASE_URL_ENV)?;
    let model_identifier = required_live_environment(LIVE_MODEL_ENV)?;
    let credential = BearerCredential::new(required_live_environment(LIVE_API_KEY_ENV)?)
        .context("live OpenAI-compatible credential is invalid")?;

    let configured_label = optional_live_environment(LIVE_LABEL_ENV)?
        .map(ProviderLabel::new)
        .transpose()
        .context("live OpenAI-compatible provider label is invalid")?;
    let provider_label = configured_label
        .as_ref()
        .map_or("openai-compatible", ProviderLabel::as_str)
        .to_owned();

    let mut config = OpenAiCompatibleConfig::new(base_url, model_identifier, credential)
        .context("live OpenAI-compatible provider configuration is invalid")?;
    if let Some(label) = configured_label {
        config = config.with_provider_label(label);
    }
    let port = build_openai_compatible_model_port(config)
        .context("live OpenAI-compatible model client could not be constructed")?;

    Ok(ModelComposition {
        port,
        provider_label,
    })
}

fn required_live_environment(name: &'static str) -> anyhow::Result<String> {
    env::var(name)
        .map_err(|_| anyhow::anyhow!("required live configuration variable {name} is missing"))
}

fn optional_live_environment(name: &'static str) -> anyhow::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(anyhow::anyhow!(
            "live configuration variable {name} is invalid"
        )),
    }
}

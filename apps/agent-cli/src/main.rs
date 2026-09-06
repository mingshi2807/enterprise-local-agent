mod action_program;

use std::{env, sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, CapabilityKind, LoopEventKind, ModelMessage, ModelRequest, ModelRole,
    RunBudget, RunId, SessionId, ToolDefinition, ToolName, ToolOutput, ToolSchema,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ContainedToolPort, ExecutionHarness, HarnessConfig,
    M0ReadOnlyPolicy, M6ApprovalPolicy, ModelPort, RunContext, ToolPort, ToolRegistry,
    testing::{FakeContainedToolPort, FakeToolPort, InMemoryAuditSink, ScriptedApprovalPort},
};
use agent_loop::LoopEngine;
use agent_provider_rig::{
    BearerCredential, OpenAiCompatibleConfig, ProviderLabel, RigModelAdapter,
    build_openai_compatible_model_port, testing::FakeRigModel,
};
use anyhow::{Context, bail};

use crate::action_program::{ActionProgram, ActionWorkingState};

const LIVE_MODE_FLAG: &str = "--live-openai-compatible";
const LOCAL_WRITE_DEMO_FLAG: &str = "--demo-local-write-fake-containment";
const LIVE_BASE_URL_ENV: &str = "ELA_OPENAI_COMPAT_BASE_URL";
const LIVE_MODEL_ENV: &str = "ELA_OPENAI_COMPAT_MODEL";
const LIVE_API_KEY_ENV: &str = "ELA_OPENAI_COMPAT_API_KEY";
const LIVE_LABEL_ENV: &str = "ELA_OPENAI_COMPAT_LABEL";

#[derive(Clone, Copy)]
enum CliMode {
    Deterministic,
    LiveOpenAiCompatible,
    LocalWriteFakeContainment,
}

struct ModelComposition {
    port: Arc<dyn ModelPort>,
    provider_label: String,
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

    let budget = match mode {
        CliMode::LocalWriteFakeContainment => RunBudget::new(1, 1, 1, Duration::from_secs(5))
            .context("failed to construct the M6 run budget")?
            .with_max_approval_requests(1),
        CliMode::Deterministic | CliMode::LiveOpenAiCompatible => {
            RunBudget::new(1, 1, 1, Duration::from_secs(5))
                .context("failed to construct the M4 run budget")?
        }
    };
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);

    let model = compose_model(mode)?;

    let local_write_demo = matches!(mode, CliMode::LocalWriteFakeContainment);
    let tool_name = ToolName::new(if local_write_demo {
        "write_demo_note"
    } else {
        "get_agent_capabilities"
    })?;
    let definition = ToolDefinition::new(
        tool_name.clone(),
        if local_write_demo {
            "fake contained local-write demo tool"
        } else {
            "report the agent's deterministic capabilities"
        },
        if local_write_demo {
            CapabilityKind::LocalWrite
        } else {
            CapabilityKind::ReadOnly
        },
        ToolSchema::new(serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }))?,
    )?;
    let mut tools = ToolRegistry::new();
    if local_write_demo {
        let tool = Arc::new(FakeContainedToolPort::succeeding(
            definition,
            ToolOutput::new(serde_json::json!({"status": "fake-contained"})),
        ));
        let tool_port: Arc<dyn ContainedToolPort> = tool;
        tools.register_contained(tool_port)?;
    } else {
        let tool = Arc::new(FakeToolPort::succeeding(
            definition,
            ToolOutput::new(serde_json::json!({"status": "available"})),
        ));
        let tool_port: Arc<dyn ToolPort> = tool;
        tools.register(tool_port)?;
    }

    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let config = HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))?;
    let harness = ExecutionHarness::new(
        model.port,
        tools,
        if local_write_demo {
            Arc::new(M6ApprovalPolicy)
        } else {
            Arc::new(M0ReadOnlyPolicy)
        },
        audit_sink,
        config,
    );
    let harness = if local_write_demo {
        harness.with_approval_port(Arc::new(ScriptedApprovalPort::approve_all()))
    } else {
        harness
    };

    let prompt = if local_write_demo {
        "Return only the exact JSON action envelope for write_demo_note with an empty arguments object."
    } else {
        "Return only the exact JSON action envelope for get_agent_capabilities with an empty arguments object."
    };
    let mut program = ActionProgram::new(ModelRequest::new(vec![ModelMessage::new(
        ModelRole::User,
        prompt,
    )]));
    let mut working_state = ActionWorkingState::new();

    let _cancellation = harness.start_run(&mut context).await?;
    LoopEngine::new()
        .run(&harness, &mut context, &mut program, &mut working_state)
        .await?;

    println!("RunId: {}", context.run_id());
    println!("Provider: {}", model.provider_label);
    if local_write_demo {
        println!("Approval: scripted fake");
        println!("Containment: test fake — NO OS isolation");
    }
    println!("Run start");
    for event in audit.events() {
        match event.kind() {
            AgentEventKind::ModelInvocationStarted { model_call_id, .. } => {
                println!("ModelCallId: {model_call_id}");
            }
            AgentEventKind::ActionProposed {
                action_proposal_id,
                tool_name,
                ..
            } => println!("ActionProposalId: {action_proposal_id}; tool: {tool_name}"),
            AgentEventKind::ActionExecutionBound {
                action_proposal_id,
                tool_call_id,
            } => {
                println!("ActionProposalId: {action_proposal_id}; ToolCallId: {tool_call_id} bound")
            }
            AgentEventKind::ApprovalRequested {
                approval_request_id,
                tool_call_id,
                tool_name,
                capability,
                ..
            } => println!(
                "ApprovalRequestId: {approval_request_id}; ToolCallId: {tool_call_id}; tool: {tool_name}; capability: {capability:?}"
            ),
            AgentEventKind::ApprovalGranted {
                approval_request_id,
            } => println!("ApprovalRequestId: {approval_request_id} granted"),
            AgentEventKind::ApprovalDenied {
                approval_request_id,
            } => println!("ApprovalRequestId: {approval_request_id} denied"),
            AgentEventKind::ApprovalFailed {
                approval_request_id,
                kind,
            } => println!("ApprovalRequestId: {approval_request_id} failed: {kind:?}"),
            AgentEventKind::ContainmentFailed {
                tool_call_id, kind, ..
            } => println!("ToolCallId: {tool_call_id}; containment failed: {kind:?}"),
            AgentEventKind::Loop { event } => match event {
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
            },
            AgentEventKind::RunStarted { .. }
            | AgentEventKind::RunFinished { .. }
            | AgentEventKind::AuditDegraded
            | AgentEventKind::ModelInvocationCompleted { .. }
            | AgentEventKind::ModelInvocationFailed { .. }
            | AgentEventKind::ActionValidated { .. }
            | AgentEventKind::ActionRejected { .. }
            | AgentEventKind::ToolInvocationStarted { .. }
            | AgentEventKind::ToolInvocationCompleted { .. }
            | AgentEventKind::ToolInvocationDomainFailed { .. }
            | AgentEventKind::ToolInvocationAdapterFailed { .. }
            | AgentEventKind::ToolPolicyDenied { .. }
            | AgentEventKind::KnowledgeRetrievalStarted { .. }
            | AgentEventKind::KnowledgeRetrievalRestarted { .. }
            | AgentEventKind::KnowledgeRetrievalCompleted { .. }
            | AgentEventKind::KnowledgeRetrievalFailed { .. }
            | AgentEventKind::ModelGroundingBound { .. }
            | AgentEventKind::Graph { .. } => {}
        }
    }
    println!("Final status: {:?}", context.status());
    println!("ModelCalls usage: {}", context.usage().model_calls());
    println!("ToolCalls usage: {}", context.usage().tool_calls());
    println!("Iterations usage: {}", context.usage().iterations());
    println!(
        "ApprovalRequests usage: {}",
        context.usage().approval_requests()
    );
    println!("Audit degraded: {}", context.audit_degraded());

    Ok(())
}

fn parse_mode() -> anyhow::Result<CliMode> {
    let mut arguments = env::args().skip(1);
    match (arguments.next(), arguments.next()) {
        (None, None) => Ok(CliMode::Deterministic),
        (Some(flag), None) if flag == LIVE_MODE_FLAG => Ok(CliMode::LiveOpenAiCompatible),
        (Some(flag), None) if flag == LOCAL_WRITE_DEMO_FLAG => {
            Ok(CliMode::LocalWriteFakeContainment)
        }
        _ => bail!("usage: agent-cli [{LIVE_MODE_FLAG}|{LOCAL_WRITE_DEMO_FLAG}]"),
    }
}

fn compose_model(mode: CliMode) -> anyhow::Result<ModelComposition> {
    match mode {
        CliMode::Deterministic => {
            let port: Arc<dyn ModelPort> =
                Arc::new(RigModelAdapter::new(FakeRigModel::scripted_text(
                    r#"{"action":{"tool":"get_agent_capabilities","arguments":{}}}"#,
                )));
            Ok(ModelComposition {
                port,
                provider_label: "deterministic-rig-fake".to_owned(),
            })
        }
        CliMode::LocalWriteFakeContainment => {
            let port: Arc<dyn ModelPort> =
                Arc::new(RigModelAdapter::new(FakeRigModel::scripted_text(
                    r#"{"action":{"tool":"write_demo_note","arguments":{}}}"#,
                )));
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

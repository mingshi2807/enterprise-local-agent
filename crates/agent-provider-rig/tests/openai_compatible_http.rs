mod support;

use std::{sync::Arc, time::Duration};

use agent_core::{
    AgentEventKind, CapabilityKind, LoopFailureKind, ModelMessage, ModelOutputPart, ModelRequest,
    ModelResponse, ModelRole, RunBudget, RunId, RunOutcome, RunStatus, SessionId, TokenUsage,
    ToolCall, ToolCallId, ToolDefinition, ToolInput, ToolName, ToolOutput, ToolResult, ToolSchema,
};
use agent_harness::{
    AuditFailurePolicy, AuditSink, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy,
    ModelPortError, RunContext, ToolPort, ToolRegistry,
    testing::{FakeToolPort, InMemoryAuditSink},
};
use agent_loop::{
    LoopEffects, LoopEngine, LoopFuture, LoopProgram, LoopStepError, ReflectDecision,
    TerminalLoopDecision, VerificationResult,
};
use agent_provider_rig::{
    BearerCredential, OpenAiCompatibleConfig, ProviderLabel, build_openai_compatible_model_port,
    probe_openai_compatible,
};
use rig_core::serde_json::{self, Value, json};

use support::{CapturedRequest, ScriptedResponse, TestOpenAiServer};

const TOKEN_SENTINEL: &str = "m4-secret-token-sentinel";
const PROMPT_SENTINEL: &str = "m4-prompt-sentinel";
const RESPONSE_SENTINEL: &str = "m4-response-sentinel";
const MODEL_IDENTIFIER: &str = "gateway-model-alias@latest";

fn success_response() -> String {
    json!({
        "id": "test-completion-id",
        "object": "chat.completion",
        "created": 1,
        "model": "resolved-provider-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": RESPONSE_SENTINEL
            },
            "logprobs": null,
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18
        }
    })
    .to_string()
}

fn config(base_url: &str) -> OpenAiCompatibleConfig {
    OpenAiCompatibleConfig::new(
        base_url,
        MODEL_IDENTIFIER,
        BearerCredential::new(TOKEN_SENTINEL).expect("test credential must be valid"),
    )
    .expect("test provider configuration must be valid")
    .with_provider_label(ProviderLabel::new("deterministic-http").expect("label must be valid"))
}

fn ordered_request() -> ModelRequest {
    ModelRequest::new(vec![
        ModelMessage::new(ModelRole::System, "system message"),
        ModelMessage::new(ModelRole::User, PROMPT_SENTINEL),
        ModelMessage::new(ModelRole::Assistant, "assistant history"),
    ])
}

fn user_request() -> ModelRequest {
    ModelRequest::new(vec![ModelMessage::new(ModelRole::User, PROMPT_SENTINEL)])
}

fn response_text(response: &ModelResponse) -> Option<&str> {
    response.output().iter().find_map(|part| match part {
        ModelOutputPart::Text(text) => Some(text.as_str()),
        ModelOutputPart::ToolCall(_) => None,
    })
}

fn expect_model_error(
    result: Result<ModelResponse, ModelPortError>,
    message: &str,
) -> ModelPortError {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

fn message_text(message: &Value) -> Option<&str> {
    let content = message.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text);
    }
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Some(text);
    }
    content
        .as_array()?
        .iter()
        .find_map(|part| part.get("text").and_then(Value::as_str))
}

fn assert_standard_request(request: &CapturedRequest) {
    assert_eq!(request.method(), "POST");
    assert_eq!(request.path(), "/v1/chat/completions");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer m4-secret-token-sentinel")
    );

    let body = request.json_body().expect("request body must be JSON");
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some(MODEL_IDENTIFIER)
    );
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

#[tokio::test]
async fn both_api_root_slash_forms_reach_chat_completions() {
    for trailing_slash in [false, true] {
        let server = TestOpenAiServer::start(vec![ScriptedResponse::json(200, success_response())])
            .await
            .expect("test server must start");
        let base_url = if trailing_slash {
            server.base_url_with_trailing_slash()
        } else {
            server.base_url().to_owned()
        };
        let port =
            build_openai_compatible_model_port(config(&base_url)).expect("model port must build");

        let response = port
            .invoke(user_request())
            .await
            .expect("completion must succeed");
        assert_eq!(response_text(&response), Some(RESPONSE_SENTINEL));

        let requests = server.finish().await.expect("test server must finish");
        assert_eq!(requests.len(), 1);
        assert_standard_request(&requests[0]);
    }
}

#[tokio::test]
async fn explicit_loopback_no_auth_sends_no_authorization_header() {
    let server = TestOpenAiServer::start(vec![ScriptedResponse::json(200, success_response())])
        .await
        .expect("test server must start");
    let config = OpenAiCompatibleConfig::new_no_auth_loopback(server.base_url(), MODEL_IDENTIFIER)
        .expect("loopback no-auth configuration");
    let port = build_openai_compatible_model_port(config).expect("model port must build");
    let response = port.invoke(user_request()).await.expect("completion");
    assert_eq!(response_text(&response), Some(RESPONSE_SENTINEL));
    let requests = server.finish().await.expect("server finish");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path(), "/v1/chat/completions");
    assert_eq!(requests[0].header("authorization"), None);
}

#[tokio::test]
async fn provider_readiness_uses_bounded_models_endpoint() {
    let server = TestOpenAiServer::start(vec![ScriptedResponse::json(
        200,
        json!({"object":"list","data":[{"id":MODEL_IDENTIFIER}]}).to_string(),
    )])
    .await
    .expect("test server must start");
    let config = config(server.base_url());
    probe_openai_compatible(&config).await.expect("readiness");
    let requests = server.finish().await.expect("server finish");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method(), "GET");
    assert_eq!(requests[0].path(), "/v1/models");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer m4-secret-token-sentinel")
    );
}

#[tokio::test]
async fn wire_request_preserves_model_auth_and_order_and_maps_text_usage() {
    let server = TestOpenAiServer::start(vec![ScriptedResponse::json(200, success_response())])
        .await
        .expect("test server must start");
    let port = build_openai_compatible_model_port(config(server.base_url()))
        .expect("model port must build");

    let response = port
        .invoke(ordered_request())
        .await
        .expect("completion must succeed");

    assert_eq!(response_text(&response), Some(RESPONSE_SENTINEL));
    assert_eq!(
        response.token_usage(),
        Some(TokenUsage::new(Some(11), Some(7)))
    );

    let requests = server.finish().await.expect("test server must finish");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_standard_request(request);
    let body = request.json_body().expect("request body must be JSON");
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .expect("request messages must be an array");
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[0].get("role").and_then(Value::as_str),
        Some("system")
    );
    assert_eq!(message_text(&messages[0]), Some("system message"));
    assert_eq!(
        messages[1].get("role").and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(message_text(&messages[1]), Some(PROMPT_SENTINEL));
    assert_eq!(
        messages[2].get("role").and_then(Value::as_str),
        Some("assistant")
    );
    assert_eq!(message_text(&messages[2]), Some("assistant history"));
}

#[tokio::test]
async fn provider_http_statuses_map_to_stable_errors() {
    for (status, expected) in [
        (401, ModelPortError::Rejected),
        (403, ModelPortError::Rejected),
        (408, ModelPortError::Unavailable),
        (429, ModelPortError::Unavailable),
        (500, ModelPortError::Unavailable),
        (502, ModelPortError::Unavailable),
        (503, ModelPortError::Unavailable),
    ] {
        let body = format!("{{\"error\":\"{TOKEN_SENTINEL}-{PROMPT_SENTINEL}\"}}");
        let server = TestOpenAiServer::start(vec![ScriptedResponse::json(status, body)])
            .await
            .expect("test server must start");
        let port = build_openai_compatible_model_port(config(server.base_url()))
            .expect("model port must build");

        let error = expect_model_error(
            port.invoke(user_request()).await,
            "provider status must fail",
        );
        let rendered = format!("{error:?} {error}");
        assert_eq!(error, expected);
        for sentinel in [TOKEN_SENTINEL, PROMPT_SENTINEL] {
            assert!(!rendered.contains(sentinel));
        }

        let requests = server.finish().await.expect("test server must finish");
        assert_eq!(requests.len(), 1);
    }
}

#[tokio::test]
async fn malformed_success_responses_fail_without_leaking_payloads() {
    for body in [
        format!("not-json-{RESPONSE_SENTINEL}"),
        json!({"unexpected": RESPONSE_SENTINEL}).to_string(),
    ] {
        let server = TestOpenAiServer::start(vec![ScriptedResponse::json(200, body)])
            .await
            .expect("test server must start");
        let port = build_openai_compatible_model_port(config(server.base_url()))
            .expect("model port must build");

        let error = expect_model_error(
            port.invoke(user_request()).await,
            "malformed success response must fail",
        );
        let rendered = format!("{error:?} {error}");
        assert_eq!(error, ModelPortError::Failed);
        assert!(!rendered.contains(RESPONSE_SENTINEL));
        assert!(!rendered.contains(TOKEN_SENTINEL));

        let requests = server.finish().await.expect("test server must finish");
        assert_eq!(requests.len(), 1);
    }
}

struct HttpWorkingState {
    model_response: Option<ModelResponse>,
    tool_result: Option<ToolResult>,
}

struct HttpLoopProgram {
    model_request: ModelRequest,
    tool_call: ToolCall,
}

impl LoopProgram for HttpLoopProgram {
    type WorkingState = HttpWorkingState;

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
                    .any(|part| matches!(part, ModelOutputPart::Text(text) if !text.is_empty()))
            });
        let tool_succeeded = matches!(
            working_state.tool_result,
            Some(ToolResult::Succeeded { .. })
        );
        let verification = if has_text && tool_succeeded {
            VerificationResult::Passed
        } else {
            VerificationResult::Failed
        };
        Box::pin(std::future::ready(Ok(verification)))
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

#[tokio::test]
async fn full_loop_completes_through_real_http_transport_with_metadata_only_events() {
    let server = TestOpenAiServer::start(vec![ScriptedResponse::json(200, success_response())])
        .await
        .expect("test server must start");
    let model = build_openai_compatible_model_port(config(server.base_url()))
        .expect("model port must build");

    let tool_call_id = ToolCallId::new();
    let tool_name = ToolName::new("m4_read_only_lookup").expect("tool name must be valid");
    let tool_definition = ToolDefinition::new(
        tool_name.clone(),
        "deterministic M4 read-only tool",
        CapabilityKind::ReadOnly,
        ToolSchema::new(json!({"type": "object"})).expect("tool schema must be valid"),
    )
    .expect("tool definition must be valid");
    let tool: Arc<dyn ToolPort> = Arc::new(FakeToolPort::scripted(
        tool_definition,
        vec![Ok(ToolResult::Succeeded {
            call_id: tool_call_id,
            output: ToolOutput::new(json!({"status": "available"})),
        })],
    ));
    let mut registry = ToolRegistry::new();
    registry.register(tool).expect("tool must register");

    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let harness = ExecutionHarness::new(
        model,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        audit_sink,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("harness config must be valid"),
    );
    let budget = RunBudget::new(1, 1, 1, Duration::from_secs(5)).expect("run budget must be valid");
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);
    let mut program = HttpLoopProgram {
        model_request: user_request(),
        tool_call: ToolCall::new(
            tool_call_id,
            tool_name,
            ToolInput::new(json!({"lookup": "safe"})),
        ),
    };
    let mut working_state = HttpWorkingState {
        model_response: None,
        tool_result: None,
    };

    let _cancellation = harness
        .start_run(&mut context)
        .await
        .expect("run must start");
    let summary = LoopEngine::new()
        .run(&harness, &mut context, &mut program, &mut working_state)
        .await
        .expect("loop must complete");

    assert_eq!(summary.terminal_decision(), TerminalLoopDecision::Complete);
    assert_eq!(
        context.status(),
        &RunStatus::Finished(RunOutcome::Completed)
    );
    assert_eq!(context.usage().model_calls(), 1);
    assert_eq!(context.usage().tool_calls(), 1);
    assert_eq!(context.usage().iterations(), 1);
    assert_eq!(
        response_text(
            working_state
                .model_response
                .as_ref()
                .expect("model response must be stored")
        ),
        Some(RESPONSE_SENTINEL)
    );

    let serialized_events = serde_json::to_string(&audit.events()).expect("events must serialize");
    for sentinel in [TOKEN_SENTINEL, PROMPT_SENTINEL, RESPONSE_SENTINEL] {
        assert!(!serialized_events.contains(sentinel));
    }
    assert!(audit.events().iter().any(|event| {
        matches!(
            event.kind(),
            AgentEventKind::ModelInvocationCompleted { .. }
        )
    }));

    let requests = server.finish().await.expect("test server must finish");
    assert_eq!(requests.len(), 1);
    assert_standard_request(&requests[0]);
}

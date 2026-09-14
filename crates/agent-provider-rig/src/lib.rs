//! Rig completion adapter for the enterprise harness model port.
//!
//! This crate intentionally constructs `rig_core::completion::CompletionRequest`
//! directly. That makes Rig request-shape changes a compile-time tripwire during
//! upgrades instead of silently accepting builder defaults.
//!
//! `record_telemetry_content = false` is a security invariant for this adapter:
//! request, response, tool, and provider-error payloads must stay out of
//! telemetry and audit events.
//!
//! The adapter performs one completion await. Dropping that future on harness
//! cancellation or deadline is local cooperative cancellation only; it does not
//! prove a remote provider stopped work.
//!
//! Future real-provider adapters need an explicit retry and idempotency review
//! before adding retry behavior around completion calls.

mod openai_compatible;

pub use openai_compatible::{
    BearerCredential, BearerCredentialError, OpenAiCompatibleAuth, OpenAiCompatibleBuildError,
    OpenAiCompatibleConfig, OpenAiCompatibleConfigError, OpenAiCompatibleReadinessError,
    ProviderLabel, ProviderLabelError, build_openai_compatible_model_port, probe_openai_compatible,
};

use agent_core::{
    ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole, TokenUsage,
};
use agent_harness::{ModelPort, ModelPortError, PortFuture};
use rig_core::{
    OneOrMany,
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest,
        CompletionResponse as RigCompletionResponse, Usage,
    },
    message::{Message as RigMessage, UserContent},
};

pub struct RigModelAdapter<M> {
    model: M,
}

impl<M> RigModelAdapter<M> {
    #[must_use]
    pub const fn new(model: M) -> Self {
        Self { model }
    }
}

impl<M> ModelPort for RigModelAdapter<M>
where
    M: CompletionModel + Send + Sync,
{
    fn invoke<'a>(
        &'a self,
        request: ModelRequest,
    ) -> PortFuture<'a, Result<ModelResponse, ModelPortError>> {
        Box::pin(async move {
            let request = to_rig_request(&request)?;
            self.model
                .completion(request)
                .await
                .map_err(map_completion_error)
                .and_then(from_rig_response)
        })
    }
}

fn to_rig_request(request: &ModelRequest) -> Result<CompletionRequest, ModelPortError> {
    let messages = request
        .messages()
        .iter()
        .map(to_rig_message)
        .collect::<Vec<_>>();
    let chat_history = OneOrMany::many(messages).map_err(|_| ModelPortError::Rejected)?;

    Ok(CompletionRequest {
        model: None,
        preamble: None,
        chat_history,
        documents: Vec::new(),
        tools: Vec::new(),
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        additional_params: None,
        output_schema: None,
        record_telemetry_content: false,
    })
}

fn to_rig_message(message: &ModelMessage) -> RigMessage {
    match message.role() {
        ModelRole::System => RigMessage::System {
            content: message.content().to_owned(),
        },
        ModelRole::User => RigMessage::User {
            content: OneOrMany::one(UserContent::text(message.content().to_owned())),
        },
        ModelRole::Assistant => RigMessage::Assistant {
            id: None,
            content: OneOrMany::one(AssistantContent::text(message.content().to_owned())),
        },
    }
}

fn from_rig_response<T>(
    response: RigCompletionResponse<T>,
) -> Result<ModelResponse, ModelPortError> {
    let output = response
        .choice
        .into_iter()
        .map(|content| match content {
            AssistantContent::Text(text) => Ok(ModelOutputPart::Text(text.text)),
            AssistantContent::ToolCall(_)
            | AssistantContent::Reasoning(_)
            | AssistantContent::Image(_) => Err(ModelPortError::Failed),
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ModelResponse::new(output, token_usage(response.usage)))
}

fn token_usage(usage: Usage) -> Option<TokenUsage> {
    let input_tokens = nonzero_usage(usage.input_tokens);
    let output_tokens = nonzero_usage(usage.output_tokens);
    if input_tokens.is_none() && output_tokens.is_none() {
        return None;
    }

    Some(TokenUsage::new(input_tokens, output_tokens))
}

fn nonzero_usage(value: u64) -> Option<u64> {
    if value == 0 { None } else { Some(value) }
}

fn map_completion_error(error: CompletionError) -> ModelPortError {
    if let Some(status) = error.provider_response_status() {
        let status = status.as_u16();
        if status == 408 || status == 429 || (500..=599).contains(&status) {
            return ModelPortError::Unavailable;
        }
        if (400..=499).contains(&status) {
            return ModelPortError::Rejected;
        }
        return ModelPortError::Failed;
    }

    match error {
        CompletionError::HttpError(_) => ModelPortError::Unavailable,
        CompletionError::JsonError(_)
        | CompletionError::UrlError(_)
        | CompletionError::RequestError(_)
        | CompletionError::ResponseError(_)
        | CompletionError::ProviderError(_)
        | CompletionError::ProviderResponse(_)
        | _ => ModelPortError::Failed,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use rig_core::{
        OneOrMany,
        completion::{
            CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
        },
        streaming::StreamingCompletionResponse,
    };

    use super::AssistantContent;

    #[derive(Clone)]
    pub struct FakeRigModel {
        state: Arc<Mutex<FakeRigState>>,
    }

    struct FakeRigState {
        results: VecDeque<FakeRigCompletion>,
        requests: Vec<CompletionRequest>,
        invocations: usize,
    }

    enum FakeRigCompletion {
        Ready(Box<Result<FakeRigResponse, CompletionError>>),
        Pending,
    }

    #[derive(Clone)]
    pub struct FakeRigResponse {
        content: OneOrMany<AssistantContent>,
        usage: Usage,
    }

    impl FakeRigResponse {
        #[must_use]
        pub fn text(text: impl Into<String>) -> Self {
            Self {
                content: OneOrMany::one(AssistantContent::text(text.into())),
                usage: Usage::new(),
            }
        }

        #[must_use]
        pub const fn with_usage(mut self, usage: Usage) -> Self {
            self.usage = usage;
            self
        }

        #[must_use]
        pub const fn content(content: OneOrMany<AssistantContent>, usage: Usage) -> Self {
            Self { content, usage }
        }
    }

    impl FakeRigModel {
        #[must_use]
        pub fn scripted(results: Vec<Result<FakeRigResponse, CompletionError>>) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeRigState {
                    results: results
                        .into_iter()
                        .map(|result| FakeRigCompletion::Ready(Box::new(result)))
                        .collect(),
                    requests: Vec::new(),
                    invocations: 0,
                })),
            }
        }

        #[must_use]
        pub fn scripted_text(text: impl Into<String>) -> Self {
            Self::scripted(vec![Ok(FakeRigResponse::text(text))])
        }

        #[must_use]
        pub fn scripted_error(error: CompletionError) -> Self {
            Self::scripted(vec![Err(error)])
        }

        #[must_use]
        pub fn pending() -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeRigState {
                    results: [FakeRigCompletion::Pending].into(),
                    requests: Vec::new(),
                    invocations: 0,
                })),
            }
        }

        #[must_use]
        pub fn invocation_count(&self) -> usize {
            self.state.lock().map_or(0, |state| state.invocations)
        }

        #[must_use]
        pub fn captured_requests(&self) -> Vec<CompletionRequest> {
            self.state
                .lock()
                .map_or_else(|_| Vec::new(), |state| state.requests.clone())
        }
    }

    impl CompletionModel for FakeRigModel {
        type Response = ();
        type StreamingResponse = ();
        type Client = ();

        fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
            Self::pending()
        }

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let result = match self.state.lock() {
                Ok(mut state) => {
                    state.invocations += 1;
                    state.requests.push(request);
                    state.results.pop_front()
                }
                Err(_) => {
                    return Err(CompletionError::ProviderError(
                        "fake rig model state is unavailable".to_owned(),
                    ));
                }
            };

            let Some(result) = result else {
                return Err(CompletionError::ProviderError(
                    "fake rig model has no scripted completion".to_owned(),
                ));
            };

            let response = match result {
                FakeRigCompletion::Ready(result) => (*result)?,
                FakeRigCompletion::Pending => std::future::pending().await,
            };

            Ok(CompletionResponse {
                choice: response.content,
                usage: response.usage,
                raw_response: (),
                message_id: None,
            })
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
        ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
            Err(CompletionError::ProviderError(
                "fake rig model streaming is unsupported".to_owned(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        future::Future,
        path::Path,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll, Waker},
        time::Duration,
    };

    use agent_core::{
        AgentEventKind, BudgetDimension, CapabilityKind, ModelMessage, ModelOutputPart,
        ModelRequest, ModelResponse, ModelRole, RunBudget, RunId, RunOutcome, RunStatus, SessionId,
        TokenUsage, ToolCall, ToolCallId, ToolDefinition, ToolInput, ToolName, ToolOutput,
        ToolResult, ToolSchema,
    };
    use agent_harness::{
        AuditFailurePolicy, AuditSink, ExecutionHarness, ExecutionStage, HarnessConfig,
        HarnessError, M0ReadOnlyPolicy, ModelPort, RunContext, ToolPort, ToolRegistry,
        testing::{FakeToolPort, InMemoryAuditSink},
    };
    use agent_loop::{
        LoopEffects, LoopEngine, LoopFuture, LoopProgram, LoopStepError, ReflectDecision,
        TerminalLoopDecision, VerificationResult,
    };
    use rig_core::{
        OneOrMany,
        completion::{AssistantContent, CompletionError, CompletionModel, Usage},
        message::{DocumentSourceKind, Image, Text},
    };

    use super::{
        RigMessage, RigModelAdapter,
        testing::{FakeRigModel, FakeRigResponse},
    };

    fn poll_once<F>(future: Pin<&mut F>) -> Poll<F::Output>
    where
        F: Future + ?Sized,
    {
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        future.poll(&mut context)
    }

    fn block_ready<F>(mut future: F) -> Option<F::Output>
    where
        F: Future + Unpin,
    {
        match poll_once(Pin::new(&mut future)) {
            Poll::Ready(output) => Some(output),
            Poll::Pending => None,
        }
    }

    fn request(messages: Vec<ModelMessage>) -> ModelRequest {
        ModelRequest::new(messages)
    }

    fn user_request(content: &str) -> ModelRequest {
        request(vec![ModelMessage::new(ModelRole::User, content)])
    }

    fn invoke(
        adapter: &RigModelAdapter<FakeRigModel>,
        request: ModelRequest,
    ) -> Result<ModelResponse, ModelPortError> {
        block_ready(adapter.invoke(request)).expect("adapter future should be ready")
    }

    fn expect_model_error(result: Result<ModelResponse, ModelPortError>) -> ModelPortError {
        result.err().expect("expected model error")
    }

    fn assert_text_outputs(response: &ModelResponse, expected: &[&str]) {
        assert_eq!(response.output().len(), expected.len());
        for (actual, expected) in response.output().iter().zip(expected.iter()) {
            assert!(matches!(actual, ModelOutputPart::Text(text) if text == expected));
        }
    }

    fn budget(model_calls: u32, tool_calls: u32, iterations: u32, elapsed: Duration) -> RunBudget {
        RunBudget::new(model_calls, tool_calls, iterations, elapsed)
            .expect("test budget must be valid")
    }

    fn context(
        model_calls: u32,
        tool_calls: u32,
        iterations: u32,
        elapsed: Duration,
    ) -> RunContext {
        RunContext::new(
            RunId::new(),
            SessionId::new(),
            budget(model_calls, tool_calls, iterations, elapsed),
        )
    }

    fn harness(
        model: Arc<dyn ModelPort>,
        tools: Vec<Arc<dyn ToolPort>>,
        audit: Arc<dyn AuditSink>,
    ) -> ExecutionHarness {
        let mut registry = ToolRegistry::new();
        for tool in tools {
            registry.register(tool).expect("test tool must register");
        }
        ExecutionHarness::new(
            model,
            registry,
            Arc::new(M0ReadOnlyPolicy),
            audit,
            HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
                .expect("test harness config must be valid"),
        )
    }

    fn rig_harness(
        model: FakeRigModel,
        audit: Arc<dyn AuditSink>,
    ) -> (ExecutionHarness, FakeRigModel) {
        let adapter: Arc<dyn ModelPort> = Arc::new(RigModelAdapter::new(model.clone()));
        (harness(adapter, Vec::new(), audit), model)
    }

    fn model_call_error<T>(result: Result<T, HarnessError>) -> HarnessError {
        result.err().expect("expected harness error")
    }

    fn tool_definition(name: &str) -> ToolDefinition {
        ToolDefinition::new(
            ToolName::new(name).expect("test tool name must be valid"),
            "deterministic read-only test tool",
            CapabilityKind::ReadOnly,
            ToolSchema::new(rig_core::serde_json::json!({"type": "object"}))
                .expect("test schema must be valid"),
        )
        .expect("test tool definition must be valid")
    }

    fn tool_call(name: &str, sentinel: &str) -> ToolCall {
        ToolCall::new(
            ToolCallId::new(),
            ToolName::new(name).expect("test tool name must be valid"),
            ToolInput::new(rig_core::serde_json::json!({"sentinel": sentinel})),
        )
    }

    use agent_harness::ModelPortError;

    #[test]
    fn empty_requests_are_rejected_before_calling_rig() {
        let model = FakeRigModel::scripted_text("unused");
        let adapter = RigModelAdapter::new(model.clone());

        let error = expect_model_error(invoke(&adapter, request(Vec::new())));

        assert_eq!(error, ModelPortError::Rejected);
        assert_eq!(model.invocation_count(), 0);
    }

    #[test]
    fn request_conversion_preserves_roles_and_order_with_empty_tools_and_no_telemetry() {
        let model = FakeRigModel::scripted_text("ok");
        let adapter = RigModelAdapter::new(model.clone());

        let response = invoke(
            &adapter,
            request(vec![
                ModelMessage::new(ModelRole::System, "system"),
                ModelMessage::new(ModelRole::User, "user"),
                ModelMessage::new(ModelRole::Assistant, "assistant"),
            ]),
        )
        .expect("request should succeed");

        assert_text_outputs(&response, &["ok"]);

        let requests = model.captured_requests();
        assert_eq!(requests.len(), 1);
        let captured = &requests[0];
        assert!(!captured.record_telemetry_content);
        assert!(captured.tools.is_empty());
        assert!(captured.documents.is_empty());
        assert!(captured.additional_params.is_none());
        assert!(captured.tool_choice.is_none());
        assert!(captured.output_schema.is_none());

        let messages = captured.chat_history.iter().collect::<Vec<_>>();
        assert_eq!(messages.len(), 3);
        assert!(matches!(messages[0], RigMessage::System { content } if content == "system"));
        assert!(matches!(messages[1], RigMessage::User { content }
                if matches!(content.first_ref(), rig_core::message::UserContent::Text(text) if text.text == "user")));
        assert!(
            matches!(messages[2], RigMessage::Assistant { id: None, content }
                if matches!(content.first_ref(), AssistantContent::Text(text) if text.text == "assistant"))
        );
    }

    #[test]
    fn text_only_response_maps_all_text_parts_atomically() {
        let content = OneOrMany::many(vec![
            AssistantContent::text("first"),
            AssistantContent::text("second"),
        ])
        .expect("non-empty content");
        let model =
            FakeRigModel::scripted(vec![Ok(FakeRigResponse::content(content, Usage::new()))]);
        let adapter = RigModelAdapter::new(model);

        let response = invoke(
            &adapter,
            request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
        )
        .expect("text-only response should convert");

        assert_text_outputs(&response, &["first", "second"]);
    }

    #[test]
    fn mixed_response_with_tool_call_fails_entire_conversion() {
        let tool_call = rig_core::message::ToolCall::new(
            "call-1".to_owned(),
            rig_core::message::ToolFunction::new(
                "lookup".to_owned(),
                rig_core::serde_json::json!({}),
            ),
        );
        let content = OneOrMany::many(vec![
            AssistantContent::text("before"),
            AssistantContent::ToolCall(tool_call),
        ])
        .expect("non-empty content");
        let model =
            FakeRigModel::scripted(vec![Ok(FakeRigResponse::content(content, Usage::new()))]);
        let adapter = RigModelAdapter::new(model);

        let error = expect_model_error(invoke(
            &adapter,
            request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
        ));

        assert_eq!(error, ModelPortError::Failed);
    }

    #[test]
    fn reasoning_and_image_outputs_fail_conversion() {
        let image = Image {
            data: DocumentSourceKind::Raw(Vec::new()),
            media_type: None,
            detail: None,
            additional_params: None,
        };

        for content in [
            AssistantContent::Reasoning(rig_core::message::Reasoning::new("thinking")),
            AssistantContent::Image(image),
        ] {
            let model = FakeRigModel::scripted(vec![Ok(FakeRigResponse::content(
                OneOrMany::one(content),
                Usage::new(),
            ))]);
            let adapter = RigModelAdapter::new(model);

            let error = expect_model_error(invoke(
                &adapter,
                request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
            ));

            assert_eq!(error, ModelPortError::Failed);
        }
    }

    #[test]
    fn usage_zero_sentinel_maps_to_absent_usage_and_nonzero_fields_are_preserved() {
        let adapter = RigModelAdapter::new(FakeRigModel::scripted(vec![Ok(
            FakeRigResponse::text("none").with_usage(Usage::new()),
        )]));
        let response = invoke(
            &adapter,
            request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
        )
        .expect("response should convert");
        assert_eq!(response.token_usage(), None);

        let mut usage = Usage::new();
        usage.input_tokens = 0;
        usage.output_tokens = 0;
        usage.total_tokens = 11;
        usage.reasoning_tokens = 5;
        let adapter = RigModelAdapter::new(FakeRigModel::scripted(vec![Ok(
            FakeRigResponse::text("some").with_usage(usage),
        )]));

        let response = invoke(
            &adapter,
            request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
        )
        .expect("response should convert");

        assert_eq!(response.token_usage(), None);

        let mut usage = Usage::new();
        usage.input_tokens = 11;
        let adapter = RigModelAdapter::new(FakeRigModel::scripted(vec![Ok(
            FakeRigResponse::text("some").with_usage(usage),
        )]));

        let response = invoke(
            &adapter,
            request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
        )
        .expect("response should convert");

        assert_eq!(
            response.token_usage(),
            Some(TokenUsage::new(Some(11), None))
        );
    }

    #[test]
    fn completion_errors_map_without_exposing_raw_error_content() {
        for (status, expected) in [
            (400, ModelPortError::Rejected),
            (404, ModelPortError::Rejected),
            (408, ModelPortError::Unavailable),
            (429, ModelPortError::Unavailable),
            (500, ModelPortError::Unavailable),
            (503, ModelPortError::Unavailable),
            (200, ModelPortError::Failed),
        ] {
            let error = completion_error_with_status(status);
            let adapter = RigModelAdapter::new(FakeRigModel::scripted_error(error));

            let actual = expect_model_error(invoke(
                &adapter,
                request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
            ));

            assert_eq!(actual, expected, "status {status}");
        }

        for error in [
            CompletionError::ResponseError("raw response body".to_owned()),
            CompletionError::ProviderError("raw provider body".to_owned()),
            CompletionError::from_provider_body("raw statusless provider body"),
        ] {
            let adapter = RigModelAdapter::new(FakeRigModel::scripted_error(error));
            let actual = expect_model_error(invoke(
                &adapter,
                request(vec![ModelMessage::new(ModelRole::User, "prompt")]),
            ));
            assert_eq!(actual, ModelPortError::Failed);
        }
    }

    #[test]
    fn url_request_and_statusless_http_errors_have_exact_mappings() {
        let url_error = url::Url::parse("http://[::1").expect_err("URL must be invalid");
        assert_eq!(
            super::map_completion_error(CompletionError::UrlError(url_error)),
            ModelPortError::Failed
        );

        let request_error = std::io::Error::other("REQUEST_ERROR_SENTINEL");
        assert_eq!(
            super::map_completion_error(CompletionError::RequestError(Box::new(request_error))),
            ModelPortError::Failed
        );

        assert_eq!(
            super::map_completion_error(CompletionError::HttpError(
                rig_core::http_client::Error::StreamEnded,
            )),
            ModelPortError::Unavailable
        );
    }

    #[test]
    fn fake_rig_model_pending_and_streaming_errors_do_not_panic() {
        let model = FakeRigModel::pending();
        let adapter = RigModelAdapter::new(model);
        let mut completion =
            adapter.invoke(request(vec![ModelMessage::new(ModelRole::User, "prompt")]));
        assert!(matches!(poll_once(completion.as_mut()), Poll::Pending));

        let stream_model = FakeRigModel::pending();
        let stream = block_ready(Box::pin(stream_model.stream(rig_request("prompt"))))
            .expect("stream future should be ready");
        assert!(matches!(stream, Err(CompletionError::ProviderError(_))));
    }

    #[tokio::test]
    async fn execution_harness_model_budget_increments_exactly_once_for_rig_adapter() {
        let audit = Arc::new(InMemoryAuditSink::new());
        let (runtime, model) = rig_harness(FakeRigModel::scripted_text("ok"), audit.clone());
        let mut run = context(1, 0, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");

        let response = runtime
            .invoke_model(&mut run, user_request("budget once"))
            .await
            .expect("model invocation must succeed");

        assert_text_outputs(&response, &["ok"]);
        assert_eq!(run.usage().model_calls(), 1);
        assert_eq!(model.invocation_count(), 1);
        assert!(matches!(
            audit.events()[1].kind(),
            AgentEventKind::ModelInvocationStarted {
                usage: 1,
                limit: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn zero_model_call_budget_never_invokes_rig_adapter() {
        let (runtime, model) = rig_harness(
            FakeRigModel::scripted_text("must not invoke"),
            Arc::new(InMemoryAuditSink::new()),
        );
        let mut run = context(0, 0, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");

        let error = model_call_error(runtime.invoke_model(&mut run, user_request("deny")).await);

        assert_eq!(error.budget_dimension(), Some(BudgetDimension::ModelCalls));
        assert_eq!(model.invocation_count(), 0);
        assert_eq!(run.usage().model_calls(), 0);
        assert_eq!(
            run.status(),
            &RunStatus::Finished(RunOutcome::BudgetExceeded {
                dimension: BudgetDimension::ModelCalls,
            })
        );
    }

    #[tokio::test]
    async fn exhausted_model_call_budget_never_invokes_rig_adapter_again() {
        let (runtime, model) = rig_harness(
            FakeRigModel::scripted(vec![
                Ok(FakeRigResponse::text("first")),
                Ok(FakeRigResponse::text("must not invoke")),
            ]),
            Arc::new(InMemoryAuditSink::new()),
        );
        let mut run = context(1, 0, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        runtime
            .invoke_model(&mut run, user_request("first"))
            .await
            .expect("first invocation must succeed");

        let error = model_call_error(runtime.invoke_model(&mut run, user_request("deny")).await);

        assert_eq!(error.budget_dimension(), Some(BudgetDimension::ModelCalls));
        assert_eq!(model.invocation_count(), 1);
        assert_eq!(run.usage().model_calls(), 1);
        assert_eq!(
            run.status(),
            &RunStatus::Finished(RunOutcome::BudgetExceeded {
                dimension: BudgetDimension::ModelCalls,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pending_rig_adapter_cancellation_terminalizes_run() {
        let (runtime, model) =
            rig_harness(FakeRigModel::pending(), Arc::new(InMemoryAuditSink::new()));
        let mut run = context(1, 0, 1, Duration::from_secs(30));
        let cancellation = runtime.start_run(&mut run).await.expect("run must start");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            cancellation.request_cancel();
        });

        let error = model_call_error(
            runtime
                .invoke_model(&mut run, user_request("pending cancel"))
                .await,
        );

        assert!(matches!(
            error,
            HarnessError::Cancelled {
                stage: ExecutionStage::Invocation
            }
        ));
        assert_eq!(run.status(), &RunStatus::Finished(RunOutcome::Cancelled));
        assert_eq!(model.invocation_count(), 1);
        assert_eq!(model.captured_requests().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn pending_rig_adapter_deadline_terminalizes_run() {
        let (runtime, model) =
            rig_harness(FakeRigModel::pending(), Arc::new(InMemoryAuditSink::new()));
        let mut run = context(1, 0, 1, Duration::from_secs(1));
        runtime.start_run(&mut run).await.expect("run must start");

        let error = model_call_error(
            runtime
                .invoke_model(&mut run, user_request("pending deadline"))
                .await,
        );

        assert!(matches!(
            error,
            HarnessError::DeadlineExceeded {
                stage: ExecutionStage::Invocation
            }
        ));
        assert_eq!(
            run.status(),
            &RunStatus::Finished(RunOutcome::BudgetExceeded {
                dimension: BudgetDimension::Elapsed,
            })
        );
        assert_eq!(model.invocation_count(), 1);
        assert_eq!(model.captured_requests().len(), 1);
    }

    #[tokio::test]
    async fn rig_adapter_events_do_not_leak_request_response_or_error_sentinels() {
        const REQUEST_SENTINEL: &str = "REQUEST_SENTINEL_DO_NOT_AUDIT";
        const RESPONSE_SENTINEL: &str = "RESPONSE_SENTINEL_DO_NOT_AUDIT";
        const ERROR_SENTINEL: &str = "ERROR_SENTINEL_DO_NOT_AUDIT";

        let audit = Arc::new(InMemoryAuditSink::new());
        let (runtime, _model) = rig_harness(
            FakeRigModel::scripted(vec![
                Ok(FakeRigResponse::text(RESPONSE_SENTINEL)),
                Err(CompletionError::ProviderError(ERROR_SENTINEL.to_owned())),
            ]),
            audit.clone(),
        );
        let mut run = context(2, 0, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        runtime
            .invoke_model(&mut run, user_request(REQUEST_SENTINEL))
            .await
            .expect("first invocation must succeed");
        let error = model_call_error(
            runtime
                .invoke_model(&mut run, user_request(REQUEST_SENTINEL))
                .await,
        );
        assert_eq!(error, HarnessError::ModelPort(ModelPortError::Failed));

        let events =
            rig_core::serde_json::to_string(&audit.events()).expect("events must serialize");
        for sentinel in [REQUEST_SENTINEL, RESPONSE_SENTINEL, ERROR_SENTINEL] {
            assert!(!events.contains(sentinel));
        }
    }

    #[tokio::test]
    async fn one_iteration_loop_uses_rig_adapter_and_read_only_tool_then_completes() {
        let model = FakeRigModel::scripted_text("loop model output");
        let model_handle = model.clone();
        let model_port: Arc<dyn ModelPort> = Arc::new(RigModelAdapter::new(model));

        let tool_name = "m3_lookup";
        let tool = Arc::new(FakeToolPort::succeeding(
            tool_definition(tool_name),
            ToolOutput::new(rig_core::serde_json::json!({
                "value": "LOOP_TOOL_OUTPUT_SENTINEL"
            })),
        ));
        let tool_handle = tool.clone();
        let tool_port: Arc<dyn ToolPort> = tool;
        let runtime = harness(
            model_port,
            vec![tool_port],
            Arc::new(InMemoryAuditSink::new()),
        );
        let mut run = context(1, 1, 1, Duration::from_secs(30));
        runtime.start_run(&mut run).await.expect("run must start");
        let mut program = RigLoopProgram::new(tool_call(tool_name, "LOOP_TOOL_INPUT_SENTINEL"));
        let mut state = RigLoopState::default();

        let summary = LoopEngine::new()
            .run(&runtime, &mut run, &mut program, &mut state)
            .await
            .expect("loop must complete");

        assert_eq!(model_handle.invocation_count(), 1);
        assert_eq!(tool_handle.invocation_count(), 1);
        assert!(state.model_completed);
        assert!(state.tool_completed);
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

    #[test]
    fn sibling_core_harness_loop_sources_do_not_import_rig() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let crates_dir = manifest_dir.parent().expect("crate has parent directory");

        for crate_name in ["agent-core", "agent-harness", "agent-loop"] {
            let src_dir = crates_dir.join(crate_name).join("src");
            assert_no_rig_references(&src_dir);
        }
    }

    fn completion_error_with_status(status: u16) -> CompletionError {
        let response = rig_core::http_client::Response::builder()
            .status(status)
            .body(())
            .expect("test status must be valid");
        CompletionError::from_http_response(response.status(), "provider body")
    }

    fn rig_request(prompt: &str) -> rig_core::completion::CompletionRequest {
        rig_core::completion::CompletionRequest {
            model: None,
            preamble: None,
            chat_history: OneOrMany::one(RigMessage::User {
                content: OneOrMany::one(rig_core::message::UserContent::Text(Text::new(prompt))),
            }),
            documents: Vec::new(),
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            additional_params: None,
            output_schema: None,
            record_telemetry_content: false,
        }
    }

    fn assert_no_rig_references(path: &Path) {
        for entry in fs::read_dir(path).expect("source directory must be readable") {
            let entry = entry.expect("source entry must be readable");
            let path = entry.path();
            if path.is_dir() {
                assert_no_rig_references(&path);
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }

            let source = fs::read_to_string(&path).expect("source file must be readable");
            for forbidden in ["rig_core", "rig::", "rig_agent"] {
                assert!(
                    !source.contains(forbidden),
                    "{} contains forbidden Rig reference {forbidden}",
                    path.display(),
                );
            }
        }
    }

    #[derive(Default)]
    struct RigLoopState {
        model_completed: bool,
        tool_completed: bool,
    }

    struct RigLoopProgram {
        call: ToolCall,
    }

    impl RigLoopProgram {
        const fn new(call: ToolCall) -> Self {
            Self { call }
        }
    }

    impl LoopProgram for RigLoopProgram {
        type WorkingState = RigLoopState;

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
            Box::pin(async move {
                let response = effects
                    .invoke_model(user_request("LOOP_MODEL_REQUEST_SENTINEL"))
                    .await?;
                working_state.model_completed = matches!(
                    response.output().first(),
                    Some(ModelOutputPart::Text(text)) if text == "loop model output"
                );
                Ok(())
            })
        }

        fn act<'a>(
            &'a mut self,
            _iteration: u32,
            working_state: &'a mut Self::WorkingState,
            mut effects: LoopEffects<'a>,
        ) -> LoopFuture<'a, Result<(), LoopStepError>> {
            let call = self.call.clone();
            Box::pin(async move {
                let result = effects.invoke_tool(call).await?;
                working_state.tool_completed = matches!(result, ToolResult::Succeeded { .. });
                Ok(())
            })
        }

        fn verify<'a>(
            &'a mut self,
            _iteration: u32,
            working_state: &'a mut Self::WorkingState,
            _effects: LoopEffects<'a>,
        ) -> LoopFuture<'a, Result<VerificationResult, LoopStepError>> {
            let passed = working_state.model_completed && working_state.tool_completed;
            Box::pin(std::future::ready(Ok(if passed {
                VerificationResult::Passed
            } else {
                VerificationResult::Failed
            })))
        }

        fn reflect<'a>(
            &'a mut self,
            _iteration: u32,
            _working_state: &'a mut Self::WorkingState,
            verification: VerificationResult,
            _effects: LoopEffects<'a>,
        ) -> LoopFuture<'a, Result<ReflectDecision, LoopStepError>> {
            Box::pin(std::future::ready(Ok(match verification {
                VerificationResult::Passed => ReflectDecision::Complete,
                VerificationResult::Failed => ReflectDecision::Fail {
                    kind: agent_core::LoopFailureKind::VerificationFailed,
                },
            })))
        }
    }
}

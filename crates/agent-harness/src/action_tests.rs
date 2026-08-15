use std::{sync::Arc, time::Duration};

use agent_core::{
    ActionProposal, ActionProposalId, ActionRejectionReason, AgentEventKind, BudgetDimension,
    CapabilityKind, ModelCallId, ModelMessage, ModelOutputPart, ModelRequest, ModelResponse,
    ModelRole, RunBudget, RunId, RunOutcome, RunStatus, SessionId, ToolCall, ToolCallId,
    ToolDefinition, ToolDomainFailure, ToolDomainFailureKind, ToolInput, ToolName, ToolOutput,
    ToolResult, ToolSchema,
};
use serde_json::json;

use crate::{
    ActionPreparationError, ApprovalPort, AuditFailurePolicy, AuditSink, AuthorizationDecision,
    CapabilityPolicy, ExecutionHarness, HarnessConfig, HarnessError, M0ReadOnlyPolicy,
    M6ApprovalPolicy, ModelPort, PortFuture, RunContext, ToolPort, ToolPortError, ToolRegistry,
    ToolRegistryError, ToolSchemaRegistrationError,
    action::{ActionValidator, TextActionDecoder},
    testing::{
        FailingAuditSink, FakeContainedToolPort, FakeModelPort, FakeToolPort, InMemoryAuditSink,
        ScriptedApprovalPort,
    },
};

struct DefinitionOnlyTool {
    definition: ToolDefinition,
}

impl ToolPort for DefinitionOnlyTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn invoke<'a>(&'a self, _call: ToolCall) -> PortFuture<'a, Result<ToolResult, ToolPortError>> {
        Box::pin(std::future::ready(Err(ToolPortError::Unavailable)))
    }
}

fn response(text: impl Into<String>) -> ModelResponse {
    ModelResponse::new(vec![ModelOutputPart::Text(text.into())], None)
}

fn decode(
    text: impl Into<String>,
) -> Result<agent_core::ActionProposal, crate::ActionValidationError> {
    TextActionDecoder::decode(response(text), ActionProposalId::new(), ModelCallId::new())
}

fn definition(name: &str, schema: serde_json::Value) -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new(name).expect("tool name must be valid"),
        "test tool",
        CapabilityKind::ReadOnly,
        ToolSchema::new(schema).expect("schema must be an object"),
    )
    .expect("definition must be valid")
}

fn registry(name: &str, schema: serde_json::Value) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let port: Arc<dyn ToolPort> = Arc::new(DefinitionOnlyTool {
        definition: definition(name, schema),
    });
    registry.register(port).expect("tool must register");
    registry
}

#[test]
fn exact_action_envelope_decodes_without_normalizing_tool_name() {
    let proposal = decode(r#"{"action":{"tool":"ExactTool","arguments":{"value":1}}}"#)
        .expect("envelope must decode");

    assert_eq!(proposal.tool_name().as_str(), "ExactTool");
    assert_eq!(proposal.arguments().as_value(), &json!({"value": 1}));
}

#[test]
fn malformed_fenced_prose_trailing_missing_and_unknown_envelopes_are_rejected() {
    for invalid in [
        "not-json",
        "```json\n{\"action\":{\"tool\":\"lookup\",\"arguments\":{}}}\n```",
        "prose {\"action\":{\"tool\":\"lookup\",\"arguments\":{}}}",
        "{\"action\":{\"tool\":\"lookup\",\"arguments\":{}}} trailing",
        "{}",
        "{\"action\":{\"tool\":\"lookup\"}}",
        "{\"action\":{\"tool\":\"lookup\",\"arguments\":{},\"extra\":1}}",
        "{\"action\":{\"tool\":\"lookup\",\"arguments\":{}},\"extra\":1}",
        "{\"action\":{\"tool\":\" lookup\",\"arguments\":{}}}",
    ] {
        let error = decode(invalid).expect_err("invalid envelope must be rejected");
        assert!(matches!(
            error.reason(),
            ActionRejectionReason::MalformedEnvelope | ActionRejectionReason::InvalidToolName
        ));
    }
}

#[test]
fn duplicate_envelope_and_nested_argument_keys_are_rejected_during_decoding() {
    for duplicate in [
        r#"{"action":{"tool":"lookup","tool":"other","arguments":{}}}"#,
        r#"{"action":{"tool":"lookup","arguments":{}},"action":{"tool":"other","arguments":{}}}"#,
        r#"{"action":{"tool":"lookup","arguments":{"nested":{"key":1,"key":2}}}}"#,
    ] {
        let error =
            decode(duplicate).expect_err("duplicate keys must fail before a proposal exists");
        assert_eq!(error.reason(), ActionRejectionReason::MalformedEnvelope);
    }
}

#[test]
fn decoder_rejects_empty_oversized_and_unsupported_output_shapes() {
    let empty = decode("   ").expect_err("empty output must fail");
    assert_eq!(empty.reason(), ActionRejectionReason::EmptyOutput);

    let oversized = decode("x".repeat(crate::MAX_PLANNING_RESPONSE_BYTES + 1))
        .expect_err("oversized output must fail before parsing");
    assert_eq!(oversized.reason(), ActionRejectionReason::ResponseTooLarge);

    let response = ModelResponse::new(
        vec![
            ModelOutputPart::Text("{}".to_owned()),
            ModelOutputPart::Text("{}".to_owned()),
        ],
        None,
    );
    let error = TextActionDecoder::decode(response, ActionProposalId::new(), ModelCallId::new())
        .expect_err("multiple text parts must fail atomically");
    assert_eq!(
        error.reason(),
        ActionRejectionReason::UnsupportedOutputShape
    );
}

#[test]
fn argument_complexity_limits_are_enforced_before_validation() {
    let oversized_name = "x".repeat(crate::MAX_ACTION_TOOL_NAME_BYTES + 1);
    let error = decode(format!(
        "{{\"action\":{{\"tool\":\"{oversized_name}\",\"arguments\":{{}}}}}}"
    ))
    .expect_err("oversized tool name must fail");
    assert_eq!(error.reason(), ActionRejectionReason::SecurityLimitExceeded);

    let mut nested = json!({});
    for _ in 0..crate::MAX_ACTION_ARGUMENT_DEPTH {
        nested = json!({"nested": nested});
    }
    let error = decode(json!({"action": {"tool": "lookup", "arguments": nested}}).to_string())
        .expect_err("excessive nesting must fail");
    assert_eq!(error.reason(), ActionRejectionReason::SecurityLimitExceeded);

    let keys = (0..=crate::MAX_ACTION_ARGUMENT_KEYS)
        .map(|index| (format!("key_{index}"), json!(index)))
        .collect::<serde_json::Map<_, _>>();
    let error = decode(json!({"action": {"tool": "lookup", "arguments": keys}}).to_string())
        .expect_err("too many argument keys must fail");
    assert_eq!(error.reason(), ActionRejectionReason::SecurityLimitExceeded);

    let oversized_arguments = "x".repeat(crate::MAX_ACTION_ARGUMENT_BYTES);
    let error = decode(
        json!({"action": {"tool": "lookup", "arguments": {"value": oversized_arguments}}})
            .to_string(),
    )
    .expect_err("oversized serialized arguments must fail");
    assert_eq!(error.reason(), ActionRejectionReason::SecurityLimitExceeded);

    let excessive_array = (0..=crate::MAX_ACTION_ARGUMENT_ARRAY_ELEMENTS)
        .map(|value| json!(value))
        .collect::<Vec<_>>();
    let error = decode(
        json!({"action": {"tool": "lookup", "arguments": {"values": excessive_array}}}).to_string(),
    )
    .expect_err("too many total array elements must fail");
    assert_eq!(error.reason(), ActionRejectionReason::SecurityLimitExceeded);
}

#[test]
fn schema_profile_is_context_aware_and_explicitly_draft_2020_12() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "title": "Action arguments",
        "description": "Annotations are permitted",
        "properties": {
            "oneOf": {"type": "string"},
            "custom_property_name": {
                "enum": [{"oneOf": "data, not schema"}],
                "const": {"$ref": "also data"}
            }
        },
        "additionalProperties": false
    });
    let registry_with_draft = registry("lookup", schema);
    let binding = registry_with_draft
        .get(&ToolName::new("lookup").expect("name must be valid"))
        .expect("binding must exist");

    assert_eq!(binding.validator().draft(), jsonschema::Draft::Draft202012);

    let registry_without_draft = registry(
        "lookup",
        json!({"type": "object", "additionalProperties": true}),
    );
    let binding_without_draft = registry_without_draft
        .get(&ToolName::new("lookup").expect("name must be valid"))
        .expect("binding must exist");
    assert_eq!(
        binding_without_draft.validator().draft(),
        jsonschema::Draft::Draft202012
    );
}

#[test]
fn wrong_draft_and_unsupported_schema_keywords_fail_registration() {
    for schema in [
        json!({"$schema": "http://json-schema.org/draft-07/schema#", "type": "object"}),
        json!({"type": "object", "$ref": "#/$defs/action"}),
        json!({"type": "object", "$dynamicRef": "#action"}),
        json!({"type": "object", "format": "custom"}),
        json!({"type": "object", "pattern": ".*"}),
        json!({"type": "object", "patternProperties": {".*": {"type": "string"}}}),
        json!({"type": "object", "oneOf": [{"type": "object"}]}),
        json!({"type": "object", "anyOf": [{"type": "object"}]}),
        json!({"type": "object", "allOf": [{"type": "object"}]}),
        json!({"type": "object", "not": {"type": "string"}}),
        json!({"type": "object", "if": {"type": "object"}}),
        json!({"type": "object", "then": {"type": "object"}}),
        json!({"type": "object", "else": {"type": "object"}}),
        json!({"type": "object", "unevaluatedProperties": false}),
        json!({"type": "object", "dependentSchemas": {}}),
        json!({"type": ["object", "null"]}),
        json!({"type": "object", "customKeyword": true}),
    ] {
        let mut registry = ToolRegistry::new();
        let port: Arc<dyn ToolPort> = Arc::new(DefinitionOnlyTool {
            definition: definition("lookup", schema),
        });
        assert!(matches!(
            registry.register(port),
            Err(ToolRegistryError::InvalidInputSchema {
                reason: ToolSchemaRegistrationError::UnsupportedProfile,
                ..
            })
        ));
    }
}

#[test]
fn schema_size_depth_and_key_limits_fail_registration() {
    let oversized = json!({
        "type": "object",
        "description": "x".repeat(crate::MAX_TOOL_SCHEMA_BYTES)
    });
    assert_schema_registration_reason(oversized, ToolSchemaRegistrationError::TooLarge);

    let mut deeply_nested = json!({"type": "string"});
    for _ in 0..crate::MAX_TOOL_SCHEMA_DEPTH {
        deeply_nested = json!({"type": "array", "items": deeply_nested});
    }
    assert_schema_registration_reason(deeply_nested, ToolSchemaRegistrationError::TooComplex);

    let properties = (0..crate::MAX_TOOL_SCHEMA_KEYS)
        .map(|index| (format!("property_{index}"), json!({"type": "string"})))
        .collect::<serde_json::Map<_, _>>();
    assert_schema_registration_reason(
        json!({"type": "object", "properties": properties}),
        ToolSchemaRegistrationError::TooComplex,
    );
}

fn assert_schema_registration_reason(
    schema: serde_json::Value,
    expected: ToolSchemaRegistrationError,
) {
    let mut registry = ToolRegistry::new();
    let port: Arc<dyn ToolPort> = Arc::new(DefinitionOnlyTool {
        definition: definition("lookup", schema),
    });
    let error = registry
        .register(port)
        .expect_err("schema outside fixed limits must fail registration");
    assert!(matches!(
        error,
        ToolRegistryError::InvalidInputSchema { reason, .. } if reason == expected
    ));
}

#[test]
fn schema_controls_unknown_argument_properties() {
    let proposal = || {
        decode(r#"{"action":{"tool":"lookup","arguments":{"unknown":1}}}"#)
            .expect("decoder must not reject schema-owned property names")
    };
    let permissive = registry(
        "lookup",
        json!({"type": "object", "additionalProperties": true}),
    );
    let restrictive = registry(
        "lookup",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    );

    assert!(ActionValidator::validate(proposal(), &permissive).is_ok());
    let error = ActionValidator::validate(proposal(), &restrictive)
        .expect_err("schema must reject additional property");
    assert_eq!(
        error.reason(),
        ActionRejectionReason::ArgumentsSchemaMismatch
    );
}

#[test]
fn exact_tool_name_lookup_and_schema_validation_are_distinct() {
    let registry = registry(
        "ExactTool",
        json!({
            "type": "object",
            "properties": {"count": {"type": "integer", "minimum": 1}},
            "required": ["count"],
            "additionalProperties": false
        }),
    );

    let unknown = decode(r#"{"action":{"tool":"exacttool","arguments":{"count":1}}}"#)
        .expect("proposal must decode");
    assert_eq!(
        ActionValidator::validate(unknown, &registry)
            .expect_err("case-changing must not match")
            .reason(),
        ActionRejectionReason::UnknownTool
    );

    let invalid = decode(r#"{"action":{"tool":"ExactTool","arguments":{"count":0}}}"#)
        .expect("proposal must decode");
    assert_eq!(
        ActionValidator::validate(invalid, &registry)
            .expect_err("schema-invalid arguments must fail")
            .reason(),
        ActionRejectionReason::ArgumentsSchemaMismatch
    );

    let valid = decode(r#"{"action":{"tool":"ExactTool","arguments":{"count":1}}}"#)
        .expect("proposal must decode");
    assert!(ActionValidator::validate(valid, &registry).is_ok());
}

#[test]
fn validator_reapplies_structural_limits_for_non_text_candidates() {
    let registry = registry(
        "lookup",
        json!({"type": "object", "additionalProperties": true}),
    );
    let proposal = ActionProposal::new(
        ActionProposalId::new(),
        ModelCallId::new(),
        ToolName::new("lookup").expect("tool name must be valid"),
        ToolInput::new(json!({"value": "x".repeat(crate::MAX_ACTION_ARGUMENT_BYTES)})),
    );
    assert_eq!(
        ActionValidator::validate(proposal, &registry)
            .expect_err("all candidate sources must receive structural limits")
            .reason(),
        ActionRejectionReason::SecurityLimitExceeded
    );

    let non_object = ActionProposal::new(
        ActionProposalId::new(),
        ModelCallId::new(),
        ToolName::new("lookup").expect("tool name must be valid"),
        ToolInput::new(json!(["not", "an", "object"])),
    );
    assert_eq!(
        ActionValidator::validate(non_object, &registry)
            .expect_err("arguments must remain object-shaped")
            .reason(),
        ActionRejectionReason::ArgumentsSchemaMismatch
    );
}

struct GovernedFixture {
    harness: ExecutionHarness,
    audit: Arc<InMemoryAuditSink>,
    tool: Arc<FakeToolPort>,
}

fn governed_fixture(
    response_text: &str,
    capability: CapabilityKind,
    schema: serde_json::Value,
    tool: impl FnOnce(ToolDefinition) -> FakeToolPort,
) -> GovernedFixture {
    let model = Arc::new(FakeModelPort::scripted(vec![Ok(response(response_text))]));
    let model_port: Arc<dyn ModelPort> = model;
    let tool = Arc::new(tool(definition_with_capability(
        "lookup", capability, schema,
    )));
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    let mut registry = ToolRegistry::new();
    registry.register(tool_port).expect("tool must register");
    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let harness = ExecutionHarness::new(
        model_port,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        audit_sink,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("harness config must be valid"),
    );
    GovernedFixture {
        harness,
        audit,
        tool,
    }
}

fn definition_with_capability(
    name: &str,
    capability: CapabilityKind,
    schema: serde_json::Value,
) -> ToolDefinition {
    ToolDefinition::new(
        ToolName::new(name).expect("tool name must be valid"),
        "test tool",
        capability,
        ToolSchema::new(schema).expect("schema must be an object"),
    )
    .expect("definition must be valid")
}

fn run_context(tool_calls: u32) -> RunContext {
    let budget =
        RunBudget::new(1, tool_calls, 1, Duration::from_secs(30)).expect("budget must be valid");
    RunContext::new(RunId::new(), SessionId::new(), budget)
}

async fn prepare(
    harness: &ExecutionHarness,
    context: &mut RunContext,
) -> Result<crate::ValidatedAction, ActionPreparationError> {
    let invocation = harness
        .invoke_model_tracked(
            context,
            ModelRequest::new(vec![ModelMessage::new(ModelRole::User, "prompt-sentinel")]),
        )
        .await?;
    harness.prepare_action(context, invocation).await
}

fn governed_fixture_with_audit_failure(
    failure_policy: AuditFailurePolicy,
    fail_on_attempt: usize,
) -> (ExecutionHarness, Arc<FakeToolPort>, Arc<FailingAuditSink>) {
    let model: Arc<dyn ModelPort> = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
    ))]));
    let tool = Arc::new(FakeToolPort::succeeding(
        definition(
            "lookup",
            json!({"type": "object", "additionalProperties": false}),
        ),
        ToolOutput::new(json!({"status": "ok"})),
    ));
    let tool_port: Arc<dyn ToolPort> = tool.clone();
    let mut registry = ToolRegistry::new();
    registry.register(tool_port).expect("tool must register");
    let audit = Arc::new(FailingAuditSink::on_attempt(fail_on_attempt));
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let harness = ExecutionHarness::new(
        model,
        registry,
        Arc::new(M0ReadOnlyPolicy),
        audit_sink,
        HarnessConfig::new(failure_policy, Duration::from_secs(1))
            .expect("harness config must be valid"),
    );
    (harness, tool, audit)
}

#[tokio::test]
async fn governed_read_only_action_correlates_model_proposal_and_tool_call() {
    let fixture = governed_fixture(
        r#"{"action":{"tool":"lookup","arguments":{"query":"argument-sentinel"}}}"#,
        CapabilityKind::ReadOnly,
        json!({
            "type": "object",
            "description": "schema-sentinel",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
            "additionalProperties": false
        }),
        |definition| {
            FakeToolPort::succeeding(
                definition,
                ToolOutput::new(json!({"secret": "result-sentinel"})),
            )
        },
    );
    let mut context = run_context(1);
    fixture
        .harness
        .start_run(&mut context)
        .await
        .expect("run must start");

    let action = prepare(&fixture.harness, &mut context)
        .await
        .expect("action must validate");
    let proposal_id = action.proposal_id();
    let result = fixture
        .harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect("read-only action must execute");
    let tool_call_id = result.call_id();

    assert_eq!(fixture.tool.invocation_count(), 1);
    assert_eq!(context.usage().model_calls(), 1);
    assert_eq!(context.usage().tool_calls(), 1);
    assert_ne!(proposal_id.as_uuid(), tool_call_id.as_uuid());

    let events = fixture.audit.events();
    let model_call_id = events
        .iter()
        .find_map(|event| match event.kind() {
            AgentEventKind::ModelInvocationStarted { model_call_id, .. } => Some(*model_call_id),
            _ => None,
        })
        .expect("model correlation event must exist");
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ActionProposed {
            model_call_id: event_model_call_id,
            action_proposal_id,
            ..
        } if *event_model_call_id == model_call_id && *action_proposal_id == proposal_id
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind(),
        AgentEventKind::ActionExecutionBound {
            action_proposal_id,
            tool_call_id: event_tool_call_id,
        } if *action_proposal_id == proposal_id && *event_tool_call_id == tool_call_id
    )));

    let serialized = serde_json::to_string(&events).expect("events must serialize");
    for sentinel in [
        "prompt-sentinel",
        "argument-sentinel",
        "schema-sentinel",
        "result-sentinel",
    ] {
        assert!(!serialized.contains(sentinel));
    }
}

#[tokio::test]
async fn fail_closed_action_audit_failures_stop_before_tool_execution() {
    for fail_on_attempt in [4, 5, 6] {
        let (harness, tool, audit) =
            governed_fixture_with_audit_failure(AuditFailurePolicy::FailClosed, fail_on_attempt);
        let mut context = run_context(1);
        harness
            .start_run(&mut context)
            .await
            .expect("run must start");

        let invocation = harness
            .invoke_model_tracked(
                &mut context,
                ModelRequest::new(vec![ModelMessage::new(ModelRole::User, "prompt")]),
            )
            .await
            .expect("model call must complete");
        let result = match harness.prepare_action(&mut context, invocation).await {
            Ok(action) => harness.invoke_validated_action(&mut context, action).await,
            Err(ActionPreparationError::Harness(error)) => Err(error),
            Err(ActionPreparationError::Validation(_)) => {
                panic!("valid action must not fail structural validation")
            }
        };

        assert!(matches!(result, Err(HarnessError::Audit { .. })));
        assert_eq!(audit.attempt_count(), fail_on_attempt);
        assert_eq!(context.usage().tool_calls(), 0);
        assert_eq!(tool.invocation_count(), 0);
        assert!(!context.audit_degraded());
    }
}

#[tokio::test]
async fn fail_open_action_audit_failures_mark_degraded_and_continue() {
    for fail_on_attempt in [4, 5, 6] {
        let (harness, tool, audit) =
            governed_fixture_with_audit_failure(AuditFailurePolicy::FailOpen, fail_on_attempt);
        let mut context = run_context(1);
        harness
            .start_run(&mut context)
            .await
            .expect("run must start");

        let action = prepare(&harness, &mut context)
            .await
            .expect("fail-open preparation must continue");
        harness
            .invoke_validated_action(&mut context, action)
            .await
            .expect("fail-open binding must continue to execution");

        assert!(audit.attempt_count() >= fail_on_attempt);
        assert!(context.audit_degraded());
        assert_eq!(context.usage().tool_calls(), 1);
        assert_eq!(tool.invocation_count(), 1);
    }
}

#[tokio::test]
async fn malformed_and_schema_invalid_actions_consume_no_tool_budget() {
    for (response_text, parsed) in [
        (
            "```json\n{\"action\":{\"tool\":\"lookup\",\"arguments\":{}}}\n```",
            false,
        ),
        (
            r#"{"action":{"tool":"lookup","arguments":{"unknown":true}}}"#,
            true,
        ),
    ] {
        let fixture = governed_fixture(
            response_text,
            CapabilityKind::ReadOnly,
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            |definition| {
                FakeToolPort::succeeding(definition, ToolOutput::new(json!({"unused": true})))
            },
        );
        let mut context = run_context(1);
        fixture
            .harness
            .start_run(&mut context)
            .await
            .expect("run must start");

        let error = prepare(&fixture.harness, &mut context)
            .await
            .expect_err("invalid action must fail preparation");

        assert!(matches!(error, ActionPreparationError::Validation(_)));
        assert_eq!(context.usage().tool_calls(), 0);
        assert_eq!(fixture.tool.invocation_count(), 0);
        let events = fixture.audit.events();
        let proposed = events
            .iter()
            .position(|event| matches!(event.kind(), AgentEventKind::ActionProposed { .. }));
        let rejected = events
            .iter()
            .position(|event| matches!(event.kind(), AgentEventKind::ActionRejected { .. }))
            .expect("invalid attempt must be correlated by a rejection event");
        if parsed {
            assert!(proposed.is_some_and(|position| position < rejected));
        } else {
            assert!(proposed.is_none());
        }
    }
}

#[tokio::test]
async fn policy_denied_action_is_bound_but_uninvoked_and_unbudgeted() {
    for capability in [
        CapabilityKind::LocalWrite,
        CapabilityKind::ExternalWrite,
        CapabilityKind::Privileged,
    ] {
        let fixture = governed_fixture(
            r#"{"action":{"tool":"lookup","arguments":{}}}"#,
            capability,
            json!({"type": "object", "additionalProperties": false}),
            |definition| {
                FakeToolPort::succeeding(definition, ToolOutput::new(json!({"unused": true})))
            },
        );
        let mut context = run_context(1);
        fixture
            .harness
            .start_run(&mut context)
            .await
            .expect("run must start");
        let action = prepare(&fixture.harness, &mut context)
            .await
            .expect("action must validate before policy");
        let proposal_id = action.proposal_id();

        let error = fixture
            .harness
            .invoke_validated_action(&mut context, action)
            .await
            .expect_err("non-read-only action must be denied");

        match capability {
            CapabilityKind::LocalWrite => assert!(matches!(error, HarnessError::PolicyDenied(_))),
            CapabilityKind::ExternalWrite | CapabilityKind::Privileged => {
                assert!(matches!(
                    error,
                    HarnessError::CapabilityNotExecutable { .. }
                ))
            }
            CapabilityKind::ReadOnly => unreachable!("read-only is not part of this test"),
        }
        assert_eq!(context.usage().tool_calls(), 0);
        assert_eq!(fixture.tool.invocation_count(), 0);
        let events = fixture.audit.events();
        let bound_tool_call_id = events
            .iter()
            .find_map(|event| match event.kind() {
                AgentEventKind::ActionExecutionBound {
                    action_proposal_id,
                    tool_call_id,
                } if *action_proposal_id == proposal_id => Some(*tool_call_id),
                _ => None,
            })
            .expect("denied action must retain tool-call correlation");
        assert!(events.iter().any(|event| matches!(
            event.kind(),
            AgentEventKind::ToolPolicyDenied { tool_call_id, .. }
                if *tool_call_id == bound_tool_call_id
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind(), AgentEventKind::ToolInvocationStarted { .. }))
        );
    }
}

struct AlwaysAllowPolicy;

impl CapabilityPolicy for AlwaysAllowPolicy {
    fn authorize(&self, _capability: CapabilityKind) -> AuthorizationDecision {
        AuthorizationDecision::Allowed
    }
}

fn local_write_harness(
    contained: Option<Arc<FakeContainedToolPort>>,
    approval: Option<Arc<dyn ApprovalPort>>,
    max_approval_requests: u32,
    policy: Arc<dyn CapabilityPolicy>,
) -> (ExecutionHarness, Arc<InMemoryAuditSink>) {
    let model: Arc<dyn ModelPort> = Arc::new(FakeModelPort::scripted(vec![Ok(response(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
    ))]));
    let definition = definition_with_capability(
        "lookup",
        CapabilityKind::LocalWrite,
        json!({"type": "object", "additionalProperties": false}),
    );
    let mut registry = ToolRegistry::new();
    if let Some(tool) = contained {
        registry
            .register_contained(tool)
            .expect("contained local-write tool must register");
    } else {
        let direct: Arc<dyn ToolPort> = Arc::new(FakeToolPort::succeeding(
            definition,
            ToolOutput::new(json!({"unused": true})),
        ));
        registry
            .register(direct)
            .expect("direct local-write tool must register");
    }
    let audit = Arc::new(InMemoryAuditSink::new());
    let audit_sink: Arc<dyn AuditSink> = audit.clone();
    let harness = ExecutionHarness::new(
        model,
        registry,
        policy,
        audit_sink,
        HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(1))
            .expect("harness config must be valid"),
    );
    let harness = match approval {
        Some(port) => harness.with_approval_port(port),
        None => harness,
    };
    let _ = max_approval_requests;
    (harness, audit)
}

fn run_context_with_approval(tool_calls: u32, approval_requests: u32) -> RunContext {
    let budget = RunBudget::new(1, tool_calls, 1, Duration::from_secs(30))
        .expect("budget must be valid")
        .with_max_approval_requests(approval_requests);
    RunContext::new(RunId::new(), SessionId::new(), budget)
}

#[tokio::test]
async fn direct_local_write_cannot_request_approval_or_execute() {
    let (harness, audit) = local_write_harness(
        None,
        Some(Arc::new(ScriptedApprovalPort::approve_all())),
        1,
        Arc::new(M6ApprovalPolicy),
    );
    let mut context = run_context_with_approval(1, 1);
    harness.start_run(&mut context).await.expect("run starts");
    let action = prepare(&harness, &mut context)
        .await
        .expect("action validates");

    let error = harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("direct local-write cannot execute");

    assert!(matches!(error, HarnessError::ContainmentUnavailable { .. }));
    assert_eq!(context.usage().approval_requests(), 0);
    assert_eq!(context.usage().tool_calls(), 0);
    assert!(
        audit
            .events()
            .iter()
            .any(|event| matches!(event.kind(), AgentEventKind::ContainmentFailed { .. }))
    );
}

#[tokio::test]
async fn contained_local_write_granted_approval_executes_once() {
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition_with_capability(
            "lookup",
            CapabilityKind::LocalWrite,
            json!({"type": "object", "additionalProperties": false}),
        ),
        ToolOutput::new(json!({"ok": true})),
    ));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let (harness, audit) = local_write_harness(
        Some(Arc::clone(&contained)),
        Some(approval.clone()),
        1,
        Arc::new(M6ApprovalPolicy),
    );
    let mut context = run_context_with_approval(1, 1);
    harness.start_run(&mut context).await.expect("run starts");
    let action = prepare(&harness, &mut context)
        .await
        .expect("action validates");

    let result = harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect("approved contained local-write executes");

    assert!(matches!(result, ToolResult::Succeeded { .. }));
    assert_eq!(context.usage().approval_requests(), 1);
    assert_eq!(context.usage().tool_calls(), 1);
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 1);
    let events = audit.events();
    let approval_granted = events
        .iter()
        .position(|event| matches!(event.kind(), AgentEventKind::ApprovalGranted { .. }))
        .expect("grant must be audited");
    let tool_started = events
        .iter()
        .position(|event| matches!(event.kind(), AgentEventKind::ToolInvocationStarted { .. }))
        .expect("tool start must be audited");
    assert!(approval_granted < tool_started);
}

#[tokio::test]
async fn approval_denial_consumes_approval_budget_but_zero_tool_calls() {
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition_with_capability(
            "lookup",
            CapabilityKind::LocalWrite,
            json!({"type": "object", "additionalProperties": false}),
        ),
        ToolOutput::new(json!({"unused": true})),
    ));
    let approval = Arc::new(ScriptedApprovalPort::deny_all());
    let (harness, audit) = local_write_harness(
        Some(Arc::clone(&contained)),
        Some(approval.clone()),
        1,
        Arc::new(M6ApprovalPolicy),
    );
    let mut context = run_context_with_approval(1, 1);
    harness.start_run(&mut context).await.expect("run starts");
    let action = prepare(&harness, &mut context)
        .await
        .expect("action validates");

    let error = harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("denial fails closed");

    assert!(matches!(error, HarnessError::ApprovalDenied));
    assert_eq!(context.usage().approval_requests(), 1);
    assert_eq!(context.usage().tool_calls(), 0);
    assert_eq!(approval.invocation_count(), 1);
    assert_eq!(contained.invocation_count(), 0);
    assert!(
        audit
            .events()
            .iter()
            .any(|event| matches!(event.kind(), AgentEventKind::ApprovalDenied { .. }))
    );
}

#[tokio::test]
async fn zero_approval_budget_terminalizes_before_approval_port() {
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition_with_capability(
            "lookup",
            CapabilityKind::LocalWrite,
            json!({"type": "object", "additionalProperties": false}),
        ),
        ToolOutput::new(json!({"unused": true})),
    ));
    let approval = Arc::new(ScriptedApprovalPort::approve_all());
    let (harness, _audit) = local_write_harness(
        Some(Arc::clone(&contained)),
        Some(approval.clone()),
        0,
        Arc::new(M6ApprovalPolicy),
    );
    let mut context = run_context_with_approval(1, 0);
    harness.start_run(&mut context).await.expect("run starts");
    let action = prepare(&harness, &mut context)
        .await
        .expect("action validates");

    let error = harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("approval budget is zero");

    assert_eq!(
        error.budget_dimension(),
        Some(BudgetDimension::ApprovalRequests)
    );
    assert_eq!(approval.invocation_count(), 0);
    assert_eq!(contained.invocation_count(), 0);
    assert_eq!(context.usage().approval_requests(), 0);
    assert!(matches!(
        context.status(),
        RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ApprovalRequests
        })
    ));
}

#[tokio::test]
async fn faulty_allowed_local_write_policy_cannot_bypass_approval_requirement() {
    let contained = Arc::new(FakeContainedToolPort::succeeding(
        definition_with_capability(
            "lookup",
            CapabilityKind::LocalWrite,
            json!({"type": "object", "additionalProperties": false}),
        ),
        ToolOutput::new(json!({"unused": true})),
    ));
    let (harness, _audit) = local_write_harness(
        Some(Arc::clone(&contained)),
        Some(Arc::new(ScriptedApprovalPort::approve_all())),
        1,
        Arc::new(AlwaysAllowPolicy),
    );
    let mut context = run_context_with_approval(1, 1);
    harness.start_run(&mut context).await.expect("run starts");
    let action = prepare(&harness, &mut context)
        .await
        .expect("action validates");

    let error = harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("faulty allowed local-write cannot execute");

    assert!(matches!(error, HarnessError::ApprovalRequired));
    assert_eq!(context.usage().approval_requests(), 0);
    assert_eq!(context.usage().tool_calls(), 0);
    assert_eq!(contained.invocation_count(), 0);
}

#[tokio::test]
async fn mismatched_tool_result_call_id_is_adapter_failure() {
    let mismatched_id = ToolCallId::new();
    let fixture = governed_fixture(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
        CapabilityKind::ReadOnly,
        json!({"type": "object", "additionalProperties": false}),
        |definition| {
            FakeToolPort::scripted(
                definition,
                vec![Ok(ToolResult::Succeeded {
                    call_id: mismatched_id,
                    output: ToolOutput::new(json!({})),
                })],
            )
        },
    );
    let mut context = run_context(1);
    fixture
        .harness
        .start_run(&mut context)
        .await
        .expect("run must start");
    let action = prepare(&fixture.harness, &mut context)
        .await
        .expect("action must validate");

    let error = fixture
        .harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("mismatched result correlation must fail");

    assert_eq!(error, HarnessError::ToolPort(ToolPortError::AdapterFailure));
    assert_eq!(context.usage().tool_calls(), 1);
}

#[tokio::test]
async fn domain_failure_preserves_the_bound_tool_call_id() {
    let fixture = governed_fixture(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
        CapabilityKind::ReadOnly,
        json!({"type": "object", "additionalProperties": false}),
        |definition| {
            FakeToolPort::domain_failing(
                definition,
                ToolDomainFailure::new(ToolDomainFailureKind::NotFound),
            )
        },
    );
    let mut context = run_context(1);
    fixture
        .harness
        .start_run(&mut context)
        .await
        .expect("run must start");
    let action = prepare(&fixture.harness, &mut context)
        .await
        .expect("action must validate");

    let result = fixture
        .harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect("domain failure is a typed result");
    let bound_id = fixture
        .audit
        .events()
        .iter()
        .find_map(|event| match event.kind() {
            AgentEventKind::ActionExecutionBound { tool_call_id, .. } => Some(*tool_call_id),
            _ => None,
        })
        .expect("binding event must exist");

    assert_eq!(result.call_id(), bound_id);
    assert!(matches!(result, ToolResult::DomainFailure { .. }));
}

#[tokio::test]
async fn tool_budget_exhaustion_after_binding_terminalizes_without_invocation() {
    let fixture = governed_fixture(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
        CapabilityKind::ReadOnly,
        json!({"type": "object", "additionalProperties": false}),
        |definition| FakeToolPort::succeeding(definition, ToolOutput::new(json!({"unused": true}))),
    );
    let mut context = run_context(0);
    fixture
        .harness
        .start_run(&mut context)
        .await
        .expect("run must start");
    let action = prepare(&fixture.harness, &mut context)
        .await
        .expect("action must validate");

    let error = fixture
        .harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("zero tool budget must stop execution");

    assert!(matches!(error, HarnessError::BudgetExceeded(_)));
    assert_eq!(context.usage().tool_calls(), 0);
    assert_eq!(fixture.tool.invocation_count(), 0);
    assert_eq!(
        context.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::ToolCalls,
        })
    );
    assert!(
        fixture
            .audit
            .events()
            .iter()
            .any(|event| matches!(event.kind(), AgentEventKind::ActionExecutionBound { .. }))
    );
}

#[tokio::test]
async fn cancellation_before_governed_act_preserves_harness_authority() {
    let fixture = governed_fixture(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
        CapabilityKind::ReadOnly,
        json!({"type": "object", "additionalProperties": false}),
        |definition| FakeToolPort::succeeding(definition, ToolOutput::new(json!({"unused": true}))),
    );
    let mut context = run_context(1);
    let cancellation = fixture
        .harness
        .start_run(&mut context)
        .await
        .expect("run must start");
    let action = prepare(&fixture.harness, &mut context)
        .await
        .expect("action must validate");
    cancellation.request_cancel();

    let error = fixture
        .harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("cancellation must stop governed act");

    assert!(matches!(error, HarnessError::Cancelled { .. }));
    assert_eq!(
        context.status(),
        &RunStatus::Finished(RunOutcome::Cancelled)
    );
    assert_eq!(context.usage().tool_calls(), 0);
    assert_eq!(fixture.tool.invocation_count(), 0);
    assert!(
        !fixture
            .audit
            .events()
            .iter()
            .any(|event| matches!(event.kind(), AgentEventKind::ActionExecutionBound { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn elapsed_deadline_before_governed_act_preserves_harness_authority() {
    let fixture = governed_fixture(
        r#"{"action":{"tool":"lookup","arguments":{}}}"#,
        CapabilityKind::ReadOnly,
        json!({"type": "object", "additionalProperties": false}),
        |definition| FakeToolPort::succeeding(definition, ToolOutput::new(json!({"unused": true}))),
    );
    let mut context = run_context(1);
    fixture
        .harness
        .start_run(&mut context)
        .await
        .expect("run must start");
    let action = prepare(&fixture.harness, &mut context)
        .await
        .expect("action must validate");
    tokio::time::advance(Duration::from_secs(31)).await;

    let error = fixture
        .harness
        .invoke_validated_action(&mut context, action)
        .await
        .expect_err("elapsed deadline must stop governed act");

    assert!(matches!(error, HarnessError::DeadlineExceeded { .. }));
    assert_eq!(
        context.status(),
        &RunStatus::Finished(RunOutcome::BudgetExceeded {
            dimension: BudgetDimension::Elapsed,
        })
    );
    assert_eq!(context.usage().tool_calls(), 0);
    assert_eq!(fixture.tool.invocation_count(), 0);
    assert!(
        !fixture
            .audit
            .events()
            .iter()
            .any(|event| matches!(event.kind(), AgentEventKind::ActionExecutionBound { .. }))
    );
}

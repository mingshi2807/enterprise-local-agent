use std::{collections::HashSet, fmt, sync::Arc};

use agent_core::{
    ActionProposal, ActionProposalId, ActionRejectionReason, ModelCallId, ModelOutputPart,
    ModelResponse, ToolCall, ToolCallId, ToolInput, ToolName, ToolSchema,
};
use jsonschema::Validator;
use serde::{Deserialize, Deserializer, de};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::{HarnessError, ToolRegistry};

pub const MAX_PLANNING_RESPONSE_BYTES: usize = 16 * 1024;
pub const MAX_ACTION_TOOL_NAME_BYTES: usize = 128;
pub const MAX_ACTION_ARGUMENT_BYTES: usize = 8 * 1024;
pub const MAX_ACTION_ARGUMENT_DEPTH: usize = 8;
pub const MAX_ACTION_ARGUMENT_KEYS: usize = 64;
pub const MAX_ACTION_ARGUMENT_ARRAY_ELEMENTS: usize = 128;
pub const MAX_ACTION_ARGUMENT_NODES: usize = 256;
pub const MAX_TOOL_SCHEMA_BYTES: usize = 32 * 1024;
pub const MAX_TOOL_SCHEMA_DEPTH: usize = 16;
pub const MAX_TOOL_SCHEMA_KEYS: usize = 256;

const DRAFT_2020_12_URI: &str = "https://json-schema.org/draft/2020-12/schema";

/// Result of one completed model invocation with harness-owned correlation.
///
/// The response stays opaque on the action path and is consumed by
/// [`crate::ExecutionHarness::prepare_action`].
pub struct CompletedModelInvocation {
    model_call_id: ModelCallId,
    response: ModelResponse,
}

impl CompletedModelInvocation {
    pub(crate) const fn new(model_call_id: ModelCallId, response: ModelResponse) -> Self {
        Self {
            model_call_id,
            response,
        }
    }

    pub(crate) fn into_parts(self) -> (ModelCallId, ModelResponse) {
        (self.model_call_id, self.response)
    }

    pub(crate) fn into_response(self) -> ModelResponse {
        self.response
    }
}

/// In-process proof that an action proposal passed structural and schema
/// validation. Policy authorization still occurs afterwards.
///
/// It is deliberately single-use and process-local:
///
/// ```compile_fail
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<agent_harness::ValidatedAction>();
/// ```
///
/// ```compile_fail
/// fn requires_serialize<T: serde::Serialize>() {}
/// requires_serialize::<agent_harness::ValidatedAction>();
/// ```
pub struct ValidatedAction {
    proposal: ActionProposal,
}

impl ValidatedAction {
    #[must_use]
    pub const fn proposal_id(&self) -> ActionProposalId {
        self.proposal.id()
    }

    #[must_use]
    pub const fn tool_name(&self) -> &ToolName {
        self.proposal.tool_name()
    }

    pub(crate) fn into_tool_call(self, tool_call_id: ToolCallId) -> ToolCall {
        let (_, _, tool_name, arguments) = self.proposal.into_parts();
        ToolCall::new(tool_call_id, tool_name, arguments)
    }
}

impl fmt::Debug for ValidatedAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedAction")
            .field("proposal_id", &self.proposal.id())
            .field("tool_name", self.proposal.tool_name())
            .field("arguments", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("action proposal was rejected: {reason:?}")]
pub struct ActionValidationError {
    proposal_id: ActionProposalId,
    model_call_id: ModelCallId,
    reason: ActionRejectionReason,
}

impl ActionValidationError {
    pub(crate) const fn new(
        proposal_id: ActionProposalId,
        model_call_id: ModelCallId,
        reason: ActionRejectionReason,
    ) -> Self {
        Self {
            proposal_id,
            model_call_id,
            reason,
        }
    }

    #[must_use]
    pub const fn proposal_id(&self) -> ActionProposalId {
        self.proposal_id
    }

    #[must_use]
    pub(crate) const fn model_call_id(&self) -> ModelCallId {
        self.model_call_id
    }

    #[must_use]
    pub const fn reason(&self) -> ActionRejectionReason {
        self.reason
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ActionPreparationError {
    #[error(transparent)]
    Validation(#[from] ActionValidationError),
    #[error(transparent)]
    Harness(#[from] HarnessError),
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ToolSchemaRegistrationError {
    #[error("tool input schema exceeds the M5 size limit")]
    TooLarge,
    #[error("tool input schema exceeds the M5 complexity limits")]
    TooComplex,
    #[error("tool input schema is outside the supported M5 profile")]
    UnsupportedProfile,
    #[error("tool input schema could not be compiled")]
    CompilationFailed,
}

pub(crate) struct TextActionDecoder;

impl TextActionDecoder {
    pub(crate) fn decode(
        response: ModelResponse,
        proposal_id: ActionProposalId,
        model_call_id: ModelCallId,
    ) -> Result<ActionProposal, ActionValidationError> {
        let mut output = response.output().iter();
        let text = match (output.next(), output.next()) {
            (Some(ModelOutputPart::Text(text)), None) => text,
            (None, None) => {
                return Err(ActionValidationError::new(
                    proposal_id,
                    model_call_id,
                    ActionRejectionReason::EmptyOutput,
                ));
            }
            _ => {
                return Err(ActionValidationError::new(
                    proposal_id,
                    model_call_id,
                    ActionRejectionReason::UnsupportedOutputShape,
                ));
            }
        };

        if text.trim().is_empty() {
            return Err(ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::EmptyOutput,
            ));
        }
        if text.len() > MAX_PLANNING_RESPONSE_BYTES {
            return Err(ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::ResponseTooLarge,
            ));
        }

        let value = deserialize_unique_json(text).map_err(|_| {
            ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::MalformedEnvelope,
            )
        })?;
        let (tool_name, arguments) = decode_envelope(value)
            .map_err(|reason| ActionValidationError::new(proposal_id, model_call_id, reason))?;

        validate_argument_limits(&arguments)
            .map_err(|reason| ActionValidationError::new(proposal_id, model_call_id, reason))?;

        Ok(ActionProposal::new(
            proposal_id,
            model_call_id,
            tool_name,
            ToolInput::new(arguments),
        ))
    }
}

pub(crate) struct ActionValidator;

impl ActionValidator {
    pub(crate) fn validate(
        proposal: ActionProposal,
        registry: &ToolRegistry,
    ) -> Result<ValidatedAction, ActionValidationError> {
        let proposal_id = proposal.id();
        let model_call_id = proposal.source_model_call_id();
        if proposal.tool_name().as_str().len() > MAX_ACTION_TOOL_NAME_BYTES {
            return Err(ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::SecurityLimitExceeded,
            ));
        }
        if !proposal.arguments().as_value().is_object() {
            return Err(ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::ArgumentsSchemaMismatch,
            ));
        }
        validate_argument_limits(proposal.arguments().as_value())
            .map_err(|reason| ActionValidationError::new(proposal_id, model_call_id, reason))?;
        let binding = registry.get(proposal.tool_name()).ok_or_else(|| {
            ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::UnknownTool,
            )
        })?;
        if !binding
            .validator()
            .is_valid(proposal.arguments().as_value())
        {
            return Err(ActionValidationError::new(
                proposal_id,
                model_call_id,
                ActionRejectionReason::ArgumentsSchemaMismatch,
            ));
        }

        Ok(ValidatedAction { proposal })
    }
}

pub(crate) fn compile_tool_schema(
    schema: &ToolSchema,
) -> Result<Arc<Validator>, ToolSchemaRegistrationError> {
    let bytes = serde_json::to_vec(schema.as_value())
        .map_err(|_| ToolSchemaRegistrationError::CompilationFailed)?;
    if bytes.len() > MAX_TOOL_SCHEMA_BYTES {
        return Err(ToolSchemaRegistrationError::TooLarge);
    }
    validate_schema_complexity(schema.as_value())?;
    validate_schema_profile(schema.as_value(), true, 1)?;
    jsonschema::draft202012::options()
        .build(schema.as_value())
        .map(Arc::new)
        .map_err(|_| ToolSchemaRegistrationError::CompilationFailed)
}

fn deserialize_unique_json(text: &str) -> Result<Value, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = UniqueJson::deserialize(&mut deserializer)?.0;
    deserializer.end()?;
    Ok(value)
}

fn decode_envelope(value: Value) -> Result<(ToolName, Value), ActionRejectionReason> {
    let Value::Object(mut root) = value else {
        return Err(ActionRejectionReason::MalformedEnvelope);
    };
    if root.len() != 1 {
        return Err(ActionRejectionReason::MalformedEnvelope);
    }
    let Some(Value::Object(mut action)) = root.remove("action") else {
        return Err(ActionRejectionReason::MalformedEnvelope);
    };
    if action.len() != 2 {
        return Err(ActionRejectionReason::MalformedEnvelope);
    }
    let Some(Value::String(raw_tool_name)) = action.remove("tool") else {
        return Err(ActionRejectionReason::MalformedEnvelope);
    };
    let Some(arguments @ Value::Object(_)) = action.remove("arguments") else {
        return Err(ActionRejectionReason::MalformedEnvelope);
    };
    if !action.is_empty() {
        return Err(ActionRejectionReason::MalformedEnvelope);
    }
    if raw_tool_name.len() > MAX_ACTION_TOOL_NAME_BYTES {
        return Err(ActionRejectionReason::SecurityLimitExceeded);
    }
    let tool_name =
        ToolName::new(raw_tool_name).map_err(|_| ActionRejectionReason::InvalidToolName)?;
    Ok((tool_name, arguments))
}

fn validate_argument_limits(arguments: &Value) -> Result<(), ActionRejectionReason> {
    let serialized =
        serde_json::to_vec(arguments).map_err(|_| ActionRejectionReason::SecurityLimitExceeded)?;
    if serialized.len() > MAX_ACTION_ARGUMENT_BYTES {
        return Err(ActionRejectionReason::SecurityLimitExceeded);
    }

    let mut stack = vec![(arguments, 1_usize)];
    let mut object_keys = 0_usize;
    let mut array_elements = 0_usize;
    let mut nodes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_ACTION_ARGUMENT_NODES || depth > MAX_ACTION_ARGUMENT_DEPTH {
            return Err(ActionRejectionReason::SecurityLimitExceeded);
        }
        match value {
            Value::Object(object) => {
                object_keys = object_keys.saturating_add(object.len());
                if object_keys > MAX_ACTION_ARGUMENT_KEYS {
                    return Err(ActionRejectionReason::SecurityLimitExceeded);
                }
                stack.extend(object.values().map(|value| (value, depth + 1)));
            }
            Value::Array(array) => {
                array_elements = array_elements.saturating_add(array.len());
                if array_elements > MAX_ACTION_ARGUMENT_ARRAY_ELEMENTS {
                    return Err(ActionRejectionReason::SecurityLimitExceeded);
                }
                stack.extend(array.iter().map(|value| (value, depth + 1)));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

fn validate_schema_complexity(schema: &Value) -> Result<(), ToolSchemaRegistrationError> {
    let mut stack = vec![(schema, 1_usize)];
    let mut keys = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_TOOL_SCHEMA_DEPTH {
            return Err(ToolSchemaRegistrationError::TooComplex);
        }
        match value {
            Value::Object(object) => {
                keys = keys.saturating_add(object.len());
                if keys > MAX_TOOL_SCHEMA_KEYS {
                    return Err(ToolSchemaRegistrationError::TooComplex);
                }
                stack.extend(object.values().map(|value| (value, depth + 1)));
            }
            Value::Array(array) => {
                stack.extend(array.iter().map(|value| (value, depth + 1)));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

fn validate_schema_profile(
    schema: &Value,
    root: bool,
    depth: usize,
) -> Result<(), ToolSchemaRegistrationError> {
    if depth > MAX_TOOL_SCHEMA_DEPTH {
        return Err(ToolSchemaRegistrationError::TooComplex);
    }
    let Value::Object(object) = schema else {
        return Err(ToolSchemaRegistrationError::UnsupportedProfile);
    };

    if root {
        match object.get("$schema") {
            None => {}
            Some(Value::String(uri)) if uri == DRAFT_2020_12_URI => {}
            Some(_) => return Err(ToolSchemaRegistrationError::UnsupportedProfile),
        }
        if object.get("type").and_then(Value::as_str) != Some("object") {
            return Err(ToolSchemaRegistrationError::UnsupportedProfile);
        }
    } else if object.contains_key("$schema") {
        return Err(ToolSchemaRegistrationError::UnsupportedProfile);
    }

    for (keyword, value) in object {
        match keyword.as_str() {
            "$schema" if root => {}
            "title" | "description" => {
                if !value.is_string() {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            "type" => {
                let Some(kind) = value.as_str() else {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                };
                if !matches!(
                    kind,
                    "object" | "array" | "string" | "number" | "integer" | "boolean" | "null"
                ) {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            "enum" => {
                if !matches!(value, Value::Array(values) if !values.is_empty()) {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            "const" => {}
            "properties" => {
                let Value::Object(properties) = value else {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                };
                for property_schema in properties.values() {
                    validate_schema_profile(property_schema, false, depth + 1)?;
                }
            }
            "required" => {
                if !matches!(value, Value::Array(values) if values.iter().all(Value::is_string)) {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            "additionalProperties" => {
                if !value.is_boolean() {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            "minProperties" | "maxProperties" | "minItems" | "maxItems" | "minLength"
            | "maxLength" => {
                if value.as_u64().is_none() {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            "items" => validate_schema_profile(value, false, depth + 1)?,
            "minimum" | "maximum" => {
                if !value.is_number() {
                    return Err(ToolSchemaRegistrationError::UnsupportedProfile);
                }
            }
            _ => return Err(ToolSchemaRegistrationError::UnsupportedProfile),
        }
    }
    Ok(())
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonVisitor).map(Self)
    }
}

struct UniqueJsonVisitor;

impl<'de> de::Visitor<'de> for UniqueJsonVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value with unique object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        UniqueJson::deserialize(deserializer).map(|value| value.0)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueJson>()? {
            values.push(value.0);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let value = map.next_value::<UniqueJson>()?;
            values.insert(key, value.0);
        }
        Ok(Value::Object(values))
    }
}

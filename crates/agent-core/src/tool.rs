use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;
use thiserror::Error;

use crate::{CapabilityKind, ToolCallId};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(value: impl Into<String>) -> Result<Self, ToolNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ToolNameError::Empty);
        }
        if value.trim() != value {
            return Err(ToolNameError::SurroundingWhitespace);
        }
        if value.chars().any(char::is_control) {
            return Err(ToolNameError::ControlCharacter);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ToolNameError {
    #[error("tool name must not be empty")]
    Empty,
    #[error("tool name must not have surrounding whitespace")]
    SurroundingWhitespace,
    #[error("tool name must not contain control characters")]
    ControlCharacter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ToolSchema(Value);

impl ToolSchema {
    pub fn new(value: Value) -> Result<Self, ToolSchemaError> {
        if !value.is_object() {
            return Err(ToolSchemaError::MustBeObject);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ToolSchema {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ToolSchemaError {
    #[error("tool schema must be a JSON object")]
    MustBeObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolDefinition {
    name: ToolName,
    description: String,
    capability: CapabilityKind,
    input_schema: ToolSchema,
}

impl ToolDefinition {
    pub fn new(
        name: ToolName,
        description: impl Into<String>,
        capability: CapabilityKind,
        input_schema: ToolSchema,
    ) -> Result<Self, ToolDefinitionError> {
        let description = description.into();
        if description.trim().is_empty() {
            return Err(ToolDefinitionError::EmptyDescription);
        }

        Ok(Self {
            name,
            description,
            capability,
            input_schema,
        })
    }

    #[must_use]
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub const fn capability(&self) -> CapabilityKind {
        self.capability
    }

    #[must_use]
    pub const fn input_schema(&self) -> &ToolSchema {
        &self.input_schema
    }
}

impl<'de> Deserialize<'de> for ToolDefinition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Representation {
            name: ToolName,
            description: String,
            capability: CapabilityKind,
            input_schema: ToolSchema,
        }

        let representation = Representation::deserialize(deserializer)?;
        Self::new(
            representation.name,
            representation.description,
            representation.capability,
            representation.input_schema,
        )
        .map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ToolDefinitionError {
    #[error("tool description must not be empty")]
    EmptyDescription,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolInput(Value);

impl ToolInput {
    #[must_use]
    pub const fn new(value: Value) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.0
    }
}

impl fmt::Debug for ToolInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ToolInput([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolOutput(Value);

impl ToolOutput {
    #[must_use]
    pub const fn new(value: Value) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_value(&self) -> &Value {
        &self.0
    }
}

impl fmt::Debug for ToolOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ToolOutput([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    id: ToolCallId,
    name: ToolName,
    input: ToolInput,
}

impl ToolCall {
    #[must_use]
    pub const fn new(id: ToolCallId, name: ToolName, input: ToolInput) -> Self {
        Self { id, name, input }
    }

    #[must_use]
    pub const fn id(&self) -> ToolCallId {
        self.id
    }

    #[must_use]
    pub const fn name(&self) -> &ToolName {
        &self.name
    }

    #[must_use]
    pub const fn input(&self) -> &ToolInput {
        &self.input
    }
}

impl fmt::Debug for ToolCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolCall")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("input", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResult {
    Succeeded {
        call_id: ToolCallId,
        output: ToolOutput,
    },
    DomainFailure {
        call_id: ToolCallId,
        failure: ToolDomainFailure,
    },
}

impl fmt::Debug for ToolResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded { call_id, .. } => formatter
                .debug_struct("ToolResult::Succeeded")
                .field("call_id", call_id)
                .field("output", &"[REDACTED]")
                .finish(),
            Self::DomainFailure { call_id, failure } => formatter
                .debug_struct("ToolResult::DomainFailure")
                .field("call_id", call_id)
                .field("failure", failure)
                .finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDomainFailure {
    kind: ToolDomainFailureKind,
}

impl ToolDomainFailure {
    #[must_use]
    pub const fn new(kind: ToolDomainFailureKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(&self) -> ToolDomainFailureKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDomainFailureKind {
    InvalidInput,
    Rejected,
    NotFound,
    Conflict,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> ToolSchema {
        ToolSchema::new(serde_json::json!({"type": "object"})).expect("object schema must be valid")
    }

    #[test]
    fn tool_metadata_is_validated_during_construction() {
        assert_eq!(ToolName::new(""), Err(ToolNameError::Empty));
        assert_eq!(
            ToolName::new(" lookup"),
            Err(ToolNameError::SurroundingWhitespace)
        );
        assert_eq!(
            ToolSchema::new(serde_json::json!([])),
            Err(ToolSchemaError::MustBeObject)
        );

        let name = ToolName::new("lookup").expect("tool name must be valid");
        assert_eq!(
            ToolDefinition::new(name, " ", CapabilityKind::ReadOnly, schema()),
            Err(ToolDefinitionError::EmptyDescription)
        );
    }

    #[test]
    fn payload_debug_output_is_redacted() {
        let call = ToolCall::new(
            ToolCallId::new(),
            ToolName::new("lookup").expect("tool name must be valid"),
            ToolInput::new(serde_json::json!({"secret": "do-not-log"})),
        );
        let result = ToolResult::Succeeded {
            call_id: call.id(),
            output: ToolOutput::new(serde_json::json!({"secret": "also-do-not-log"})),
        };

        assert!(!format!("{call:?}").contains("do-not-log"));
        assert!(!format!("{result:?}").contains("also-do-not-log"));
    }

    #[test]
    fn invalid_metadata_cannot_enter_through_deserialization() {
        let json = r#"{"name":"lookup","description":"","capability":"read_only","input_schema":{"type":"object"}}"#;

        let error =
            serde_json::from_str::<ToolDefinition>(json).expect_err("empty description must fail");

        assert!(error.to_string().contains("description"));
    }
}

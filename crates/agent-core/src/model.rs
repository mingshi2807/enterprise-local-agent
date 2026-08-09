use serde::{Deserialize, Serialize};

use crate::ToolCall;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessage {
    role: ModelRole,
    content: String,
}

impl ModelMessage {
    #[must_use]
    pub fn new(role: ModelRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    #[must_use]
    pub const fn role(&self) -> ModelRole {
        self.role
    }

    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequest {
    messages: Vec<ModelMessage>,
}

impl ModelRequest {
    #[must_use]
    pub fn new(messages: Vec<ModelMessage>) -> Self {
        Self { messages }
    }

    #[must_use]
    pub fn messages(&self) -> &[ModelMessage] {
        &self.messages
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelOutputPart {
    Text(String),
    ToolCall(ToolCall),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResponse {
    output: Vec<ModelOutputPart>,
    token_usage: Option<TokenUsage>,
}

impl ModelResponse {
    #[must_use]
    pub fn new(output: Vec<ModelOutputPart>, token_usage: Option<TokenUsage>) -> Self {
        Self {
            output,
            token_usage,
        }
    }

    #[must_use]
    pub fn output(&self) -> &[ModelOutputPart] {
        &self.output
    }

    #[must_use]
    pub const fn token_usage(&self) -> Option<TokenUsage> {
        self.token_usage
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

impl TokenUsage {
    #[must_use]
    pub const fn new(input_tokens: Option<u64>, output_tokens: Option<u64>) -> Self {
        Self {
            input_tokens,
            output_tokens,
        }
    }

    #[must_use]
    pub const fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    #[must_use]
    pub const fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCallId, ToolInput, ToolName};

    #[test]
    fn response_supports_text_and_provider_neutral_tool_calls() {
        let tool_name = ToolName::new("inventory_lookup").expect("tool name must be valid");
        let call = ToolCall::new(
            ToolCallId::new(),
            tool_name,
            ToolInput::new(serde_json::json!({"sku": "A-1"})),
        );
        let response = ModelResponse::new(
            vec![
                ModelOutputPart::Text("Checking inventory".to_owned()),
                ModelOutputPart::ToolCall(call),
            ],
            Some(TokenUsage::new(Some(7), None)),
        );

        assert_eq!(response.output().len(), 2);
        assert_eq!(
            response
                .token_usage()
                .and_then(|usage| usage.input_tokens()),
            Some(7)
        );
        assert_eq!(
            response
                .token_usage()
                .and_then(|usage| usage.output_tokens()),
            None
        );
    }
}

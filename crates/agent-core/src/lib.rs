//! Provider-independent domain values for the enterprise agent runtime.

mod budget;
mod capability;
mod event;
mod ids;
mod model;
mod run;
mod tool;

pub use budget::{BudgetDimension, BudgetError, BudgetUsage, RunBudget};
pub use capability::CapabilityKind;
pub use event::{
    AgentEvent, AgentEventKind, CURRENT_EVENT_SCHEMA_VERSION, EventSchemaVersion, EventSequence,
};
pub use ids::{RunId, SessionId, ToolCallId};
pub use model::{
    ModelMessage, ModelOutputPart, ModelRequest, ModelResponse, ModelRole, TokenUsage,
};
pub use run::{RunFailureKind, RunOutcome, RunStateError, RunStatus};
pub use tool::{
    ToolCall, ToolDefinition, ToolDefinitionError, ToolDomainFailure, ToolDomainFailureKind,
    ToolInput, ToolName, ToolNameError, ToolOutput, ToolResult, ToolSchema, ToolSchemaError,
};

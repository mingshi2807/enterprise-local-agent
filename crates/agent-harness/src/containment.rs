use agent_core::{ToolCall, ToolDefinition, ToolInput, ToolResult};
use thiserror::Error;

use crate::{ApprovalPreview, ApprovalPreviewError, PortFuture};

/// Trusted contained execution adapter contract.
///
/// Implementing this trait is a trusted adapter assertion. The Rust trait
/// itself does not prove OS isolation. M6 ships no production containment
/// adapter; deterministic fakes are test/demo-only.
pub trait ContainedToolPort: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn approval_preview(&self, input: &ToolInput) -> Result<ApprovalPreview, ContainmentPortError>;

    fn invoke_contained<'a>(
        &'a self,
        call: ToolCall,
    ) -> PortFuture<'a, Result<ToolResult, ContainmentPortError>>;
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ContainmentPortError {
    #[error("contained executor is unavailable")]
    Unavailable,
    #[error("approval preview was rejected")]
    PreviewRejected,
    #[error("contained executor failed")]
    Infrastructure,
}

impl From<ApprovalPreviewError> for ContainmentPortError {
    fn from(_: ApprovalPreviewError) -> Self {
        Self::PreviewRejected
    }
}

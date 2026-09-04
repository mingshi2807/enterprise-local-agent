use agent_core::{ToolCall, ToolDefinition, ToolInput, ToolResult};
use thiserror::Error;

use crate::{ApprovalPreview, ApprovalPreviewError, PortFuture};

/// Trusted contained execution adapter contract.
///
/// Implementing this trait is a trusted adapter assertion. The Rust trait
/// itself does not prove OS isolation. Adapters must separately probe and test
/// every containment guarantee they claim; deterministic fakes are
/// test/demo-only.
pub trait ContainedToolPort: Send + Sync {
    fn definition(&self) -> &ToolDefinition;

    fn approval_preview(&self, input: &ToolInput) -> Result<ApprovalPreview, ContainmentPortError>;

    fn start_contained(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ContainedInvocation>, ContainmentPortError>;
}

/// One started contained invocation whose process lifetime is adapter-owned.
///
/// The harness explicitly terminates and reaps this handle when cancellation
/// or a deadline wins. Implementations must not return from
/// `terminate_and_reap` while child processes remain alive.
pub trait ContainedInvocation: Send {
    fn wait<'a>(&'a mut self) -> PortFuture<'a, Result<ToolResult, ContainmentPortError>>;

    fn terminate_and_reap<'a>(&'a mut self) -> PortFuture<'a, Result<(), ContainmentPortError>>;
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

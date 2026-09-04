use agent_core::ToolCallId;
use serde::{Deserialize, Serialize};

pub(crate) const PROTOCOL_VERSION: u16 = 1;
pub(crate) const MAX_PROTOCOL_FRAME_BYTES: usize = 8 * 1024;
pub(crate) const MAX_RESPONSE_FRAME_BYTES: usize = 4 * 1024;
pub const MAX_CONTENT_BYTES: usize = 4 * 1024;
pub const MAX_RELATIVE_PATH_BYTES: usize = 240;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerRequest {
    Probe {
        version: u16,
        host_namespaces: NamespaceIds,
        host_probe_port: u16,
    },
    WorkspaceWriteFile {
        version: u16,
        tool_call_id: ToolCallId,
        basename: String,
        content: String,
    },
    #[cfg(feature = "certification-hooks")]
    CertificationHold { version: u16 },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NamespaceIds {
    pub user: u64,
    pub mount: u64,
    pub pid: u64,
    pub network: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LandlockState {
    FullyEnforced,
    PartiallyEnforced,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerResponse {
    Probe {
        version: u16,
        namespaces_isolated: bool,
        network_isolated: bool,
        environment_isolated: bool,
        descriptors_isolated: bool,
        home_isolated: bool,
        no_new_privileges: bool,
        capabilities_dropped: bool,
        openat2_enforced: bool,
        landlock: LandlockState,
    },
    WorkspaceWriteFile {
        version: u16,
        tool_call_id: ToolCallId,
        result: WorkerWriteResult,
    },
    ProtocolError {
        version: u16,
        code: WorkerErrorCode,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerWriteResult {
    Written { bytes_written: u32 },
    Failed { code: WorkerErrorCode },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerErrorCode {
    InvalidInput,
    Rejected,
    NotFound,
    Conflict,
    IoFailure,
    Unsupported,
}

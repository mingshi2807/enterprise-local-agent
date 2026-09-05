//! Fixed-operation adapters for the two M8 enterprise knowledge backends.

mod ocpp;
mod standards;

pub use ocpp::{OcppApiConfig, OcppApiConfigError, OcppKnowledgeAdapter};
pub use standards::{StandardsMcpConfig, StandardsMcpConfigError, StandardsMcpKnowledgeAdapter};

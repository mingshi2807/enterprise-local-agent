//! Provider-neutral authenticated identity and service authorization contracts.

use std::{collections::BTreeSet, future::Future, pin::Pin};

use agent_core::{DurableApprovalWaitId, PrincipalId, RunId, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_PRINCIPAL_ROLES: usize = 3;
pub const MAX_WORKFLOW_ID_BYTES: usize = 96;
pub const MAX_SESSION_PAGE_ITEMS: u16 = 256;
pub const AUTHORIZATION_POLICY_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    Service,
    LocalProcess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    User,
    Approver,
    Operator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationClass {
    UnixPeer,
    LoopbackBearer,
}

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedPrincipal {
    id: PrincipalId,
    kind: PrincipalKind,
    roles: BTreeSet<PrincipalRole>,
    authentication_class: AuthenticationClass,
}

impl VerifiedPrincipal {
    pub fn new(
        id: PrincipalId,
        kind: PrincipalKind,
        roles: impl IntoIterator<Item = PrincipalRole>,
        authentication_class: AuthenticationClass,
    ) -> Result<Self, IdentityError> {
        let roles = roles.into_iter().collect::<BTreeSet<_>>();
        if roles.is_empty() || roles.len() > MAX_PRINCIPAL_ROLES {
            return Err(IdentityError::InvalidPrincipal);
        }
        Ok(Self {
            id,
            kind,
            roles,
            authentication_class,
        })
    }

    #[must_use]
    pub const fn id(&self) -> &PrincipalId {
        &self.id
    }

    #[must_use]
    pub const fn kind(&self) -> PrincipalKind {
        self.kind
    }

    #[must_use]
    pub const fn authentication_class(&self) -> AuthenticationClass {
        self.authentication_class
    }

    #[must_use]
    pub fn has_role(&self, role: PrincipalRole) -> bool {
        self.roles.contains(&role)
    }
}

impl std::fmt::Debug for VerifiedPrincipal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedPrincipal")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("role_count", &self.roles.len())
            .field("authentication_class", &self.authentication_class)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSeparation {
    RequesterMayApprove,
    RequesterMustDiffer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationAction {
    CreateSession,
    ListSessions,
    ReadSessionHistory,
    StartWorkflow,
    ReadRun,
    ReadEvents,
    CancelRun,
    ListWaiting,
    ReadApprovalPreview,
    DecideApproval,
    ResumeRun,
    AbortWaiting,
    ReadRuntimeStatus,
    ReadOperations,
    InspectReconciliation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizationResource<'a> {
    Global,
    OwnedSession {
        owner: &'a PrincipalId,
    },
    Workflow {
        workflow_id: &'a str,
    },
    OwnedRun {
        workflow_id: &'a str,
        owner: &'a PrincipalId,
    },
    Approval {
        workflow_id: &'a str,
        owner: &'a PrincipalId,
        requester: &'a PrincipalId,
    },
    LegacyUnowned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Granted,
    Denied,
}

pub trait ServiceAuthorizationPolicy: Send + Sync {
    fn authorize(
        &self,
        principal: &VerifiedPrincipal,
        action: AuthorizationAction,
        resource: AuthorizationResource<'_>,
    ) -> AuthorizationDecision;

    fn policy_version(&self) -> u32;
    fn fingerprint(&self) -> AuthorizationPolicyFingerprint;
}

#[derive(Clone, PartialEq, Eq)]
pub struct DefaultDenyServiceAuthorizationPolicy {
    policy_version: u32,
    fingerprint: AuthorizationPolicyFingerprint,
    workflows: BTreeSet<String>,
    approval_separation: ApprovalSeparation,
}

impl DefaultDenyServiceAuthorizationPolicy {
    pub fn new(
        policy_version: u32,
        fingerprint: AuthorizationPolicyFingerprint,
        workflows: impl IntoIterator<Item = String>,
        approval_separation: ApprovalSeparation,
    ) -> Result<Self, IdentityError> {
        if policy_version == 0 {
            return Err(IdentityError::InvalidPolicy);
        }
        let workflows = workflows.into_iter().collect::<BTreeSet<_>>();
        if workflows.is_empty()
            || workflows.len() > 32
            || workflows
                .iter()
                .any(|workflow| !valid_workflow_id(workflow))
        {
            return Err(IdentityError::InvalidPolicy);
        }
        Ok(Self {
            policy_version,
            fingerprint,
            workflows,
            approval_separation,
        })
    }

    fn workflow_allowed(&self, workflow_id: &str) -> bool {
        self.workflows.contains(workflow_id)
    }
}

impl ServiceAuthorizationPolicy for DefaultDenyServiceAuthorizationPolicy {
    fn authorize(
        &self,
        principal: &VerifiedPrincipal,
        action: AuthorizationAction,
        resource: AuthorizationResource<'_>,
    ) -> AuthorizationDecision {
        use AuthorizationAction as A;
        use AuthorizationResource as R;
        use PrincipalRole as Role;

        let granted = match (action, resource) {
            (A::CreateSession, R::Global) => principal.has_role(Role::User),
            (A::ListSessions, R::Global) => principal.has_role(Role::User),
            (A::ReadSessionHistory, R::OwnedSession { owner }) => {
                principal.has_role(Role::User) && principal.id() == owner
            }
            (A::StartWorkflow, R::Workflow { workflow_id }) => {
                principal.has_role(Role::User) && self.workflow_allowed(workflow_id)
            }
            (
                A::ReadRun | A::ReadEvents | A::CancelRun | A::ResumeRun | A::AbortWaiting,
                R::OwnedRun { workflow_id, owner },
            ) => {
                principal.has_role(Role::User)
                    && principal.id() == owner
                    && self.workflow_allowed(workflow_id)
            }
            (A::ListWaiting, R::Global) => principal.has_role(Role::Approver),
            (A::ReadRuntimeStatus, R::Global) => {
                principal.has_role(Role::User)
                    || principal.has_role(Role::Approver)
                    || principal.has_role(Role::Operator)
            }
            (
                A::ReadApprovalPreview | A::DecideApproval,
                R::Approval {
                    workflow_id,
                    requester,
                    ..
                },
            ) => {
                principal.has_role(Role::Approver)
                    && self.workflow_allowed(workflow_id)
                    && (self.approval_separation == ApprovalSeparation::RequesterMayApprove
                        || principal.id() != requester)
            }
            (A::ReadOperations | A::InspectReconciliation, R::Global) => {
                principal.has_role(Role::Operator)
            }
            _ => false,
        };
        if granted {
            AuthorizationDecision::Granted
        } else {
            AuthorizationDecision::Denied
        }
    }

    fn policy_version(&self) -> u32 {
        self.policy_version
    }

    fn fingerprint(&self) -> AuthorizationPolicyFingerprint {
        self.fingerprint
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthorizationPolicyFingerprint([u8; 32]);

impl AuthorizationPolicyFingerprint {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl std::fmt::Debug for AuthorizationPolicyFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthorizationPolicyFingerprint([REDACTED])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRunAuthorization {
    owner: PrincipalId,
    requester: PrincipalId,
    workflow_id: String,
    policy_version: u32,
    policy_fingerprint: AuthorizationPolicyFingerprint,
}

impl DurableRunAuthorization {
    pub fn new(
        owner: PrincipalId,
        requester: PrincipalId,
        workflow_id: String,
        policy_version: u32,
        policy_fingerprint: AuthorizationPolicyFingerprint,
    ) -> Result<Self, IdentityError> {
        if policy_version == 0 || !valid_workflow_id(&workflow_id) {
            return Err(IdentityError::InvalidPolicy);
        }
        Ok(Self {
            owner,
            requester,
            workflow_id,
            policy_version,
            policy_fingerprint,
        })
    }

    #[must_use]
    pub const fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    #[must_use]
    pub const fn requester(&self) -> &PrincipalId {
        &self.requester
    }
    #[must_use]
    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }
    #[must_use]
    pub const fn policy_version(&self) -> u32 {
        self.policy_version
    }
    #[must_use]
    pub const fn policy_fingerprint(&self) -> AuthorizationPolicyFingerprint {
        self.policy_fingerprint
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOwnershipRecord {
    session_id: SessionId,
    owner: PrincipalId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPageCursor {
    session_id: SessionId,
}

impl SessionPageCursor {
    #[must_use]
    pub const fn new(session_id: SessionId) -> Self {
        Self { session_id }
    }

    #[must_use]
    pub const fn session_id(self) -> SessionId {
        self.session_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionOwnershipPage {
    items: Vec<SessionOwnershipRecord>,
    next: Option<SessionPageCursor>,
}

impl SessionOwnershipPage {
    #[must_use]
    pub const fn new(items: Vec<SessionOwnershipRecord>, next: Option<SessionPageCursor>) -> Self {
        Self { items, next }
    }

    #[must_use]
    pub fn items(&self) -> &[SessionOwnershipRecord] {
        &self.items
    }

    #[must_use]
    pub const fn next(&self) -> Option<SessionPageCursor> {
        self.next
    }
}

impl SessionOwnershipRecord {
    #[must_use]
    pub const fn new(session_id: SessionId, owner: PrincipalId) -> Self {
        Self { session_id, owner }
    }
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }
    #[must_use]
    pub const fn owner(&self) -> &PrincipalId {
        &self.owner
    }
}

pub type IdentityFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait SessionOwnershipPort: Send + Sync {
    fn create_session<'a>(
        &'a self,
        record: &'a SessionOwnershipRecord,
    ) -> IdentityFuture<'a, Result<(), IdentityStoreError>>;
    fn load_session<'a>(
        &'a self,
        session_id: SessionId,
    ) -> IdentityFuture<'a, Result<Option<SessionOwnershipRecord>, IdentityStoreError>>;
    fn list_sessions<'a>(
        &'a self,
        owner: &'a PrincipalId,
        after: Option<SessionPageCursor>,
        limit: u16,
    ) -> IdentityFuture<'a, Result<SessionOwnershipPage, IdentityStoreError>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityAuditPhase {
    AuthorizationGranted,
    MutationRequested,
    MutationCommitted,
    MutationFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityAuditEvent {
    pub phase: SecurityAuditPhase,
    pub actor: PrincipalId,
    pub action: AuthorizationAction,
    pub session_id: Option<SessionId>,
    pub run_id: Option<RunId>,
    pub wait_id: Option<DurableApprovalWaitId>,
}

pub trait SecurityAuditPort: Send + Sync {
    fn record<'a>(
        &'a self,
        event: &'a SecurityAuditEvent,
    ) -> IdentityFuture<'a, Result<(), SecurityAuditError>>;
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("verified principal is invalid")]
    InvalidPrincipal,
    #[error("authorization policy is invalid")]
    InvalidPolicy,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum IdentityStoreError {
    #[error("identity store is unavailable")]
    Unavailable,
    #[error("identity store rejected conflicting state")]
    Conflict,
    #[error("identity store is corrupt")]
    Corrupt,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("security audit failed")]
pub struct SecurityAuditError;

fn valid_workflow_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_WORKFLOW_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(feature = "test-support")]
pub mod testing {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct InMemorySecurityAudit {
        events: Mutex<Vec<SecurityAuditEvent>>,
        fail: bool,
    }

    impl InMemorySecurityAudit {
        #[must_use]
        pub const fn failing() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        #[must_use]
        pub fn events(&self) -> Vec<SecurityAuditEvent> {
            self.events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default()
        }
    }

    impl SecurityAuditPort for InMemorySecurityAudit {
        fn record<'a>(
            &'a self,
            event: &'a SecurityAuditEvent,
        ) -> IdentityFuture<'a, Result<(), SecurityAuditError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(SecurityAuditError);
                }
                self.events
                    .lock()
                    .map_err(|_| SecurityAuditError)?
                    .push(event.clone());
                Ok(())
            })
        }
    }

    pub fn principal(id: &str, roles: &[PrincipalRole]) -> VerifiedPrincipal {
        VerifiedPrincipal::new(
            PrincipalId::new(id).expect("test principal id"),
            PrincipalKind::Human,
            roles.iter().copied(),
            AuthenticationClass::UnixPeer,
        )
        .expect("test principal")
    }

    pub fn policy(
        workflows: impl IntoIterator<Item = String>,
        separation: ApprovalSeparation,
    ) -> DefaultDenyServiceAuthorizationPolicy {
        DefaultDenyServiceAuthorizationPolicy::new(
            1,
            AuthorizationPolicyFingerprint::from_bytes([1; 32]),
            workflows,
            separation,
        )
        .expect("test policy")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(id: &str, roles: &[PrincipalRole]) -> VerifiedPrincipal {
        VerifiedPrincipal::new(
            PrincipalId::new(id).expect("principal id"),
            PrincipalKind::Human,
            roles.iter().copied(),
            AuthenticationClass::UnixPeer,
        )
        .expect("principal")
    }

    fn policy(separation: ApprovalSeparation) -> DefaultDenyServiceAuthorizationPolicy {
        DefaultDenyServiceAuthorizationPolicy::new(
            1,
            AuthorizationPolicyFingerprint::from_bytes([7; 32]),
            ["workflow-v1".to_owned()],
            separation,
        )
        .expect("policy")
    }

    #[test]
    fn default_deny_separates_user_approver_and_operator() {
        let policy = policy(ApprovalSeparation::RequesterMustDiffer);
        let requester = principal("requester", &[PrincipalRole::User]);
        let approver = principal("approver", &[PrincipalRole::Approver]);
        let operator = principal("operator", &[PrincipalRole::Operator]);

        assert_eq!(
            policy.authorize(
                &requester,
                AuthorizationAction::StartWorkflow,
                AuthorizationResource::Workflow {
                    workflow_id: "workflow-v1"
                }
            ),
            AuthorizationDecision::Granted
        );
        assert_eq!(
            policy.authorize(
                &operator,
                AuthorizationAction::StartWorkflow,
                AuthorizationResource::Workflow {
                    workflow_id: "workflow-v1"
                }
            ),
            AuthorizationDecision::Denied
        );
        assert_eq!(
            policy.authorize(
                &approver,
                AuthorizationAction::DecideApproval,
                AuthorizationResource::Approval {
                    workflow_id: "workflow-v1",
                    owner: requester.id(),
                    requester: requester.id(),
                }
            ),
            AuthorizationDecision::Granted
        );
    }

    #[test]
    fn requester_must_differ_is_enforced() {
        let policy = policy(ApprovalSeparation::RequesterMustDiffer);
        let both = principal("both", &[PrincipalRole::User, PrincipalRole::Approver]);
        assert_eq!(
            policy.authorize(
                &both,
                AuthorizationAction::DecideApproval,
                AuthorizationResource::Approval {
                    workflow_id: "workflow-v1",
                    owner: both.id(),
                    requester: both.id(),
                }
            ),
            AuthorizationDecision::Denied
        );
    }

    #[test]
    fn runtime_status_is_available_to_authenticated_roles_only() {
        let policy = policy(ApprovalSeparation::RequesterMayApprove);
        for role in [
            PrincipalRole::User,
            PrincipalRole::Approver,
            PrincipalRole::Operator,
        ] {
            let actor = principal("status-reader", &[role]);
            assert_eq!(
                policy.authorize(
                    &actor,
                    AuthorizationAction::ReadRuntimeStatus,
                    AuthorizationResource::Global,
                ),
                AuthorizationDecision::Granted
            );
        }
    }

    #[test]
    fn verified_principal_debug_is_bounded() {
        let principal = principal("secret-subject", &[PrincipalRole::User]);
        let debug = format!("{principal:?}");
        assert!(!debug.contains("secret-subject"));
        assert!(!debug.contains("token"));
    }
}

//! Local XChaCha20-Poly1305 sealing for the narrow M10 LocalWrite capsule.

use agent_harness::{
    ActionSealBinding, ActionSealError, ActionSealPort, LocalWriteActionCapsuleV1,
    MAX_SEAL_KEY_ID_BYTES, SealedLocalWriteAction,
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, KeyInit, Payload},
};
use zeroize::Zeroizing;

pub const LOCAL_ACTION_SEAL_KEY_BYTES: usize = 32;

pub struct LocalActionSealer {
    key_id: String,
    key: Zeroizing<[u8; LOCAL_ACTION_SEAL_KEY_BYTES]>,
}

impl LocalActionSealer {
    pub fn new(
        key_id: impl Into<String>,
        key: [u8; LOCAL_ACTION_SEAL_KEY_BYTES],
    ) -> Result<Self, ActionSealError> {
        let key_id = key_id.into();
        if key_id.is_empty()
            || key_id.len() > MAX_SEAL_KEY_ID_BYTES
            || key_id.chars().any(|character| {
                !character.is_ascii_alphanumeric() && character != '-' && character != '_'
            })
        {
            return Err(ActionSealError::KeyUnavailable);
        }
        Ok(Self {
            key_id,
            key: Zeroizing::new(key),
        })
    }

    fn cipher(&self) -> Result<XChaCha20Poly1305, ActionSealError> {
        XChaCha20Poly1305::new_from_slice(self.key.as_ref())
            .map_err(|_| ActionSealError::KeyUnavailable)
    }
}

impl ActionSealPort for LocalActionSealer {
    fn seal_local_write(
        &self,
        binding: &ActionSealBinding,
        action: &LocalWriteActionCapsuleV1,
    ) -> Result<SealedLocalWriteAction, ActionSealError> {
        let plaintext = Zeroizing::new(
            serde_json::to_vec(action).map_err(|_| ActionSealError::EncodingFailed)?,
        );
        let aad = serde_json::to_vec(binding).map_err(|_| ActionSealError::EncodingFailed)?;
        let nonce = XNonce::generate();
        let ciphertext = self
            .cipher()?
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_slice(),
                    aad: &aad,
                },
            )
            .map_err(|_| ActionSealError::AuthenticationFailed)?;
        SealedLocalWriteAction::new(self.key_id.clone(), nonce.to_vec(), ciphertext)
    }

    fn open_local_write(
        &self,
        binding: &ActionSealBinding,
        sealed: &SealedLocalWriteAction,
    ) -> Result<LocalWriteActionCapsuleV1, ActionSealError> {
        sealed.validate()?;
        if sealed.key_id() != self.key_id || sealed.version() != binding.capsule_version() {
            return Err(ActionSealError::KeyUnavailable);
        }
        let nonce: [u8; 24] = sealed
            .nonce()
            .try_into()
            .map_err(|_| ActionSealError::InvalidCapsule)?;
        let aad = serde_json::to_vec(binding).map_err(|_| ActionSealError::EncodingFailed)?;
        let plaintext = Zeroizing::new(
            self.cipher()?
                .decrypt(
                    &nonce.into(),
                    Payload {
                        msg: sealed.ciphertext(),
                        aad: &aad,
                    },
                )
                .map_err(|_| ActionSealError::AuthenticationFailed)?,
        );
        let decoded: LocalWriteActionCapsuleV1 =
            serde_json::from_slice(&plaintext).map_err(|_| ActionSealError::InvalidCapsule)?;
        LocalWriteActionCapsuleV1::new(
            decoded.relative_path().to_owned(),
            decoded.content().to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use agent_core::{
        ActionDigest, ActionProposalId, ApprovalRequestId, DurableApprovalWaitId,
        GraphNodeAttemptId, GraphNodeId, PrincipalId, RunId, SessionId, ToolCallId,
        ToolContractDigest, WorkspaceBindingId,
    };
    use agent_harness::{ActionSealBinding, ActionSealPort, GraphDefinitionDigest, RunKey};

    use super::*;

    fn binding() -> ActionSealBinding {
        ActionSealBinding::new(
            RunKey::new(RunId::new(), SessionId::new()),
            10,
            GraphDefinitionDigest::from_bytes([1; 32]),
            GraphNodeId::new("action").expect("node"),
            GraphNodeAttemptId::new(),
            DurableApprovalWaitId::new(),
            ApprovalRequestId::new(),
            ActionProposalId::new(),
            ToolCallId::new(),
            ActionDigest::from_bytes([2; 32]),
            WorkspaceBindingId::new(),
            ToolContractDigest::from_bytes([3; 32]),
        )
    }

    #[test]
    fn round_trip_and_tamper_rejection() {
        let sealer = LocalActionSealer::new("key-1", [7; 32]).expect("sealer");
        let seal_binding = binding();
        let action =
            LocalWriteActionCapsuleV1::new("out.txt".into(), "secret".into()).expect("action");
        let sealed = sealer
            .seal_local_write(&seal_binding, &action)
            .expect("seal");
        assert!(
            !sealed
                .ciphertext()
                .windows(6)
                .any(|bytes| bytes == b"secret")
        );
        assert_eq!(
            sealer
                .open_local_write(&seal_binding, &sealed)
                .expect("open")
                .content(),
            "secret"
        );

        let swapped = binding();
        assert_eq!(
            sealer.open_local_write(&swapped, &sealed),
            Err(ActionSealError::AuthenticationFailed)
        );

        let mut ciphertext = sealed.ciphertext().to_vec();
        ciphertext[0] ^= 0x80;
        let tampered = SealedLocalWriteAction::new(
            sealed.key_id().to_owned(),
            sealed.nonce().to_vec(),
            ciphertext,
        )
        .expect("tampered envelope remains structurally valid");
        assert_eq!(
            sealer.open_local_write(&seal_binding, &tampered),
            Err(ActionSealError::AuthenticationFailed)
        );
    }

    #[test]
    fn wrong_key_and_key_id_fail_closed() {
        let first = LocalActionSealer::new("key-1", [7; 32]).expect("first");
        let wrong_key = LocalActionSealer::new("key-1", [8; 32]).expect("wrong key");
        let wrong_id = LocalActionSealer::new("key-2", [7; 32]).expect("wrong id");
        let binding = binding();
        let action =
            LocalWriteActionCapsuleV1::new("out.txt".into(), "data".into()).expect("action");
        let sealed = first.seal_local_write(&binding, &action).expect("seal");
        assert_eq!(
            wrong_key.open_local_write(&binding, &sealed),
            Err(ActionSealError::AuthenticationFailed)
        );
        assert_eq!(
            wrong_id.open_local_write(&binding, &sealed),
            Err(ActionSealError::KeyUnavailable)
        );
    }

    #[test]
    fn requester_identity_is_authenticated_capsule_data() {
        let sealer = LocalActionSealer::new("key-1", [7; 32]).expect("sealer");
        let requester = PrincipalId::new("requester-a").expect("principal");
        let requester_binding = binding().with_requester(requester);
        let action =
            LocalWriteActionCapsuleV1::new("out.txt".into(), "data".into()).expect("action");
        let sealed = sealer
            .seal_local_write(&requester_binding, &action)
            .expect("seal");
        let swapped =
            binding().with_requester(PrincipalId::new("requester-b").expect("second principal"));

        assert_eq!(
            sealer.open_local_write(&swapped, &sealed),
            Err(ActionSealError::AuthenticationFailed)
        );
    }

    #[test]
    fn nonces_are_unique_and_capsule_bounds_are_enforced() {
        let sealer = LocalActionSealer::new("key-1", [7; 32]).expect("sealer");
        let binding = binding();
        let action =
            LocalWriteActionCapsuleV1::new("out.txt".into(), "data".into()).expect("action");
        let first = sealer.seal_local_write(&binding, &action).expect("first");
        let second = sealer.seal_local_write(&binding, &action).expect("second");
        assert_ne!(first.nonce(), second.nonce());
        assert!(LocalWriteActionCapsuleV1::new("../escape".into(), "x".into()).is_err());
        assert!(
            LocalWriteActionCapsuleV1::new(
                "out.txt".into(),
                "x".repeat(agent_harness::MAX_DURABLE_CONTENT_BYTES + 1),
            )
            .is_err()
        );

        let mut encoded = serde_json::to_value(&first).expect("encode");
        encoded["version"] = serde_json::json!(99);
        let unsupported: SealedLocalWriteAction =
            serde_json::from_value(encoded).expect("structural envelope");
        assert_eq!(
            sealer.open_local_write(&binding, &unsupported),
            Err(ActionSealError::InvalidCapsule)
        );
    }
}

use std::os::unix::fs::PermissionsExt;

use super::*;

fn config(_secret: &Path) -> DeploymentConfigV2 {
    let data_directory = _secret.parent().expect("secret parent").join("data");
    DeploymentConfigV2 {
        schema_version: DEPLOYMENT_CONFIG_SCHEMA_VERSION,
        listener: ListenerConfigV1::Unix {
            socket_path: PathBuf::from("/tmp/ela.sock"),
        },
        identity: IdentityConfigV1 {
            policy_version: 1,
            approval_separation: ApprovalSeparation::RequesterMustDiffer,
            principals: vec![PrincipalConfigV1 {
                principal_id: agent_core::PrincipalId::new("local:test-user").expect("principal"),
                kind: PrincipalKind::Human,
                roles: vec![PrincipalRole::User, PrincipalRole::Approver],
                unix_uid: Some(1000),
                unix_gid: Some(1000),
                loopback_bearer: false,
            }],
        },
        storage: StorageConfigV1 {
            data_directory,
            database_file: "runs.sqlite3".into(),
            audit_file: "audit.jsonl".into(),
        },
        workflows: WorkflowConfigV1 {
            readonly_enabled: true,
            readonly_required: true,
            local_write_enabled: false,
            local_write_required: false,
        },
        model: ModelConfigV1 {
            base_url: "http://127.0.0.1:8080/v1".into(),
            model_id: "local-model".into(),
            provider_label: "local".into(),
            auth: ModelAuthConfigV1::NoAuthLoopback,
            disable_reasoning: true,
            readiness_timeout_ms: 5_000,
        },
        knowledge: KnowledgeConfigV1::Ocpp {
            base_url: "http://127.0.0.1:18081".into(),
            readiness_query: "readiness".into(),
        },
        mcp: vec![],
        local_write: None,
        limits: OperationalLimitsV1::default(),
    }
}

#[test]
fn strict_config_bounds_redaction_and_fingerprint_are_deterministic() {
    let directory = tempfile::tempdir().expect("temp");
    let secret = directory.path().join("secret");
    std::fs::write(&secret, "sensitive-value\n").expect("secret");
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).expect("mode");
    let config = config(&secret);
    config.validate().expect("valid");
    assert_eq!(
        config.fingerprint().expect("fingerprint"),
        config.fingerprint().expect("same")
    );
    assert!(!format!("{config:?}").contains("sensitive-value"));
    let reference = SecretReferenceV1 { file: secret };
    let resolved = reference.resolve().expect("resolve");
    assert_eq!(resolved.bytes(), b"sensitive-value");

    let encoded = toml::to_string(&config).expect("toml");
    let with_unknown = format!("{encoded}\nunknown = true\n");
    assert!(matches!(
        DeploymentConfigV2::parse(with_unknown.as_bytes()),
        Err(DeploymentConfigError::InvalidConfig)
    ));
    assert!(matches!(
        DeploymentConfigV2::parse(&vec![b'x'; MAX_DEPLOYMENT_CONFIG_BYTES + 1]),
        Err(DeploymentConfigError::InvalidConfig)
    ));
}

#[test]
fn checked_in_readonly_example_is_valid() {
    let example = include_bytes!("../../../docs/deployment-config-v2.example.toml");
    DeploymentConfigV2::parse(example).expect("deployment example");
}

#[test]
fn localwrite_profile_never_silently_downgrades() {
    let directory = tempfile::tempdir().expect("temp");
    let secret = directory.path().join("secret");
    let mut config = config(&secret);
    config.workflows.local_write_enabled = true;
    assert_eq!(config.validate(), Err(DeploymentConfigError::InvalidConfig));
}

#[test]
fn compatibility_ignores_build_identity_only() {
    let base = CompatibilityManifestV1 {
        version: 1,
        build_identity: "build-a".into(),
        git_identity: Some("a".into()),
        store_schema_version: 2,
        event_schema_version: 9,
        checkpoint_schema_version: 4,
        workflow_contracts: vec!["mvp-v1".into()],
        graph_digests: vec!["01".repeat(32)],
        capsule_version: 1,
        tool_contract_digests: vec!["02".repeat(32)],
        seal_key_ids: vec!["key".into()],
        workspace_bindings: vec![WorkspaceBindingId::from_uuid(uuid::Uuid::nil())],
        mcp_fingerprints: vec![],
        containment_protocol_version: 1,
        containment_artifacts: vec!["03".repeat(32)],
    };
    let mut other = base.clone();
    other.build_identity = "build-b".into();
    other.git_identity = Some("b".into());
    assert!(base.compatible_with(&other));
    other.store_schema_version += 1;
    assert!(!base.compatible_with(&other));

    for incompatible in [
        {
            let mut value = base.clone();
            value.seal_key_ids = vec!["wrong-key".into()];
            value
        },
        {
            let mut value = base.clone();
            value.workspace_bindings = vec![WorkspaceBindingId::new()];
            value
        },
        {
            let mut value = base.clone();
            value.containment_artifacts = vec!["ff".repeat(32)];
            value
        },
        {
            let mut value = base.clone();
            value.tool_contract_digests = vec!["ee".repeat(32)];
            value
        },
    ] {
        assert!(!base.compatible_with(&incompatible));
    }
}

#[test]
fn readiness_cache_and_metrics_are_payload_free_and_bounded() {
    let cache = ReadinessCache::new(ReadinessSnapshotV1 {
        version: 1,
        overall: ReadinessStatusV1::Ready,
        dependencies: vec![],
        workflows: vec![],
    });
    assert_eq!(
        cache.snapshot().expect("snapshot").overall,
        ReadinessStatusV1::Ready
    );
    let metrics = OperationalMetrics::new();
    metrics
        .record_latency(MetricLatencyKind::Model, Duration::from_secs(3))
        .expect("record");
    metrics.run_started().expect("run started");
    metrics
        .run_settled(Some(agent_service::RunDispositionV1::Waiting))
        .expect("run waiting");
    metrics
        .record_recovery(agent_service::RunDispositionV1::ManualReconciliationRequired)
        .expect("reconciliation");
    let snapshot = metrics.snapshot().expect("snapshot");
    assert_eq!(snapshot.active_runs, 0);
    assert_eq!(snapshot.runs_waiting, 1);
    assert_eq!(snapshot.runs_completed, 0);
    assert_eq!(snapshot.recovered_manual_reconciliation, 1);
    let encoded = serde_json::to_string(&snapshot).expect("json");
    for forbidden in ["prompt", "answer", "evidence", "credential", "capsule"] {
        assert!(!encoded.contains(forbidden));
    }
}

#[test]
fn deployment_lock_is_exclusive_and_released() {
    let directory = tempfile::tempdir().expect("temp");
    let data = directory.path().join("data");
    let first = DeploymentLock::acquire(&data).expect("first lock");
    assert!(matches!(
        DeploymentLock::acquire(&data),
        Err(DeploymentConfigError::LockUnavailable)
    ));
    drop(first);
    DeploymentLock::acquire(&data).expect("released lock");
}

#[tokio::test]
async fn backup_restore_is_verified_cross_build_and_tamper_detected() {
    let directory = tempfile::tempdir().expect("temp");
    let secret = directory.path().join("secret");
    let config = config(&secret);
    std::fs::create_dir_all(&config.storage.data_directory).expect("data directory");
    agent_persistence_sqlite::SqliteRunPersistence::open(config.storage.database_path())
        .await
        .expect("store");
    std::fs::write(
        config.storage.audit_path(),
        b"{\"event\":\"metadata-only\"}\n",
    )
    .expect("audit");
    let expected = CompatibilityManifestV1::from_config(&config, "build-a", Some("git-a".into()))
        .expect("compatibility");
    let backup = directory.path().join("backup");
    create_backup(&config, &backup, expected.clone())
        .await
        .expect("backup");

    let cross_build =
        CompatibilityManifestV1::from_config(&config, "build-b", Some("git-b".into()))
            .expect("cross-build compatibility");
    verify_backup(&backup, &cross_build)
        .await
        .expect("verified cross-build");

    let restore_root = directory.path().join("restore");
    let mut restore_config = config.clone();
    restore_config.storage.data_directory = restore_root;
    restore_backup(&restore_config, &backup, &cross_build)
        .await
        .expect("restore");
    assert!(restore_config.storage.database_path().is_file());
    assert!(restore_config.storage.audit_path().is_file());
    assert!(matches!(
        restore_backup(&restore_config, &backup, &cross_build).await,
        Err(DeploymentConfigError::RestoreTargetNotEmpty)
    ));

    std::fs::write(backup.join("audit.jsonl"), b"tampered").expect("tamper");
    assert!(matches!(
        verify_backup(&backup, &cross_build).await,
        Err(DeploymentConfigError::BackupInvalid)
    ));
}

use std::fs::{File, Permissions};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{
    AgentEvent, AgentEventKind, CURRENT_EVENT_SCHEMA_VERSION, DurableApprovalOutcome,
    DurableApprovalWaitId, EventSequence, RunId, RunStatus, SessionId,
};
use agent_harness::{
    AppendTransition, CURRENT_CHECKPOINT_SCHEMA_VERSION, CURRENT_STORE_SCHEMA_VERSION,
    CreateDurableApprovalWait, DurableApprovalRecord, DurableApprovalStatus,
    DurableApprovalStatusTransition, DurableCheckpoint, DurableEventPage, DurableRunPage,
    DurableRunSummary, DurableWaitingPage, DurableWaitingSummary, LoadedRun, MAX_READ_PAGE_ITEMS,
    PersistenceFuture, PersistencePortError, ReadFuture, RecordDurableApprovalDecision, RunKey,
    RunPageCursor, RunPersistencePort, RunReadPort, RunRecord, WaitingPageCursor,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, backup::Backup, params};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

const APPLICATION_ID: i32 = 0x454c_4137;
type StoredRunRow = (String, Vec<u8>, Vec<u8>, Option<Vec<u8>>, i64);
type ReadRunRow = (String, String, Vec<u8>, Vec<u8>, Option<Vec<u8>>, i64);
type StoredApprovalRow = (String, Vec<u8>, Vec<u8>, String, Vec<u8>, i64, i64);

#[derive(Clone, Debug)]
pub struct SqliteRunPersistence {
    path: PathBuf,
    blocking_permit: Arc<Semaphore>,
}

impl SqliteRunPersistence {
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, PersistencePortError> {
        let path = path.into();
        if !path.is_absolute() || path.parent().is_none_or(|parent| !parent.is_dir()) {
            return Err(PersistencePortError::Unavailable);
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && !metadata.file_type().is_file()
        {
            return Err(PersistencePortError::Unavailable);
        }
        let initialize_path = path.clone();
        tokio::task::spawn_blocking(move || initialize(&initialize_path))
            .await
            .map_err(|_| PersistencePortError::Unavailable)??;
        Ok(Self {
            path,
            blocking_permit: Arc::new(Semaphore::new(1)),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Offline/quiesced administrative access to the SQLite store.
///
/// The caller must own the deployment data-directory lock. This type never
/// starts, resumes, approves, or otherwise executes a run.
#[derive(Clone, Debug)]
pub struct SqliteStoreAdmin {
    source: PathBuf,
}

impl SqliteStoreAdmin {
    pub fn new(source: impl Into<PathBuf>) -> Result<Self, PersistencePortError> {
        let source = source.into();
        let trusted_file =
            std::fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.file_type().is_file());
        if !source.is_absolute() || !trusted_file {
            return Err(PersistencePortError::Unavailable);
        }
        Ok(Self { source })
    }

    pub async fn backup_to(
        &self,
        destination: impl Into<PathBuf>,
    ) -> Result<StoreInspection, PersistencePortError> {
        let source = self.source.clone();
        let destination = destination.into();
        tokio::task::spawn_blocking(move || backup_store(&source, &destination))
            .await
            .map_err(|_| PersistencePortError::Unavailable)?
    }

    pub async fn inspect(&self) -> Result<StoreInspection, PersistencePortError> {
        let source = self.source.clone();
        tokio::task::spawn_blocking(move || inspect_store(&source))
            .await
            .map_err(|_| PersistencePortError::Unavailable)?
    }

    pub async fn inspect_path(
        path: impl Into<PathBuf>,
    ) -> Result<StoreInspection, PersistencePortError> {
        let path = path.into();
        tokio::task::spawn_blocking(move || inspect_store(&path))
            .await
            .map_err(|_| PersistencePortError::Unavailable)?
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreInspection {
    pub store_schema_version: u16,
    pub event_schema_versions: Vec<u16>,
    pub checkpoint_schema_versions: Vec<u16>,
    pub recovery_contracts: Vec<String>,
    pub capsule_versions: Vec<u16>,
    pub seal_key_ids: Vec<String>,
    pub workspace_bindings: Vec<String>,
    pub tool_contract_digests: Vec<String>,
    pub sha256: String,
}

fn backup_store(
    source: &Path,
    destination: &Path,
) -> Result<StoreInspection, PersistencePortError> {
    if !destination.is_absolute()
        || destination.exists()
        || destination.parent().is_none_or(|parent| !parent.is_dir())
    {
        return Err(PersistencePortError::Unavailable);
    }
    let temporary = destination.with_extension("partial");
    if temporary.exists() {
        return Err(PersistencePortError::Unavailable);
    }
    let result = (|| {
        let source_connection = Connection::open(source).map_err(map_unavailable)?;
        configure(&source_connection)?;
        validate_store_identity(&source_connection)?;
        let mut destination_connection = Connection::open(&temporary).map_err(map_unavailable)?;
        {
            let backup =
                Backup::new(&source_connection, &mut destination_connection).map_err(map_failed)?;
            backup
                .run_to_completion(64, std::time::Duration::from_millis(10), None)
                .map_err(map_failed)?;
        }
        destination_connection.close().map_err(map_failed)?;
        std::fs::set_permissions(&temporary, Permissions::from_mode(0o600))
            .map_err(map_unavailable)?;
        File::open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(map_unavailable)?;
        std::fs::rename(&temporary, destination).map_err(map_unavailable)?;
        File::open(
            destination
                .parent()
                .ok_or(PersistencePortError::Unavailable)?,
        )
        .and_then(|directory| directory.sync_all())
        .map_err(map_unavailable)?;
        inspect_store(destination)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn inspect_store(path: &Path) -> Result<StoreInspection, PersistencePortError> {
    let trusted_file =
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file());
    if !path.is_absolute() || !trusted_file {
        return Err(PersistencePortError::Unavailable);
    }
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(map_unavailable)?;
    validate_store_identity(&connection)?;
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(map_failed)?;
    if integrity != "ok" {
        return Err(PersistencePortError::Corrupt);
    }
    let foreign_key_failure: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM pragma_foreign_key_check LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_failed)?;
    if foreign_key_failure.is_some() {
        return Err(PersistencePortError::Corrupt);
    }

    let event_schema_versions = distinct_u16(
        &connection,
        "SELECT DISTINCT event_schema_version FROM events ORDER BY event_schema_version",
    )?;
    let checkpoint_schema_versions = distinct_u16(
        &connection,
        "SELECT DISTINCT checkpoint_schema_version FROM checkpoints ORDER BY checkpoint_schema_version",
    )?;
    if event_schema_versions
        .iter()
        .any(|version| *version != CURRENT_EVENT_SCHEMA_VERSION.get())
        || checkpoint_schema_versions
            .iter()
            .any(|version| *version != CURRENT_CHECKPOINT_SCHEMA_VERSION)
    {
        return Err(PersistencePortError::UnsupportedVersion);
    }

    let mut recovery_contracts = Vec::new();
    let mut statement = connection
        .prepare("SELECT run_id, session_id FROM runs ORDER BY run_id")
        .map_err(map_failed)?;
    let keys = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(map_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_failed)?;
    for (run_id, session_id) in keys {
        let key = RunKey::new(
            run_id.parse().map_err(|_| PersistencePortError::Corrupt)?,
            session_id
                .parse()
                .map_err(|_| PersistencePortError::Corrupt)?,
        );
        let loaded = load_run(path, key)?;
        recovery_contracts
            .push(serde_json::to_string(&loaded.record().recovery_contract()).map_err(map_failed)?);
    }
    recovery_contracts.sort();
    recovery_contracts.dedup();

    let mut seal_key_ids = Vec::new();
    let mut capsule_versions = Vec::new();
    let mut workspace_bindings = Vec::new();
    let mut tool_contract_digests = Vec::new();
    let mut approvals = connection
        .prepare(
            "SELECT run_id, session_id, wait_id FROM durable_approvals ORDER BY run_id, wait_id",
        )
        .map_err(map_failed)?;
    let approval_keys = approvals
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(map_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_failed)?;
    for (run_id, session_id, wait_id) in approval_keys {
        let key = RunKey::new(
            run_id.parse().map_err(|_| PersistencePortError::Corrupt)?,
            session_id
                .parse()
                .map_err(|_| PersistencePortError::Corrupt)?,
        );
        let wait_id = wait_id.parse().map_err(|_| PersistencePortError::Corrupt)?;
        let record = load_durable_approval_wait(path, key, wait_id)?;
        capsule_versions.push(record.binding().capsule_version());
        seal_key_ids.push(record.sealed_action().key_id().to_owned());
        workspace_bindings.push(record.binding().workspace_binding_id().to_string());
        tool_contract_digests.push(hex(record.binding().tool_contract_digest().as_bytes()));
    }
    seal_key_ids.sort();
    seal_key_ids.dedup();
    workspace_bindings.sort();
    workspace_bindings.dedup();
    tool_contract_digests.sort();
    tool_contract_digests.dedup();
    capsule_versions.sort();
    capsule_versions.dedup();

    Ok(StoreInspection {
        store_schema_version: CURRENT_STORE_SCHEMA_VERSION,
        event_schema_versions,
        checkpoint_schema_versions,
        recovery_contracts,
        capsule_versions,
        seal_key_ids,
        workspace_bindings,
        tool_contract_digests,
        sha256: file_sha256(path)?,
    })
}

fn distinct_u16(connection: &Connection, sql: &str) -> Result<Vec<u16>, PersistencePortError> {
    let mut statement = connection.prepare(sql).map_err(map_failed)?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(map_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_failed)
}

fn file_sha256(path: &Path) -> Result<String, PersistencePortError> {
    let mut file = File::open(path).map_err(map_unavailable)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(map_unavailable)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

impl RunPersistencePort for SqliteRunPersistence {
    fn create_run<'a>(
        &'a self,
        record: &'a RunRecord,
        initial_checkpoint: &'a DurableCheckpoint,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        let record = record.clone();
        let checkpoint = initial_checkpoint.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || create_run(&path, &record, &checkpoint))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn append_transition<'a>(
        &'a self,
        transition: &'a AppendTransition,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        let transition = transition.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || append_transition(&path, &transition))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn load_run<'a>(
        &'a self,
        key: RunKey,
    ) -> PersistenceFuture<'a, Result<LoadedRun, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || load_run(&path, key))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn create_durable_approval_wait<'a>(
        &'a self,
        request: &'a CreateDurableApprovalWait,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        let request = request.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || create_durable_approval_wait(&path, &request))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn load_durable_approval_wait<'a>(
        &'a self,
        key: RunKey,
        wait_id: DurableApprovalWaitId,
    ) -> PersistenceFuture<'a, Result<DurableApprovalRecord, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || load_durable_approval_wait(&path, key, wait_id))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn load_pending_approval_requests<'a>(
        &'a self,
        key: RunKey,
    ) -> PersistenceFuture<'a, Result<Vec<DurableApprovalRecord>, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || load_pending_approval_requests(&path, key))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn record_durable_approval_decision<'a>(
        &'a self,
        request: &'a RecordDurableApprovalDecision,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        let request = request.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || record_durable_approval_decision(&path, &request))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn transition_durable_approval_status<'a>(
        &'a self,
        request: &'a DurableApprovalStatusTransition,
    ) -> PersistenceFuture<'a, Result<(), PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        let request = request.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || transition_durable_approval_status(&path, &request))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }
}

impl RunReadPort for SqliteRunPersistence {
    fn find_run<'a>(
        &'a self,
        key: RunKey,
    ) -> ReadFuture<'a, Result<Option<DurableRunSummary>, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || find_run(&path, key))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn list_runs<'a>(
        &'a self,
        after: Option<RunPageCursor>,
        limit: u16,
    ) -> ReadFuture<'a, Result<DurableRunPage, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            validate_page_limit(limit)?;
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || list_runs(&path, after, limit))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn read_events<'a>(
        &'a self,
        key: RunKey,
        after: Option<EventSequence>,
        limit: u16,
    ) -> ReadFuture<'a, Result<DurableEventPage, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            validate_page_limit(limit)?;
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || read_events(&path, key, after, limit))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }

    fn list_waiting<'a>(
        &'a self,
        after: Option<WaitingPageCursor>,
        limit: u16,
    ) -> ReadFuture<'a, Result<DurableWaitingPage, PersistencePortError>> {
        let path = self.path.clone();
        let blocking_permit = self.blocking_permit.clone();
        Box::pin(async move {
            validate_page_limit(limit)?;
            let _permit = blocking_permit
                .acquire_owned()
                .await
                .map_err(|_| PersistencePortError::Unavailable)?;
            tokio::task::spawn_blocking(move || list_waiting(&path, after, limit))
                .await
                .map_err(|_| PersistencePortError::Unavailable)?
        })
    }
}

fn validate_page_limit(limit: u16) -> Result<(), PersistencePortError> {
    if limit == 0 || limit > MAX_READ_PAGE_ITEMS {
        return Err(PersistencePortError::Conflict);
    }
    Ok(())
}

fn find_run(path: &Path, key: RunKey) -> Result<Option<DurableRunSummary>, PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    validate_store_identity(&connection)?;
    let stored: Option<StoredRunRow> = connection
        .query_row(
            "SELECT session_id,record,record_checksum,last_sequence,terminal FROM runs WHERE run_id=?1",
            [key.run_id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(map_failed)?;
    let Some((session_id, bytes, stored_checksum, last_sequence, terminal)) = stored else {
        return Ok(None);
    };
    if session_id != key.session_id().to_string()
        || checksum(&bytes) != stored_checksum
        || !matches!(terminal, 0 | 1)
    {
        return Err(PersistencePortError::Corrupt);
    }
    let record: RunRecord = decode(&bytes)?;
    if record.store_schema_version() != CURRENT_STORE_SCHEMA_VERSION
        || record.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
    {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    if record.key() != key {
        return Err(PersistencePortError::Corrupt);
    }
    Ok(Some(DurableRunSummary::new(
        key,
        record.recovery_contract(),
        decode_optional_sequence(last_sequence.as_deref())?,
        terminal == 1,
    )))
}

fn list_runs(
    path: &Path,
    after: Option<RunPageCursor>,
    limit: u16,
) -> Result<DurableRunPage, PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    validate_store_identity(&connection)?;
    let after = after
        .map(|cursor| cursor.run_id().to_string())
        .unwrap_or_default();
    let mut statement = connection
        .prepare(
            "SELECT run_id,session_id,record,record_checksum,last_sequence,terminal FROM runs
             WHERE run_id>?1 ORDER BY run_id LIMIT ?2",
        )
        .map_err(map_failed)?;
    let rows: Vec<ReadRunRow> = statement
        .query_map(params![after, i64::from(limit) + 1], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(map_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_failed)?;
    let has_more = rows.len() > usize::from(limit);
    let mut items = Vec::with_capacity(rows.len().min(usize::from(limit)));
    for (run_id, session_id, bytes, stored_checksum, last_sequence, terminal) in
        rows.into_iter().take(usize::from(limit))
    {
        if checksum(&bytes) != stored_checksum || !matches!(terminal, 0 | 1) {
            return Err(PersistencePortError::Corrupt);
        }
        let record: RunRecord = decode(&bytes)?;
        if record.key().run_id().to_string() != run_id
            || record.key().session_id().to_string() != session_id
        {
            return Err(PersistencePortError::Corrupt);
        }
        if record.store_schema_version() != CURRENT_STORE_SCHEMA_VERSION
            || record.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
        {
            return Err(PersistencePortError::UnsupportedVersion);
        }
        items.push(DurableRunSummary::new(
            record.key(),
            record.recovery_contract(),
            decode_optional_sequence(last_sequence.as_deref())?,
            terminal == 1,
        ));
    }
    let next = has_more
        .then(|| {
            items
                .last()
                .map(|item| RunPageCursor::new(item.key().run_id()))
        })
        .flatten();
    Ok(DurableRunPage::new(items, next))
}

fn read_events(
    path: &Path,
    key: RunKey,
    after: Option<EventSequence>,
    limit: u16,
) -> Result<DurableEventPage, PersistencePortError> {
    let loaded = load_run(path, key)?;
    if after.is_some_and(|cursor| {
        loaded
            .events()
            .last()
            .is_none_or(|event| cursor.get() > event.sequence().get())
    }) {
        return Err(PersistencePortError::Conflict);
    }
    let mut matching = loaded
        .events()
        .iter()
        .filter(|event| after.is_none_or(|cursor| event.sequence().get() > cursor.get()));
    let events = matching
        .by_ref()
        .take(usize::from(limit))
        .cloned()
        .collect::<Vec<_>>();
    let has_more = matching.next().is_some();
    let next = has_more
        .then(|| events.last().map(AgentEvent::sequence))
        .flatten();
    Ok(DurableEventPage::new(events, next))
}

fn list_waiting(
    path: &Path,
    after: Option<WaitingPageCursor>,
    limit: u16,
) -> Result<DurableWaitingPage, PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    validate_store_identity(&connection)?;
    let (after_run, after_wait) = after.map_or_else(
        || (String::new(), String::new()),
        |cursor| (cursor.run_id().to_string(), cursor.wait_id().to_string()),
    );
    let mut statement = connection
        .prepare(
            "SELECT run_id,session_id,wait_id FROM durable_approvals
             WHERE status<>?1 AND (run_id>?2 OR (run_id=?2 AND wait_id>?3))
             ORDER BY run_id,wait_id LIMIT ?4",
        )
        .map_err(map_failed)?;
    let rows = statement
        .query_map(
            params![
                status_code(DurableApprovalStatus::Consumed),
                after_run,
                after_wait,
                i64::from(limit) + 1
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(map_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_failed)?;
    let has_more = rows.len() > usize::from(limit);
    let mut items = Vec::with_capacity(rows.len().min(usize::from(limit)));
    for (run_id, session_id, wait_id) in rows.into_iter().take(usize::from(limit)) {
        let key = RunKey::new(
            run_id
                .parse::<RunId>()
                .map_err(|_| PersistencePortError::Corrupt)?,
            session_id
                .parse::<SessionId>()
                .map_err(|_| PersistencePortError::Corrupt)?,
        );
        let wait_id = wait_id
            .parse::<DurableApprovalWaitId>()
            .map_err(|_| PersistencePortError::Corrupt)?;
        let record = load_durable_approval_wait(path, key, wait_id)?;
        items.push(DurableWaitingSummary::new(
            key,
            wait_id,
            record.row_version(),
            record.status(),
        ));
    }
    let next = has_more
        .then(|| {
            items
                .last()
                .map(|item| WaitingPageCursor::new(item.key().run_id(), item.wait_id()))
        })
        .flatten();
    Ok(DurableWaitingPage::new(items, next))
}

fn validate_store_identity(connection: &Connection) -> Result<(), PersistencePortError> {
    let version: u16 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_failed)?;
    let application_id: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(map_failed)?;
    if version != CURRENT_STORE_SCHEMA_VERSION || application_id != APPLICATION_ID {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    Ok(())
}

fn initialize(path: &Path) -> Result<(), PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let version: u16 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_failed)?;
    if version == 0 {
        connection
            .execute_batch(&format!(
                "BEGIN IMMEDIATE;
                 PRAGMA application_id={APPLICATION_ID};
                 PRAGMA user_version={CURRENT_STORE_SCHEMA_VERSION};
                 CREATE TABLE runs(
                   run_id TEXT PRIMARY KEY NOT NULL,
                   session_id TEXT NOT NULL,
                   record BLOB NOT NULL,
                   record_checksum BLOB NOT NULL,
                   last_sequence BLOB,
                   terminal INTEGER NOT NULL CHECK(terminal IN (0,1))
                 );
                 CREATE TABLE events(
                   run_id TEXT NOT NULL,
                   sequence BLOB NOT NULL CHECK(length(sequence)=8),
                   event_schema_version INTEGER NOT NULL,
                   payload BLOB NOT NULL,
                   checksum BLOB NOT NULL,
                   PRIMARY KEY(run_id, sequence),
                   FOREIGN KEY(run_id) REFERENCES runs(run_id)
                 );
                 CREATE TABLE checkpoints(
                   run_id TEXT PRIMARY KEY NOT NULL,
                   through_sequence BLOB,
                   checkpoint_schema_version INTEGER NOT NULL,
                   payload BLOB NOT NULL,
                   checksum BLOB NOT NULL,
                   FOREIGN KEY(run_id) REFERENCES runs(run_id)
                 );
                 CREATE TABLE durable_approvals(
                   run_id TEXT NOT NULL,
                   session_id TEXT NOT NULL,
                   wait_id TEXT NOT NULL,
                   record BLOB NOT NULL,
                   checksum BLOB NOT NULL,
                   key_id TEXT NOT NULL,
                   nonce BLOB NOT NULL,
                   status INTEGER NOT NULL,
                   row_version INTEGER NOT NULL CHECK(row_version >= 0),
                   PRIMARY KEY(run_id, wait_id),
                   UNIQUE(key_id, nonce),
                   FOREIGN KEY(run_id) REFERENCES runs(run_id)
                 );
                 CREATE TRIGGER events_no_update BEFORE UPDATE ON events
                   BEGIN SELECT RAISE(ABORT, 'append-only events'); END;
                 CREATE TRIGGER events_no_delete BEFORE DELETE ON events
                   BEGIN SELECT RAISE(ABORT, 'append-only events'); END;
                 COMMIT;"
            ))
            .map_err(map_failed)?;
    } else if version != CURRENT_STORE_SCHEMA_VERSION {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    let application_id: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(map_failed)?;
    if application_id != APPLICATION_ID {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    std::fs::set_permissions(path, Permissions::from_mode(0o600)).map_err(map_unavailable)?;
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), PersistencePortError> {
    connection
        .execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(map_failed)
}

fn create_run(
    path: &Path,
    record: &RunRecord,
    checkpoint: &DurableCheckpoint,
) -> Result<(), PersistencePortError> {
    validate_record_checkpoint(record, checkpoint)?;
    let mut connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_failed)?;
    let record_bytes = encode(record)?;
    let checkpoint_bytes = encode(checkpoint)?;
    transaction
        .execute(
            "INSERT INTO runs(run_id,session_id,record,record_checksum,last_sequence,terminal)
             VALUES(?1,?2,?3,?4,NULL,0)",
            params![
                record.key().run_id().to_string(),
                record.key().session_id().to_string(),
                record_bytes,
                checksum(&record_bytes),
            ],
        )
        .map_err(map_conflict)?;
    transaction
        .execute(
            "INSERT INTO checkpoints(run_id,through_sequence,checkpoint_schema_version,payload,checksum)
             VALUES(?1,NULL,?2,?3,?4)",
            params![
                record.key().run_id().to_string(),
                CURRENT_CHECKPOINT_SCHEMA_VERSION,
                checkpoint_bytes,
                checksum(&checkpoint_bytes),
            ],
        )
        .map_err(map_failed)?;
    transaction.commit().map_err(map_failed)
}

fn append_transition(
    path: &Path,
    transition: &AppendTransition,
) -> Result<(), PersistencePortError> {
    if transition.event().run_id() != transition.key().run_id()
        || transition.event().schema_version() != CURRENT_EVENT_SCHEMA_VERSION
        || transition.checkpoint().is_some_and(|checkpoint| {
            checkpoint.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
                || checkpoint.state().key() != transition.key()
                || checkpoint.state().last_sequence() != Some(transition.event().sequence())
        })
    {
        return Err(PersistencePortError::Conflict);
    }
    let expected_next = transition
        .expected_sequence()
        .map_or(EventSequence::new(0), |sequence| {
            sequence
                .checked_next()
                .unwrap_or(EventSequence::new(u64::MAX))
        });
    if transition.event().sequence() != expected_next {
        return Err(PersistencePortError::Conflict);
    }

    let mut connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_failed)?;
    append_transition_in_transaction(&transaction, transition)?;
    transaction.commit().map_err(map_failed)
}

fn append_transition_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    transition: &AppendTransition,
) -> Result<(), PersistencePortError> {
    if transition.event().run_id() != transition.key().run_id()
        || transition.event().schema_version() != CURRENT_EVENT_SCHEMA_VERSION
        || transition.checkpoint().is_some_and(|checkpoint| {
            checkpoint.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
                || checkpoint.state().key() != transition.key()
                || checkpoint.state().last_sequence() != Some(transition.event().sequence())
        })
    {
        return Err(PersistencePortError::Conflict);
    }
    let expected_next = transition
        .expected_sequence()
        .map_or(EventSequence::new(0), |sequence| {
            sequence
                .checked_next()
                .unwrap_or(EventSequence::new(u64::MAX))
        });
    if transition.event().sequence() != expected_next {
        return Err(PersistencePortError::Conflict);
    }
    let stored: Option<(String, Option<Vec<u8>>)> = transaction
        .query_row(
            "SELECT session_id,last_sequence FROM runs WHERE run_id=?1",
            [transition.key().run_id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(map_failed)?;
    let Some((session_id, last_sequence)) = stored else {
        return Err(PersistencePortError::Conflict);
    };
    if session_id != transition.key().session_id().to_string()
        || decode_optional_sequence(last_sequence.as_deref())? != transition.expected_sequence()
    {
        return Err(PersistencePortError::Conflict);
    }

    let event_bytes = encode(transition.event())?;
    let sequence = encode_sequence(transition.event().sequence());
    transaction
        .execute(
            "INSERT INTO events(run_id,sequence,event_schema_version,payload,checksum)
             VALUES(?1,?2,?3,?4,?5)",
            params![
                transition.key().run_id().to_string(),
                sequence,
                transition.event().schema_version().get(),
                event_bytes,
                checksum(&event_bytes),
            ],
        )
        .map_err(map_conflict)?;
    let terminal = matches!(
        transition.event().kind(),
        agent_core::AgentEventKind::RunFinished { .. }
    );
    transaction
        .execute(
            "UPDATE runs SET last_sequence=?1, terminal=MAX(terminal,?2) WHERE run_id=?3",
            params![
                sequence,
                i64::from(terminal),
                transition.key().run_id().to_string()
            ],
        )
        .map_err(map_failed)?;
    if let Some(checkpoint) = transition.checkpoint() {
        let bytes = encode(checkpoint)?;
        transaction
            .execute(
                "UPDATE checkpoints SET through_sequence=?1,checkpoint_schema_version=?2,payload=?3,checksum=?4
                 WHERE run_id=?5",
                params![
                    sequence,
                    checkpoint.checkpoint_schema_version(),
                    bytes,
                    checksum(&bytes),
                    transition.key().run_id().to_string(),
                ],
            )
            .map_err(map_failed)?;
    }
    Ok(())
}

fn create_durable_approval_wait(
    path: &Path,
    request: &CreateDurableApprovalWait,
) -> Result<(), PersistencePortError> {
    if request.record().status() != DurableApprovalStatus::Waiting
        || request.record().row_version() != 0
        || request.record().binding().key() != request.transition().key()
        || !matches!(
            request.transition().event().kind(),
            AgentEventKind::Graph {
                event: agent_core::GraphProgressEvent::GraphSuspended { wait_id, .. }
            } if *wait_id == request.record().binding().wait_id()
        )
    {
        return Err(PersistencePortError::Conflict);
    }
    let mut connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_failed)?;
    append_transition_in_transaction(&transaction, request.transition())?;
    let bytes = encode(request.record())?;
    transaction
        .execute(
            "INSERT INTO durable_approvals(run_id,session_id,wait_id,record,checksum,key_id,nonce,status,row_version)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
            params![
                request.transition().key().run_id().to_string(),
                request.transition().key().session_id().to_string(),
                request.record().binding().wait_id().to_string(),
                bytes,
                checksum(&bytes),
                request.record().sealed_action().key_id(),
                request.record().sealed_action().nonce(),
                status_code(DurableApprovalStatus::Waiting),
            ],
        )
        .map_err(map_conflict)?;
    transaction.commit().map_err(map_failed)
}

fn load_durable_approval_wait(
    path: &Path,
    key: RunKey,
    wait_id: DurableApprovalWaitId,
) -> Result<DurableApprovalRecord, PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let stored: Option<StoredApprovalRow> = connection
        .query_row(
            "SELECT session_id,record,checksum,key_id,nonce,status,row_version FROM durable_approvals
             WHERE run_id=?1 AND wait_id=?2",
            params![key.run_id().to_string(), wait_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(map_failed)?;
    let Some((session_id, bytes, stored_checksum, key_id, nonce, status, row_version)) = stored
    else {
        return Err(PersistencePortError::Unavailable);
    };
    if session_id != key.session_id().to_string() || checksum(&bytes) != stored_checksum {
        return Err(PersistencePortError::Corrupt);
    }
    let record: DurableApprovalRecord = decode(&bytes)?;
    record
        .sealed_action()
        .validate()
        .map_err(|_| PersistencePortError::Corrupt)?;
    if record.binding().key() != key
        || record.binding().wait_id() != wait_id
        || record.sealed_action().key_id() != key_id
        || record.sealed_action().nonce() != nonce
    {
        return Err(PersistencePortError::Corrupt);
    }
    let row_version = u64::try_from(row_version).map_err(|_| PersistencePortError::Corrupt)?;
    Ok(record.restore_store_state(decode_status(status)?, row_version))
}

fn load_pending_approval_requests(
    path: &Path,
    key: RunKey,
) -> Result<Vec<DurableApprovalRecord>, PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let mut statement = connection
        .prepare(
            "SELECT wait_id FROM durable_approvals
             WHERE run_id=?1 AND session_id=?2 AND status<>?3 ORDER BY wait_id",
        )
        .map_err(map_failed)?;
    let ids = statement
        .query_map(
            params![
                key.run_id().to_string(),
                key.session_id().to_string(),
                status_code(DurableApprovalStatus::Consumed)
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_failed)?;
    ids.into_iter()
        .map(|id| id.parse().map_err(|_| PersistencePortError::Corrupt))
        .map(|id| id.and_then(|id| load_durable_approval_wait(path, key, id)))
        .collect()
}

fn record_durable_approval_decision(
    path: &Path,
    request: &RecordDurableApprovalDecision,
) -> Result<(), PersistencePortError> {
    let command = request.command();
    let record = load_durable_approval_wait(path, command.key, command.wait_id)?;
    let binding = record.binding();
    if record.status() != DurableApprovalStatus::Waiting
        || record.row_version() != command.expected_row_version
        || binding.approval_request_id() != command.approval_request_id
        || binding.action_proposal_id() != command.action_proposal_id
        || binding.tool_call_id() != command.tool_call_id
        || binding.action_digest() != command.action_digest
        || !matches!(request.transition().event().kind(), AgentEventKind::DurableApprovalDecisionRecorded {
            wait_id, approval_request_id, outcome, row_version
        } if *wait_id == command.wait_id
            && *approval_request_id == command.approval_request_id
            && *outcome == command.outcome
            && *row_version == command.expected_row_version.saturating_add(1))
    {
        return Err(PersistencePortError::Conflict);
    }
    let next = match command.outcome {
        DurableApprovalOutcome::Approve => DurableApprovalStatus::DecisionRecordedApprove,
        DurableApprovalOutcome::Deny => DurableApprovalStatus::DecisionRecordedDeny,
    };
    let mut connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_failed)?;
    append_transition_in_transaction(&transaction, request.transition())?;
    let changed = transaction
        .execute(
            "UPDATE durable_approvals SET status=?1,row_version=row_version+1
         WHERE run_id=?2 AND wait_id=?3 AND session_id=?4 AND status=?5 AND row_version=?6",
            params![
                status_code(next),
                command.key.run_id().to_string(),
                command.wait_id.to_string(),
                command.key.session_id().to_string(),
                status_code(DurableApprovalStatus::Waiting),
                i64::try_from(command.expected_row_version)
                    .map_err(|_| PersistencePortError::Conflict)?
            ],
        )
        .map_err(map_failed)?;
    if changed != 1 {
        return Err(PersistencePortError::Conflict);
    }
    transaction.commit().map_err(map_failed)
}

fn transition_durable_approval_status(
    path: &Path,
    request: &DurableApprovalStatusTransition,
) -> Result<(), PersistencePortError> {
    let mut connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_failed)?;
    if let Some(transition) = request.transition() {
        if transition.key() != request.key() {
            return Err(PersistencePortError::Conflict);
        }
        append_transition_in_transaction(&transaction, transition)?;
    }
    let changed = transaction
        .execute(
            "UPDATE durable_approvals SET status=?1,row_version=row_version+1
         WHERE run_id=?2 AND wait_id=?3 AND session_id=?4 AND status=?5 AND row_version=?6",
            params![
                status_code(request.next_status()),
                request.key().run_id().to_string(),
                request.wait_id().to_string(),
                request.key().session_id().to_string(),
                status_code(request.expected_status()),
                i64::try_from(request.expected_row_version())
                    .map_err(|_| PersistencePortError::Conflict)?
            ],
        )
        .map_err(map_failed)?;
    if changed != 1 {
        return Err(PersistencePortError::Conflict);
    }
    transaction.commit().map_err(map_failed)
}

const fn status_code(status: DurableApprovalStatus) -> i64 {
    match status {
        DurableApprovalStatus::Waiting => 0,
        DurableApprovalStatus::DecisionRecordedApprove => 1,
        DurableApprovalStatus::DecisionRecordedDeny => 2,
        DurableApprovalStatus::ApprovedReady => 3,
        DurableApprovalStatus::Executing => 4,
        DurableApprovalStatus::Consumed => 5,
    }
}

fn decode_status(value: i64) -> Result<DurableApprovalStatus, PersistencePortError> {
    match value {
        0 => Ok(DurableApprovalStatus::Waiting),
        1 => Ok(DurableApprovalStatus::DecisionRecordedApprove),
        2 => Ok(DurableApprovalStatus::DecisionRecordedDeny),
        3 => Ok(DurableApprovalStatus::ApprovedReady),
        4 => Ok(DurableApprovalStatus::Executing),
        5 => Ok(DurableApprovalStatus::Consumed),
        _ => Err(PersistencePortError::Corrupt),
    }
}

fn load_run(path: &Path, key: RunKey) -> Result<LoadedRun, PersistencePortError> {
    let connection = Connection::open(path).map_err(map_unavailable)?;
    configure(&connection)?;
    let version: u16 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_failed)?;
    if version != CURRENT_STORE_SCHEMA_VERSION {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(map_failed)?;
    if integrity != "ok" {
        return Err(PersistencePortError::Corrupt);
    }

    let stored: Option<StoredRunRow> = connection
        .query_row(
            "SELECT session_id,record,record_checksum,last_sequence,terminal FROM runs WHERE run_id=?1",
            [key.run_id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(map_failed)?;
    let Some((session_id, record_bytes, record_checksum, last_sequence, terminal)) = stored else {
        return Err(PersistencePortError::Unavailable);
    };
    if session_id != key.session_id().to_string() || checksum(&record_bytes) != record_checksum {
        return Err(PersistencePortError::Corrupt);
    }
    let record: RunRecord = decode(&record_bytes)?;
    if record.key() != key
        || record.store_schema_version() != CURRENT_STORE_SCHEMA_VERSION
        || record.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
    {
        return Err(PersistencePortError::UnsupportedVersion);
    }

    let (through_sequence, checkpoint_version, checkpoint_bytes, checkpoint_checksum): (
        Option<Vec<u8>>,
        u16,
        Vec<u8>,
        Vec<u8>,
    ) = connection
        .query_row(
            "SELECT through_sequence,checkpoint_schema_version,payload,checksum FROM checkpoints WHERE run_id=?1",
            [key.run_id().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(map_failed)?;
    if checkpoint_version != CURRENT_CHECKPOINT_SCHEMA_VERSION {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    if checksum(&checkpoint_bytes) != checkpoint_checksum {
        return Err(PersistencePortError::Corrupt);
    }
    let checkpoint: DurableCheckpoint = decode(&checkpoint_bytes)?;
    if checkpoint.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION {
        return Err(PersistencePortError::UnsupportedVersion);
    }
    if checkpoint.state().key() != key
        || checkpoint.state().last_sequence()
            != decode_optional_sequence(through_sequence.as_deref())?
    {
        return Err(PersistencePortError::Corrupt);
    }

    let mut statement = connection
        .prepare(
            "SELECT sequence,event_schema_version,payload,checksum FROM events
             WHERE run_id=?1 ORDER BY sequence",
        )
        .map_err(map_failed)?;
    let rows = statement
        .query_map([key.run_id().to_string()], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, u16>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(map_failed)?;
    let mut events = Vec::new();
    for row in rows {
        let (sequence, event_version, bytes, stored_checksum) = row.map_err(map_failed)?;
        if event_version != CURRENT_EVENT_SCHEMA_VERSION.get() {
            return Err(PersistencePortError::UnsupportedVersion);
        }
        if checksum(&bytes) != stored_checksum {
            return Err(PersistencePortError::Corrupt);
        }
        let event: AgentEvent = decode(&bytes)?;
        if event.schema_version() != CURRENT_EVENT_SCHEMA_VERSION {
            return Err(PersistencePortError::UnsupportedVersion);
        }
        let expected_sequence =
            u64::try_from(events.len()).map_err(|_| PersistencePortError::Corrupt)?;
        if sequence != encode_sequence(event.sequence())
            || event.sequence().get() != expected_sequence
            || event.run_id() != key.run_id()
        {
            return Err(PersistencePortError::Corrupt);
        }
        events.push(event);
    }
    let decoded_last = decode_optional_sequence(last_sequence.as_deref())?;
    let event_terminal = events.last().is_some_and(|event| {
        matches!(event.kind(), agent_core::AgentEventKind::RunFinished { .. })
    });
    if decoded_last != events.last().map(AgentEvent::sequence) || (terminal != 0) != event_terminal
    {
        return Err(PersistencePortError::Corrupt);
    }
    Ok(LoadedRun::new(record, checkpoint, events))
}

fn validate_record_checkpoint(
    record: &RunRecord,
    checkpoint: &DurableCheckpoint,
) -> Result<(), PersistencePortError> {
    if record.store_schema_version() != CURRENT_STORE_SCHEMA_VERSION
        || record.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
        || checkpoint.checkpoint_schema_version() != CURRENT_CHECKPOINT_SCHEMA_VERSION
        || checkpoint.state().key() != record.key()
        || checkpoint.state().budget() != record.budget()
        || checkpoint.state().last_sequence().is_some()
        || !matches!(checkpoint.state().status(), RunStatus::Pending)
    {
        return Err(PersistencePortError::Conflict);
    }
    Ok(())
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, PersistencePortError> {
    serde_json::to_vec(value).map_err(|_| PersistencePortError::Failed)
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PersistencePortError> {
    serde_json::from_slice(bytes).map_err(|_| PersistencePortError::Corrupt)
}

fn checksum(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn encode_sequence(sequence: EventSequence) -> [u8; 8] {
    sequence.get().to_be_bytes()
}

fn decode_optional_sequence(
    value: Option<&[u8]>,
) -> Result<Option<EventSequence>, PersistencePortError> {
    value
        .map(|bytes| {
            let bytes: [u8; 8] = bytes
                .try_into()
                .map_err(|_| PersistencePortError::Corrupt)?;
            Ok(EventSequence::new(u64::from_be_bytes(bytes)))
        })
        .transpose()
}

fn map_unavailable(_: impl std::fmt::Debug) -> PersistencePortError {
    PersistencePortError::Unavailable
}

fn map_failed(_: impl std::fmt::Debug) -> PersistencePortError {
    PersistencePortError::Failed
}

fn map_conflict(_: impl std::fmt::Debug) -> PersistencePortError {
    PersistencePortError::Conflict
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use agent_core::{
        AgentEventKind, ModelMessage, ModelRequest, ModelRole, RunBudget, RunId, SessionId,
    };
    use agent_harness::{
        AuditFailurePolicy, ExecutionHarness, HarnessConfig, M0ReadOnlyPolicy, RecoveryContract,
        RecoveryDisposition, RunContext, RunPersistencePort, ToolRegistry,
        testing::{FakeModelPort, InMemoryAuditSink},
    };

    use super::*;

    fn fixture() -> (tempfile::TempDir, RunRecord, DurableCheckpoint) {
        let directory = tempfile::tempdir().expect("temporary directory must be created");
        let record = RunRecord::new(
            RunKey::new(RunId::new(), SessionId::new()),
            RunBudget::new(2, 2, 2, Duration::from_secs(30)).expect("budget must be valid"),
            RecoveryContract::NonRestartable,
        );
        let checkpoint = DurableCheckpoint::initial(&record);
        (directory, record, checkpoint)
    }

    async fn store(directory: &tempfile::TempDir) -> Arc<SqliteRunPersistence> {
        Arc::new(
            SqliteRunPersistence::open(directory.path().join("runs.sqlite3"))
                .await
                .expect("store must open"),
        )
    }

    #[tokio::test]
    async fn offline_backup_is_verified_and_tampering_fails() {
        let (directory, record, checkpoint) = fixture();
        let store = store(&directory).await;
        store
            .create_run(&record, &checkpoint)
            .await
            .expect("create run");
        let backup_directory = tempfile::tempdir().expect("backup directory");
        let backup_path = backup_directory.path().join("backup.sqlite3");
        let admin = SqliteStoreAdmin::new(store.path()).expect("admin");
        let inspection = admin.backup_to(&backup_path).await.expect("backup");
        assert_eq!(
            inspection.store_schema_version,
            CURRENT_STORE_SCHEMA_VERSION
        );
        assert_eq!(
            inspection.checkpoint_schema_versions,
            vec![CURRENT_CHECKPOINT_SCHEMA_VERSION]
        );
        assert_eq!(
            SqliteStoreAdmin::inspect_path(&backup_path)
                .await
                .expect("inspect")
                .sha256,
            inspection.sha256
        );
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&backup_path)
            .expect("open backup");
        file.set_len(64).expect("truncate backup");
        assert!(SqliteStoreAdmin::inspect_path(&backup_path).await.is_err());
    }

    #[tokio::test]
    async fn append_load_and_sequence_conflicts_are_deterministic() {
        let (directory, record, checkpoint) = fixture();
        let store = store(&directory).await;
        store
            .create_run(&record, &checkpoint)
            .await
            .expect("run must be created");
        let event = AgentEvent::new(
            record.key().run_id(),
            EventSequence::new(0),
            AgentEventKind::RunStarted {
                started_at_unix_millis: 1,
            },
        );
        let transition = AppendTransition::new(record.key(), None, event.clone(), None);
        store
            .append_transition(&transition)
            .await
            .expect("first event must append");
        assert_eq!(
            store.append_transition(&transition).await,
            Err(PersistencePortError::Conflict)
        );
        let gap = AppendTransition::new(
            record.key(),
            Some(EventSequence::new(1)),
            AgentEvent::new(
                record.key().run_id(),
                EventSequence::new(2),
                AgentEventKind::AuditDegraded,
            ),
            None,
        );
        assert_eq!(
            store.append_transition(&gap).await,
            Err(PersistencePortError::Conflict)
        );
        let loaded = store.load_run(record.key()).await.expect("run must load");
        assert_eq!(loaded.events(), &[event]);
    }

    #[tokio::test]
    async fn read_port_pages_runs_and_events_without_exposing_storage() {
        let (directory, record, checkpoint) = fixture();
        let store = store(&directory).await;
        store
            .create_run(&record, &checkpoint)
            .await
            .expect("create");
        let event = AgentEvent::new(
            record.key().run_id(),
            EventSequence::new(0),
            AgentEventKind::RunStarted {
                started_at_unix_millis: 1,
            },
        );
        store
            .append_transition(&AppendTransition::new(
                record.key(),
                None,
                event.clone(),
                None,
            ))
            .await
            .expect("append");

        let found = store
            .find_run(record.key())
            .await
            .expect("find")
            .expect("summary");
        assert_eq!(found.key(), record.key());
        assert_eq!(found.last_sequence(), Some(EventSequence::new(0)));
        let runs = store.list_runs(None, 1).await.expect("runs");
        assert_eq!(runs.items(), &[found]);
        let events = store
            .read_events(record.key(), None, 1)
            .await
            .expect("events");
        assert_eq!(events.events(), &[event]);
        assert_eq!(
            store
                .read_events(record.key(), Some(EventSequence::new(9)), 1)
                .await,
            Err(PersistencePortError::Conflict)
        );
        assert_eq!(
            store.list_runs(None, 0).await,
            Err(PersistencePortError::Conflict)
        );
    }

    #[tokio::test]
    async fn wrong_session_and_event_run_are_rejected() {
        let (directory, record, checkpoint) = fixture();
        let store = store(&directory).await;
        store
            .create_run(&record, &checkpoint)
            .await
            .expect("run must be created");
        let wrong_session = RunKey::new(record.key().run_id(), SessionId::new());
        assert_eq!(
            store.load_run(wrong_session).await,
            Err(PersistencePortError::Corrupt)
        );
        let wrong_run = AppendTransition::new(
            record.key(),
            None,
            AgentEvent::new(
                RunId::new(),
                EventSequence::new(0),
                AgentEventKind::RunStarted {
                    started_at_unix_millis: 1,
                },
            ),
            None,
        );
        assert_eq!(
            store.append_transition(&wrong_run).await,
            Err(PersistencePortError::Conflict)
        );
    }

    #[tokio::test]
    async fn checksum_and_all_schema_versions_fail_closed() {
        for (mutation, expected) in [
            (
                "UPDATE events SET payload=x'00'",
                PersistencePortError::Corrupt,
            ),
            (
                "PRAGMA user_version=99",
                PersistencePortError::UnsupportedVersion,
            ),
            (
                "UPDATE checkpoints SET checkpoint_schema_version=99",
                PersistencePortError::UnsupportedVersion,
            ),
            (
                "UPDATE events SET event_schema_version=99",
                PersistencePortError::UnsupportedVersion,
            ),
        ] {
            let (directory, record, checkpoint) = fixture();
            let store = store(&directory).await;
            store
                .create_run(&record, &checkpoint)
                .await
                .expect("run must be created");
            let event = AgentEvent::new(
                record.key().run_id(),
                EventSequence::new(0),
                AgentEventKind::RunStarted {
                    started_at_unix_millis: 1,
                },
            );
            store
                .append_transition(&AppendTransition::new(record.key(), None, event, None))
                .await
                .expect("event must append");
            let connection = Connection::open(store.path()).expect("database must open");
            connection
                .execute_batch("DROP TRIGGER events_no_update;")
                .expect("test mutation must unlock event rows");
            connection
                .execute_batch(mutation)
                .expect("test mutation must apply");
            assert_eq!(store.load_run(record.key()).await.err(), Some(expected));
        }
    }

    #[tokio::test]
    async fn injected_transaction_failure_exposes_no_partial_event() {
        let (directory, record, checkpoint) = fixture();
        let store = store(&directory).await;
        store
            .create_run(&record, &checkpoint)
            .await
            .expect("run must be created");
        let connection = Connection::open(store.path()).expect("database must open");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_run_update BEFORE UPDATE ON runs
                 BEGIN SELECT RAISE(ABORT, 'injected crash'); END;",
            )
            .expect("failure trigger must install");
        drop(connection);
        let event = AgentEvent::new(
            record.key().run_id(),
            EventSequence::new(0),
            AgentEventKind::RunStarted {
                started_at_unix_millis: 1,
            },
        );
        assert!(
            store
                .append_transition(&AppendTransition::new(record.key(), None, event, None))
                .await
                .is_err()
        );
        let loaded = store.load_run(record.key()).await.expect("run must load");
        assert!(loaded.events().is_empty());
        assert_eq!(loaded.checkpoint().state().last_sequence(), None);
    }

    #[tokio::test]
    async fn governed_run_persists_and_recovers_without_replaying_model() {
        let (directory, record, _) = fixture();
        let store = store(&directory).await;
        let model = Arc::new(FakeModelPort::scripted(vec![Ok(
            agent_core::ModelResponse::new(vec![], None),
        )]));
        let harness = ExecutionHarness::new(
            model.clone(),
            ToolRegistry::new(),
            Arc::new(M0ReadOnlyPolicy),
            Arc::new(InMemoryAuditSink::new()),
            HarnessConfig::new(AuditFailurePolicy::FailClosed, Duration::from_secs(2))
                .expect("config must be valid"),
        )
        .with_persistence_port(store);
        let mut context = RunContext::new(
            record.key().run_id(),
            record.key().session_id(),
            *record.budget(),
        );
        harness
            .start_run(&mut context)
            .await
            .expect("run must start");
        harness
            .invoke_model(
                &mut context,
                ModelRequest::new(vec![ModelMessage::new(ModelRole::User, "not persisted")]),
            )
            .await
            .expect("model call must complete");
        harness
            .complete_run(&mut context)
            .await
            .expect("run must complete");
        assert_eq!(model.invocation_count(), 1);

        let recovered = harness
            .recover_run(record.key())
            .await
            .expect("run must recover");
        let RecoveryDisposition::Completed { state } = recovered else {
            panic!("terminal run must recover as completed");
        };
        assert_eq!(state.usage().model_calls(), 1);
        assert_eq!(model.invocation_count(), 1);
        let loaded = harness
            .recover_run(record.key())
            .await
            .expect("repeat replay must remain pure");
        assert!(matches!(loaded, RecoveryDisposition::Completed { .. }));
        assert_eq!(model.invocation_count(), 1);
    }

    #[test]
    fn adapter_is_provider_neutral_and_object_safe() {
        fn accepts(_: Arc<dyn RunPersistencePort>) {}
        let directory = tempfile::tempdir().expect("temporary directory must be created");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime must build");
        let store = runtime
            .block_on(SqliteRunPersistence::open(
                directory.path().join("runs.sqlite3"),
            ))
            .expect("store must open");
        accepts(Arc::new(store));
    }
}

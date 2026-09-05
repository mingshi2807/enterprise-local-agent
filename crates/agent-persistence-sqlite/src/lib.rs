use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{AgentEvent, CURRENT_EVENT_SCHEMA_VERSION, EventSequence, RunStatus};
use agent_harness::{
    AppendTransition, CURRENT_CHECKPOINT_SCHEMA_VERSION, CURRENT_STORE_SCHEMA_VERSION,
    DurableCheckpoint, LoadedRun, PersistenceFuture, PersistencePortError, RunKey,
    RunPersistencePort, RunRecord,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

const APPLICATION_ID: i32 = 0x454c_4137;
type StoredRunRow = (String, Vec<u8>, Vec<u8>, Option<Vec<u8>>, i64);

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
    transaction.commit().map_err(map_failed)
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
        if sequence != encode_sequence(event.sequence()) || event.run_id() != key.run_id() {
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

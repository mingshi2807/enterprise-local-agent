mod service_client;

use serde::Serialize;
use service_client::{
    BuildInfoV1, HealthV1, LocalServiceClient, ReadinessSnapshotV1, RunViewV1, ServiceEventV2,
    SessionV1,
};
use tauri::State;

#[derive(Debug, Serialize)]
struct DesktopCommandError {
    code: &'static str,
}

impl From<service_client::LocalServiceError> for DesktopCommandError {
    fn from(error: service_client::LocalServiceError) -> Self {
        Self { code: error.code() }
    }
}

#[tauri::command]
async fn service_health(
    client: State<'_, LocalServiceClient>,
) -> Result<HealthV1, DesktopCommandError> {
    client.health().await.map_err(Into::into)
}

#[tauri::command]
async fn service_readiness(
    client: State<'_, LocalServiceClient>,
) -> Result<ReadinessSnapshotV1, DesktopCommandError> {
    client.readiness().await.map_err(Into::into)
}

#[tauri::command]
async fn service_version(
    client: State<'_, LocalServiceClient>,
) -> Result<BuildInfoV1, DesktopCommandError> {
    client.version().await.map_err(Into::into)
}

#[tauri::command]
async fn conversation_create_session(
    client: State<'_, LocalServiceClient>,
) -> Result<SessionV1, DesktopCommandError> {
    client.create_session().await.map_err(Into::into)
}

#[tauri::command]
async fn conversation_start_readonly_run(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    start_request_id: String,
    input: String,
) -> Result<RunViewV1, DesktopCommandError> {
    client
        .start_readonly_run(&session_id, &start_request_id, &input)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn conversation_run_status(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
) -> Result<RunViewV1, DesktopCommandError> {
    client
        .run_status(&session_id, &run_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn conversation_cancel_run(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
) -> Result<(), DesktopCommandError> {
    client
        .cancel_run(&session_id, &run_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn conversation_read_events(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
    after_sequence: Option<u64>,
) -> Result<Vec<ServiceEventV2>, DesktopCommandError> {
    client
        .read_events(&session_id, &run_id, after_sequence)
        .await
        .map_err(Into::into)
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let client = LocalServiceClient::from_environment()?;
    tauri::Builder::default()
        .manage(client)
        .invoke_handler(tauri::generate_handler![
            service_health,
            service_readiness,
            service_version,
            conversation_create_session,
            conversation_start_readonly_run,
            conversation_run_status,
            conversation_cancel_run,
            conversation_read_events
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}

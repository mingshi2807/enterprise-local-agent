mod service_client;

use serde::Serialize;
use service_client::{
    ApprovalDecisionV1, BuildInfoV1, ConversationPageV1, DesktopApprovalPreviewV1, HealthV1,
    LocalServiceClient, ReadinessSnapshotV1, RunHistoryPageV1, RunViewV1, ServiceEventV2,
    SessionV1, WaitingPageV1,
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
async fn conversation_list_sessions(
    client: State<'_, LocalServiceClient>,
    after_session_id: Option<String>,
) -> Result<ConversationPageV1, DesktopCommandError> {
    client
        .list_sessions(after_session_id.as_deref())
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn conversation_list_runs(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    after_run_id: Option<String>,
) -> Result<RunHistoryPageV1, DesktopCommandError> {
    client
        .list_session_runs(&session_id, after_run_id.as_deref())
        .await
        .map_err(Into::into)
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
async fn conversation_start_localwrite_run(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    start_request_id: String,
    input: String,
) -> Result<RunViewV1, DesktopCommandError> {
    client
        .start_localwrite_run(&session_id, &start_request_id, &input)
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

#[tauri::command]
async fn approval_list_waiting(
    client: State<'_, LocalServiceClient>,
) -> Result<WaitingPageV1, DesktopCommandError> {
    client.list_waiting().await.map_err(Into::into)
}

#[tauri::command]
async fn approval_get_preview(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
    wait_id: String,
) -> Result<DesktopApprovalPreviewV1, DesktopCommandError> {
    client
        .approval_preview(&session_id, &run_id, &wait_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn approval_submit_decision(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
    wait_id: String,
    expected_row_version: u64,
    decision: ApprovalDecisionV1,
) -> Result<(), DesktopCommandError> {
    client
        .submit_decision(
            &session_id,
            &run_id,
            &wait_id,
            expected_row_version,
            decision,
        )
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn approval_resume_run(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
    wait_id: String,
) -> Result<(), DesktopCommandError> {
    client
        .resume_waiting(&session_id, &run_id, &wait_id)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn approval_abort_waiting(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
    wait_id: String,
    expected_row_version: u64,
) -> Result<(), DesktopCommandError> {
    client
        .abort_waiting(&session_id, &run_id, &wait_id, expected_row_version)
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
            conversation_list_sessions,
            conversation_list_runs,
            conversation_start_readonly_run,
            conversation_start_localwrite_run,
            conversation_run_status,
            conversation_cancel_run,
            conversation_read_events,
            approval_list_waiting,
            approval_get_preview,
            approval_submit_decision,
            approval_resume_run,
            approval_abort_waiting
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}

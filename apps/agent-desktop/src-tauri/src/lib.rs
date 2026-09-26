mod service_client;

use serde::Serialize;
use service_client::{
    ApprovalDecisionV1, BuildInfoV1, ConversationPageV1, DesktopApprovalPreviewV1,
    DesktopCompatibilityV1, DesktopReadinessSnapshotV1, HealthV1, LocalServiceClient,
    RunHistoryPageV1, RunViewV1, ServiceEventV2, SessionV1, WaitingPageV1,
};
use tauri::State;

const SERVICE_API_VERSION: u16 = 1;
const SERVICE_EVENT_VERSION: u16 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopBuildInfoV1 {
    application: &'static str,
    version: &'static str,
    git_revision: Option<&'static str>,
    build_profile: &'static str,
    service_api_version: u16,
    service_event_version: u16,
}

#[tauri::command]
fn desktop_build_info() -> DesktopBuildInfoV1 {
    DesktopBuildInfoV1 {
        application: "enterprise-local-agent-desktop",
        version: env!("CARGO_PKG_VERSION"),
        git_revision: option_env!("ELA_DESKTOP_GIT_REVISION"),
        build_profile: env!("ELA_DESKTOP_BUILD_PROFILE"),
        service_api_version: SERVICE_API_VERSION,
        service_event_version: SERVICE_EVENT_VERSION,
    }
}

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
async fn service_compatibility(
    client: State<'_, LocalServiceClient>,
) -> Result<DesktopCompatibilityV1, DesktopCommandError> {
    client.compatibility().await.map_err(Into::into)
}

#[tauri::command]
async fn service_readiness(
    client: State<'_, LocalServiceClient>,
) -> Result<DesktopReadinessSnapshotV1, DesktopCommandError> {
    client.require_compatible().await?;
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
    client.require_compatible().await?;
    client.create_session().await.map_err(Into::into)
}

#[tauri::command]
async fn conversation_list_sessions(
    client: State<'_, LocalServiceClient>,
    after_session_id: Option<String>,
) -> Result<ConversationPageV1, DesktopCommandError> {
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
    client
        .read_events(&session_id, &run_id, after_sequence)
        .await
        .map_err(Into::into)
}

#[tauri::command]
async fn approval_list_waiting(
    client: State<'_, LocalServiceClient>,
) -> Result<WaitingPageV1, DesktopCommandError> {
    client.require_compatible().await?;
    client.list_waiting().await.map_err(Into::into)
}

#[tauri::command]
async fn approval_get_preview(
    client: State<'_, LocalServiceClient>,
    session_id: String,
    run_id: String,
    wait_id: String,
) -> Result<DesktopApprovalPreviewV1, DesktopCommandError> {
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
    client.require_compatible().await?;
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
            desktop_build_info,
            service_health,
            service_compatibility,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_build_metadata_is_bounded_and_contract_specific() {
        let info = desktop_build_info();
        assert_eq!(info.application, "enterprise-local-agent-desktop");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(matches!(
            info.build_profile,
            "debug" | "release" | "unknown"
        ));
        assert_eq!(info.service_api_version, 1);
        assert_eq!(info.service_event_version, 2);
        assert!(info.git_revision.is_none_or(|revision| {
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        }));
    }
}

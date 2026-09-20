mod service_client;

use serde::Serialize;
use service_client::{BuildInfoV1, HealthV1, LocalServiceClient, ReadinessSnapshotV1};
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

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let client = LocalServiceClient::from_environment()?;
    tauri::Builder::default()
        .manage(client)
        .invoke_handler(tauri::generate_handler![
            service_health,
            service_readiness,
            service_version
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "service_health",
        "service_readiness",
        "service_version",
        "conversation_create_session",
        "conversation_start_readonly_run",
        "conversation_run_status",
        "conversation_cancel_run",
        "conversation_read_events",
    ]);
    if let Err(error) =
        tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
    {
        eprintln!("failed to prepare the desktop application: {error}");
        std::process::exit(1);
    }
}

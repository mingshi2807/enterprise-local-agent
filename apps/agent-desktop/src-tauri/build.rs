fn main() {
    println!("cargo:rerun-if-env-changed=ELA_GIT_REVISION");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=ELA_DESKTOP_BUILD_PROFILE={profile}");

    if let Ok(revision) = std::env::var("ELA_GIT_REVISION") {
        if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            eprintln!("ELA_GIT_REVISION must be a 40-character hexadecimal commit ID");
            std::process::exit(1);
        }
        println!("cargo:rustc-env=ELA_DESKTOP_GIT_REVISION={revision}");
    }

    let manifest = tauri_build::AppManifest::new().commands(&[
        "desktop_build_info",
        "service_health",
        "service_readiness",
        "service_version",
        "conversation_create_session",
        "conversation_list_sessions",
        "conversation_list_runs",
        "conversation_start_readonly_run",
        "conversation_start_localwrite_run",
        "conversation_run_status",
        "conversation_cancel_run",
        "conversation_read_events",
        "approval_list_waiting",
        "approval_get_preview",
        "approval_submit_decision",
        "approval_resume_run",
        "approval_abort_waiting",
    ]);
    if let Err(error) =
        tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
    {
        eprintln!("failed to prepare the desktop application: {error}");
        std::process::exit(1);
    }
}

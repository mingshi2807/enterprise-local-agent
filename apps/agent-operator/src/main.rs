use std::{env, path::PathBuf};

use agent_deployment::{
    CompatibilityManifestV1, DeploymentConfigV1, ListenerConfigV1, create_backup, restore_backup,
    verify_backup,
};
use anyhow::{Context, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

const CONFIG_ENV: &str = "ELA_DEPLOYMENT_CONFIG";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (config_path, command) = arguments()?;
    let config = DeploymentConfigV1::load(&config_path)?;
    let compatibility = CompatibilityManifestV1::from_config(
        &config,
        env!("CARGO_PKG_VERSION"),
        option_env!("ELA_GIT_IDENTITY").map(str::to_owned),
    )?;
    match command.as_slice() {
        [group, action] if group == "config" && action == "validate" => {
            print_json(&serde_json::json!({
                "valid": true,
                "schema_version": config.schema_version,
                "deployment_fingerprint": config.fingerprint()?.to_hex(),
            }))?
        }
        [command] if command == "readiness" => {
            print_online(&config, "/v1/operations/readiness").await?
        }
        [command] if command == "version" => {
            print_online(&config, "/v1/operations/version").await?
        }
        [group, action] if group == "runs" && action == "list" => {
            print_online(&config, "/v1/operations/runs?limit=64").await?;
        }
        [group, action, session_id, run_id] if group == "runs" && action == "show" => {
            validate_identifier(session_id)?;
            validate_identifier(run_id)?;
            print_online(
                &config,
                &format!("/v1/operations/runs/{session_id}/{run_id}"),
            )
            .await?;
        }
        [group, action] if group == "reconciliation" && action == "list" => {
            print_online(&config, "/v1/operations/reconciliation?limit=64").await?;
        }
        [group, action, session_id, run_id] if group == "reconciliation" && action == "show" => {
            validate_identifier(session_id)?;
            validate_identifier(run_id)?;
            print_online(
                &config,
                &format!("/v1/operations/runs/{session_id}/{run_id}"),
            )
            .await?;
        }
        [group, action, destination] if group == "backup" && action == "create" => {
            print_json(&create_backup(&config, &absolute(destination)?, compatibility).await?)?;
        }
        [group, action, source] if group == "backup" && action == "verify" => {
            print_json(&verify_backup(&absolute(source)?, &compatibility).await?)?;
        }
        [group, action, source] if group == "restore" && action == "verify" => {
            print_json(&verify_backup(&absolute(source)?, &compatibility).await?)?;
        }
        [group, action, source] if group == "restore" && action == "apply" => {
            restore_backup(&config, &absolute(source)?, &compatibility).await?;
            print_json(&serde_json::json!({ "restored": true }))?;
        }
        _ => bail!(usage()),
    }
    Ok(())
}

fn arguments() -> anyhow::Result<(PathBuf, Vec<String>)> {
    let mut values = env::args().skip(1).collect::<Vec<_>>();
    let config = if values.first().is_some_and(|value| value == "--config") {
        if values.len() < 3 {
            bail!(usage());
        }
        let path = PathBuf::from(values.remove(1));
        values.remove(0);
        path
    } else {
        env::var_os(CONFIG_ENV)
            .map(PathBuf::from)
            .context("ELA_DEPLOYMENT_CONFIG is required")?
    };
    if values.is_empty() {
        bail!(usage());
    }
    Ok((config, values))
}

fn usage() -> &'static str {
    "usage: agent-operator [--config ABSOLUTE_PATH] <config validate|readiness|version|runs list|runs show SESSION RUN|reconciliation list|reconciliation show SESSION RUN|backup create DIR|backup verify DIR|restore verify DIR|restore apply DIR>"
}

fn absolute(value: &str) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        bail!("path must be absolute");
    }
    Ok(path)
}

fn validate_identifier(value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("invalid identifier");
    }
    Ok(())
}

async fn print_online(config: &DeploymentConfigV1, path: &str) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_slice(&http_get(config, path).await?)
        .context("service returned invalid JSON")?;
    print_json(&value)
}

async fn http_get(config: &DeploymentConfigV1, path: &str) -> anyhow::Result<Vec<u8>> {
    match &config.listener {
        ListenerConfigV1::Unix { socket_path } => {
            let mut stream = tokio::net::UnixStream::connect(socket_path)
                .await
                .context("service unavailable")?;
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            stream.write_all(request.as_bytes()).await?;
            read_response(&mut stream).await
        }
        ListenerConfigV1::LoopbackTcp {
            address,
            bearer,
            allowed_hosts,
            ..
        } => {
            let secret = bearer.resolve()?;
            let bearer = std::str::from_utf8(secret.bytes()).context("bearer is not UTF-8")?;
            let host = allowed_hosts.first().context("Host allowlist is empty")?;
            let mut stream = tokio::net::TcpStream::connect(address)
                .await
                .context("service unavailable")?;
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {bearer}\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(request.as_bytes()).await?;
            read_response(&mut stream).await
        }
    }
}

async fn read_response<S: AsyncRead + Unpin>(stream: &mut S) -> anyhow::Result<Vec<u8>> {
    let mut response = Vec::new();
    stream
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)
        .await?;
    if response.len() > MAX_RESPONSE_BYTES {
        bail!("service response exceeded bound");
    }
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("malformed HTTP response")?;
    let headers = std::str::from_utf8(&response[..split]).context("malformed HTTP headers")?;
    let status = headers.lines().next().context("missing HTTP status")?;
    if !status.starts_with("HTTP/1.1 200 ") && !status.starts_with("HTTP/1.0 200 ") {
        bail!("operator request failed with sanitized service status");
    }
    Ok(response[split + 4..].to_vec())
}

fn print_json(value: &impl serde::Serialize) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

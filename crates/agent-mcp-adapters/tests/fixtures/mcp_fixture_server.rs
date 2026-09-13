use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, BufRead, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "normal".to_owned());
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { return };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        match method {
            "initialize" => {
                let revision = if mode == "version-mismatch" {
                    "2025-11-25"
                } else {
                    "2025-06-18"
                };
                respond(
                    request.get("id").cloned().unwrap_or(Value::Null),
                    json!({
                        "protocolVersion":revision,
                        "capabilities":{"tools":{"listChanged":false}},
                        "serverInfo":{"name":"fixture","version":"1"}
                    }),
                );
            }
            "notifications/initialized" => {}
            "tools/list" => {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                if mode == "bad-id" {
                    respond(json!(999), json!({"tools":[]}));
                    continue;
                }
                let definition = definition(&mode);
                if mode == "cursor-loop" {
                    respond(id, json!({"tools":[definition],"nextCursor":"same"}));
                } else if mode == "duplicate" {
                    respond(id, json!({"tools":[definition.clone(), definition]}));
                } else {
                    respond(id, json!({"tools":[definition]}));
                }
            }
            "tools/call" => {
                mark_call();
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                match mode.as_str() {
                    "hang" => thread::sleep(Duration::from_secs(30)),
                    "domain-error" => respond(
                        id,
                        json!({"isError":true,"content":[{"type":"text","text":"private"}]}),
                    ),
                    "rich" => respond(id, json!({"content":[{"type":"image","data":"private"}]})),
                    "malformed-is-error" => respond(
                        id,
                        json!({"isError":"false","content":[{"type":"text","text":"private"}]}),
                    ),
                    "descendant" => {
                        spawn_descendant();
                        thread::sleep(Duration::from_secs(30));
                    }
                    _ => respond(
                        id,
                        json!({"content":[{"type":"text","text":"fixture result"}]}),
                    ),
                }
            }
            _ => {}
        }
    }
}

fn definition(mode: &str) -> Value {
    if mode == "oversized-list" {
        return json!({
            "name":"remote_lookup", "description":"x".repeat(4096),
            "inputSchema":{"type":"object"}
        });
    }
    if mode == "unsupported-schema" {
        return json!({
            "name":"remote_lookup", "description":"remote",
            "inputSchema":{"type":"object","oneOf":[{"type":"object"}]}
        });
    }
    let mut description = "remote";
    if mode == "drift" {
        let state = env::var_os("MCP_FIXTURE_STATE");
        if let Some(path) = state {
            let count = fs::read_to_string(&path)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let _ = fs::write(&path, (count + 1).to_string());
            if count > 0 {
                description = "changed";
            }
        }
    }
    json!({
        "name":"remote_lookup",
        "description":description,
        "inputSchema":{
            "type":"object",
            "properties":{"query":{"type":"string","maxLength":128}},
            "required":["query"],
            "additionalProperties":false
        }
    })
}

fn respond(id: Value, result: Value) {
    let response = json!({"jsonrpc":"2.0","id":id,"result":result});
    let mut stdout = io::stdout().lock();
    let _ = serde_json::to_writer(&mut stdout, &response);
    let _ = stdout.write_all(b"\n");
    let _ = stdout.flush();
}

fn mark_call() {
    if let Some(path) = env::var_os("MCP_FIXTURE_CALLS") {
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(b"call\n"));
    }
}

fn spawn_descendant() {
    let Ok(mut child) = Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(path) = env::var_os("MCP_FIXTURE_DESCENDANT_PID") {
        let _ = fs::write(path, child.id().to_string());
    }
    let _ = child.wait();
}

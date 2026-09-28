# Enterprise Local Agent Quickstart

Enterprise Local Agent v1.0 is released for Linux x86_64. The desktop is a
thin client and requires separately operated runtime components:

1. `agent-service-daemon`, which owns governed execution.
2. A local OpenAI-compatible model, such as Qwen through `llama-server`.
3. An OCPP or Standards enterprise knowledge backend.
4. The Tauri desktop client.

The Debian package contains only the desktop. It does not install or start the
service, model, knowledge backend, Bubblewrap, or LocalWrite worker.

## Prerequisites

- A Debian-compatible Linux x86_64 desktop.
- Rust and Cargo for building the separately deployed service binaries.
- A reachable OpenAI-compatible model endpoint.
- One configured enterprise knowledge backend.
- The repository checked out locally.

LocalWrite has additional requirements described in
[Enable LocalWrite](#enable-localwrite).

## 1. Build the Service

From the repository root:

```bash
cargo build --release \
  -p agent-service-daemon \
  -p agent-operator
```

The resulting binaries are:

```text
target/release/agent-service-daemon
target/release/agent-operator
```

## 2. Configure the Deployment

Create a local configuration from the versioned example:

```bash
mkdir -p ~/.config/enterprise-local-agent
cp docs/deployment-config-v2.example.toml \
  ~/.config/enterprise-local-agent/deployment.toml

CFG="$(realpath ~/.config/enterprise-local-agent/deployment.toml)"
```

Edit `$CFG` and verify at least the following:

- `listener.socket_path` is an absolute path. Using
  `$XDG_RUNTIME_DIR/enterprise-local-agent.sock` lets the desktop discover it
  automatically. Prefer this default for the first deployment.
- The configured `unix_uid` and `unix_gid` match `id -u` and `id -g`.
- Storage paths are absolute and writable by the service account.
- `model.base_url` points to the trusted OpenAI-compatible endpoint.
- `model.model_id` is an exact model ID supported by that endpoint.
- `[knowledge]` selects and configures the OCPP, Standards, or federated route.
- `local_write_enabled = false` for the initial ReadOnly deployment.

For example, inspect the model IDs exposed by a loopback endpoint with:

```bash
curl -s http://127.0.0.1:18000/v1/models
```

No-auth model configuration is accepted only for a loopback endpoint. Use an
external secret reference and bearer authentication where required; never put
credentials directly in the deployment document.

Validate the configuration before starting the service:

```bash
./target/release/agent-operator \
  --config "$CFG" \
  config validate
```

## 3. Start and Check the Service

Start the daemon in one terminal:

```bash
ELA_DEPLOYMENT_CONFIG="$CFG" \
  ./target/release/agent-service-daemon
```

In another terminal, inspect readiness:

```bash
./target/release/agent-operator \
  --config "$CFG" \
  readiness
```

Do not continue until the service, model, knowledge backend, and
`enterprise-engineering-readonly-v1` workflow are ready.

## 4. Install the Desktop

Verify the checked-in release artifacts:

```bash
(
  cd release/v1.0.0
  sha256sum -c SHA256SUMS
)
```

Install the Debian package:

```bash
sudo apt install \
  "./release/v1.0.0/Enterprise Local Agent Desktop_1.0.0_amd64.deb"
```

Launch **Enterprise Local Agent Desktop** from the application menu or run:

```bash
agent-desktop
```

On Unix, the desktop checks `ELA_DESKTOP_SERVICE_SOCKET` first. Otherwise it
uses `$XDG_RUNTIME_DIR/enterprise-local-agent.sock`, followed by
`/run/enterprise-local-agent/agent.sock` on Linux.

For the default per-user deployment, do not set
`ELA_DESKTOP_SERVICE_SOCKET`. A value left over from an earlier terminal or
test deployment overrides automatic socket discovery. To launch once with any
stale override removed:

```bash
env -u ELA_DESKTOP_SERVICE_SOCKET agent-desktop
```

Set the variable only when the daemon's trusted deployment configuration uses
the exact same non-default absolute socket path.

## 5. Start a ReadOnly Conversation

1. Open **Settings > Runtime Status**.
2. Confirm that the service, model, knowledge backend, and ReadOnly workflow
   report **Ready**.
3. Select **New conversation**.
4. Keep the composer mode set to **Read only**.
5. Enter a bounded engineering question.
6. Press `Ctrl+Enter` or select **Send**.
7. Review the final answer and its trusted citations.

Use **Stop** to cancel an active run. Retrying starts a new run; it never
replays an old model invocation. Conversation and run history are maintained by
the service and are not stored as browser payloads.

## Enable LocalWrite

The `enterprise-engineering-localwrite-v1` workflow is registered only when
all LocalWrite readiness checks pass. It additionally requires:

- Bubblewrap 0.11.2 or later with functionally verified FD binding.
- The certified immutable static LocalWrite worker.
- A trusted, explicitly configured workspace.
- A stable `WorkspaceBindingId`.
- An action-seal key supplied through a trusted external secret reference.
- Correct Bubblewrap, worker, and tool-contract hashes.
- A successful M6.1 containment capability probe.

After an operator enables and validates the workflow:

1. Select **Local write** in the composer.
2. Submit a task that may propose `workspace_write_file`.
3. Review the trusted operation, relative target, and content byte count.
4. Select **Approve**, **Deny**, or **Abort** as appropriate.
5. After approval, select **Resume** explicitly.
6. Confirm the terminal result and the file inside the configured workspace.

Approval never directly executes the write. Policy, required audit, durable
action binding, and Linux containment remain server-side. With
`requester_must_differ`, a separately mapped approver must approve the
requester's action.

## Troubleshooting

| State | Meaning and action |
| --- | --- |
| Service unavailable | Start the daemon, remove stale desktop socket overrides, and verify that the desktop and deployment configuration use the same socket. Run the checks below before changing runtime configuration. |
| Service upgrade required | The desktop and service compatibility contracts do not match. Install compatible artifacts. |
| ReadOnly unavailable | Inspect readiness for model, knowledge, persistence, or audit failure. |
| LocalWrite unavailable | Verify containment artifacts, seal key, workspace binding, hashes, and the M6.1 capability probe. There is no ReadOnly downgrade. |
| Result unavailable | A volatile final answer was lost after service restart. The desktop does not regenerate it. |
| Requires attention | The run has an uncertain external effect and requires operator reconciliation. It is not retried automatically. |
| Unauthorized | The authenticated principal does not own the resource or lacks the required current role. |

For a default per-user socket, inspect the transport and metadata-only service
contracts directly:

```bash
SOCKET="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/enterprise-local-agent.sock"

test -S "$SOCKET"
curl --fail --silent --show-error --unix-socket "$SOCKET" \
  http://localhost/healthz
curl --fail --silent --show-error --unix-socket "$SOCKET" \
  http://localhost/v1/compatibility
curl --fail --silent --show-error --unix-socket "$SOCKET" \
  http://localhost/v1/operations/readiness
curl --fail --silent --show-error --unix-socket "$SOCKET" \
  http://localhost/v1/runtime/status
```

If these endpoints are healthy but the desktop still reports the service as
unavailable, confirm that the desktop package and service come from the same
compatible release set. In particular, use a desktop build that accepts the
service contract's bounded `max_read_page_items` value of 256; earlier builds
that incorrectly capped this metadata field at 64 reject an otherwise healthy
service response.

## Upgrade and Remove

Stop the desktop before upgrading. Install a compatible replacement package
with `apt`; the separately deployed service and its durable data are not part
of the desktop package:

```bash
sudo apt install "./Enterprise Local Agent Desktop_1.0.0_amd64.deb"
```

Remove only the desktop application with:

```bash
sudo apt remove enterprise-local-agent-desktop
```

Removing the desktop does not remove the service, model, knowledge backends,
SQLite state, audit records, containment worker, or operator configuration.

## Security Model

The desktop is a request and observation client. Its named Tauri commands talk
to `agent-service-daemon`; it has no generic shell, filesystem, HTTP, SQL,
model, MCP, persistence, policy, approval, or containment authority. The
service owns identity, authorization, budgets, audit, persistence, recovery,
policy, approval, and execution. Retrieved knowledge and model output remain
untrusted data. LocalWrite executes only after typed validation, policy,
required audit, exact-action approval binding, and the certified Linux
containment path.

## Known Limitations

- Version 1.0 supports Linux x86_64 only. macOS validation is post-v1.
- The OCPP knowledge route is unavailable when its corpus is empty; Standards
  remains independently usable when configured.
- A local model that does not satisfy the strict structured-output contract
  fails closed. The runtime does not heuristically reinterpret prose.
- Global exactly-once execution is not claimed. Uncertain external effects are
  surfaced for manual reconciliation and are not automatically retried.
- The delegated two-principal approval scenario and deliberately induced real
  ManualReconciliationRequired desktop state are covered deterministically but
  have not been exercised in the real acceptance environment.

## Operator Inspection

Operational inspection commands include:

```bash
./target/release/agent-operator --config "$CFG" runs list
./target/release/agent-operator --config "$CFG" reconciliation list
./target/release/agent-operator --config "$CFG" version
```

The operator CLI cannot approve, resume, change policy, or access runtime
ports. See [README.md](README.md),
[the deployment example](docs/deployment-config-v2.example.toml), and
[desktop release documentation](docs/release/desktop.md) for more detail.

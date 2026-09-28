# Feature Development Loop

Use `dev-loop.sh` for normal feature development without rebuilding or
installing the Debian package. The script is executable from Fish, Bash, or
another shell; do not source it.

## First-Time Setup

Install the native Tauri development dependencies once:

```fish
sudo apt install pkg-config libglib2.0-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

Install the desktop JavaScript dependencies if they are not already present:

```fish
cd apps/agent-desktop
npm install
cd ../..
```

Check the development environment and local service connection:

```fish
./scripts/dev-loop.sh doctor
```

## Runtime Prerequisites

Before using `run` or `all`, keep these existing dependencies running:

1. The SSH tunnel to the local OpenAI-compatible model.
2. The configured enterprise knowledge backends.
3. `agent-service-daemon` on the configured Unix socket.

The default desktop socket is:

```text
$XDG_RUNTIME_DIR/enterprise-local-agent.sock
```

Override it when required:

```fish
set -x ELA_DESKTOP_SERVICE_SOCKET /absolute/path/to/agent.sock
```

## Recommended Order

For a normal feature-development cycle, run:

```fish
./scripts/dev-loop.sh all
```

`all` performs the following operations in order:

1. TypeScript type checking.
2. ESLint validation.
3. Frontend tests.
4. Frontend production build.
5. Targeted `agent-service` and `agent-service-http` tests.
6. Desktop Rust tests.
7. Debug builds of `agent-service-daemon` and `agent-operator`.
8. Tauri development launch with Vite hot reload.

The final command remains active until the desktop development process is
closed.

## Faster UI-Only Cycle

For frontend-only changes, validate and launch separately:

```fish
./scripts/dev-loop.sh frontend
./scripts/dev-loop.sh run
```

Use this path when no Rust bridge or service contract changed.

## Individual Modes

```fish
./scripts/dev-loop.sh doctor
```

Checks required tools, native Tauri libraries, installed desktop dependencies,
and the service socket.

```fish
./scripts/dev-loop.sh frontend
```

Runs frontend type checking, linting, tests, and the production frontend build.

```fish
./scripts/dev-loop.sh check
```

Runs frontend validation, targeted Rust tests, and debug service builds without
launching the desktop.

```fish
./scripts/dev-loop.sh run
```

Checks prerequisites and launches Tauri development mode against the existing
service. It does not repeat the validation suite.

```fish
./scripts/dev-loop.sh all
```

Runs `check` and then launches Tauri development mode. This is the default when
no mode is provided:

```fish
./scripts/dev-loop.sh
```

## Scope

The development loop does not:

- start or configure the model server;
- establish the SSH tunnel;
- start enterprise knowledge backends;
- modify deployment configuration;
- create or install a `.deb` package;
- replace `/usr/bin/agent-desktop`;
- run the complete release-certification suite.

Perform packaging, clean installation, native lifecycle testing, and full
release gates only after the feature batch is stable.

## Note

The installed `.deb` does not affect `dev-loop.sh`.

The script launches:

```
npm run tauri -- dev
```

This builds and runs the repository’s debug desktop, not `/usr/bin/agent-desktop`.

To avoid confusion:

- Close the installed desktop before running `dev-loop.sh`.
- Do not launch `agent-desktop` from the application menu during development.
- Keep only the Tauri development window open.
- Both clients may access the same service, but the service prevents duplicate run/resume operations.

The installed package can remain for stable usage. Remove it only if distinguishing stable and development windows becomes confusing:

```
sudo dpkg -r enterprise-local-agent-desktop
```

## launcher

```
Running env ELA_DESKTOP_SERVICE_SOCKET=/run/user/1000/enterprise-local-agent.sock /usr/bin/agent-desktop
```

The installed desktop stack is active.

- Qwen tunnel: **ready**
- OCPP knowledge backend: **ready**
- Standards backend: **ready**
- `agent-service-daemon`: **running**
- Operator readiness check: **passed**
- ReadOnly workflow: **ready**
- LocalWrite workflow: **ready**
- Linux containment probe: **passed**
- Installed `/usr/bin/agent-desktop`: **running**
- Service socket: `/run/user/1000/enterprise-local-agent.sock`

The deployment configuration was restored at:

```
~/.config/enterprise-local-agent/deployment.toml
```

The previous `/tmp` database had been removed, so this activation starts with new disposable run history and a new LocalWrite seal key.

The startup is long because the desktop is only a thin client: the model tunnel, OCPP backend, Standards adapter readiness, daemon, containment probe, and operator check must be available first. I’ll add one Fish-callable executable that starts only missing components, waits for readiness, runs the operator check, and launches the installed desktop; it will also support `status`, `stop`, and `restart`.

Created executable scripts/start-installed.sh.

From Fish, start everything with:

```
./scripts/start-installed.sh
```

Available commands:

```
./scripts/start-installed.sh start
./scripts/start-installed.sh status
./scripts/start-installed.sh stop
./scripts/start-installed.sh restart
```

It automatically:

1. Reuses or establishes the Qwen SSH tunnel.
2. Reuses or starts the OCPP backend.
3. Reuses or starts `agent-service-daemon`.
4. Waits for readiness.
5. Runs the operator readiness check.
6. Launches `/usr/bin/agent-desktop`.
7. Avoids duplicate desktop processes.

`stop` only terminates processes started by this script. Logs are stored under:

```
~/.local/state/enterprise-local-agent/logs/
```

Syntax, status detection, idempotent startup, and full readiness were verified successfully

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

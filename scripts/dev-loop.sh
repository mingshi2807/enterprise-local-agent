#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
DESKTOP_DIR="${REPO_ROOT}/apps/agent-desktop"

DEFAULT_SOCKET="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/enterprise-local-agent.sock"
SERVICE_SOCKET="${ELA_DESKTOP_SERVICE_SOCKET:-${DEFAULT_SOCKET}}"

usage() {
  cat <<'EOF'
Usage: ./scripts/dev-loop.sh [frontend|check|run|all|doctor]

Commands:
  frontend  Run TypeScript, ESLint, frontend tests, and production frontend build.
  check     Run frontend checks, targeted Rust tests, and debug service builds.
  run       Verify local prerequisites/service, then start Tauri dev with hot reload.
  all       Run check, then run (default).
  doctor    Check tools, native Tauri libraries, dependencies, and service connectivity.

This script is executable from Bash, Fish, or another shell. Do not source it.

Environment:
  ELA_DESKTOP_SERVICE_SOCKET  Service Unix socket. Defaults to
                              $XDG_RUNTIME_DIR/enterprise-local-agent.sock.

The script does not start external dependencies, package a .deb, install software,
or modify deployment configuration.
EOF
}

fail() {
  printf 'dev-loop: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

check_native_dependencies() {
  require_command pkg-config

  local missing=()
  local package
  for package in glib-2.0 gtk+-3.0 webkit2gtk-4.1; do
    if ! pkg-config --exists "${package}"; then
      missing+=("${package}")
    fi
  done

  if ((${#missing[@]} > 0)); then
    printf 'dev-loop: missing native Tauri development packages: %s\n' "${missing[*]}" >&2
    printf '%s\n' \
      'On Ubuntu/Debian install them once with:' \
      '  sudo apt install pkg-config libglib2.0-dev libgtk-3-dev \' \
      '    libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf' >&2
    return 1
  fi
}

check_tools() {
  require_command cargo
  require_command curl
  require_command node
  require_command npm
  check_native_dependencies

  [[ -d "${DESKTOP_DIR}/node_modules" ]] ||
    fail "desktop dependencies are missing; run: (cd apps/agent-desktop && npm install)"
}

check_service() {
  [[ "${SERVICE_SOCKET}" = /* ]] || fail "service socket must be an absolute path"
  [[ -S "${SERVICE_SOCKET}" ]] || fail "service socket is unavailable: ${SERVICE_SOCKET}"

  local health
  health="$(curl --fail --silent --show-error --max-time 5 \
    --unix-socket "${SERVICE_SOCKET}" http://localhost/healthz)" ||
    fail "agent service health check failed on ${SERVICE_SOCKET}"

  [[ "${health}" == *'"lifecycle":"serving"'* ]] ||
    fail "agent service is not serving on ${SERVICE_SOCKET}"
}

frontend_checks() {
  printf '%s\n' '==> Frontend typecheck'
  npm --prefix "${DESKTOP_DIR}" run typecheck

  printf '%s\n' '==> Frontend lint'
  npm --prefix "${DESKTOP_DIR}" run lint

  printf '%s\n' '==> Frontend tests'
  npm --prefix "${DESKTOP_DIR}" test

  printf '%s\n' '==> Frontend production build'
  npm --prefix "${DESKTOP_DIR}" run build
}

targeted_checks() {
  check_tools
  frontend_checks

  printf '%s\n' '==> Targeted service tests'
  cargo test --locked --manifest-path "${REPO_ROOT}/Cargo.toml" \
    -p agent-service \
    -p agent-service-http

  printf '%s\n' '==> Desktop Rust tests'
  cargo test --locked --manifest-path "${REPO_ROOT}/Cargo.toml" -p agent-desktop

  printf '%s\n' '==> Debug service binaries'
  cargo build --locked --manifest-path "${REPO_ROOT}/Cargo.toml" \
    -p agent-service-daemon \
    -p agent-operator
}

run_desktop() {
  check_tools
  check_service

  printf '==> Starting Tauri dev against %s\n' "${SERVICE_SOCKET}"
  cd "${DESKTOP_DIR}"
  exec env ELA_DESKTOP_SERVICE_SOCKET="${SERVICE_SOCKET}" npm run tauri -- dev
}

main() {
  local command="${1:-all}"
  if (($# > 1)); then
    usage >&2
    exit 2
  fi

  case "${command}" in
    frontend)
      require_command node
      require_command npm
      [[ -d "${DESKTOP_DIR}/node_modules" ]] ||
        fail "desktop dependencies are missing; run: (cd apps/agent-desktop && npm install)"
      frontend_checks
      ;;
    check)
      targeted_checks
      ;;
    run)
      run_desktop
      ;;
    all)
      targeted_checks
      run_desktop
      ;;
    doctor)
      check_tools
      check_service
      printf 'dev-loop: ready (service socket: %s)\n' "${SERVICE_SOCKET}"
      ;;
    -h|--help|help)
      usage
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
}

main "$@"

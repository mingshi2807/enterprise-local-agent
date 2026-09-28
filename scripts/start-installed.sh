#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

CONFIG="${ELA_INSTALLED_CONFIG:-${HOME}/.config/enterprise-local-agent/deployment.toml}"
SERVICE_SOCKET="${ELA_DESKTOP_SERVICE_SOCKET:-${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/enterprise-local-agent.sock}"
MODEL_URL="${ELA_MODEL_MODELS_URL:-http://127.0.0.1:18000/v1/models}"
OCPP_HEALTH_URL="${ELA_OCPP_HEALTH_URL:-http://127.0.0.1:8000/health}"

SSH_TARGET="${ELA_MODEL_SSH_TARGET:-yj@192.168.1.26}"
SSH_LOCAL_PORT="${ELA_MODEL_LOCAL_PORT:-18000}"
SSH_REMOTE_HOST="${ELA_MODEL_REMOTE_HOST:-127.0.0.1}"
SSH_REMOTE_PORT="${ELA_MODEL_REMOTE_PORT:-8080}"

OCPP_ROOT="${ELA_OCPP_ROOT:-/home/ming/workspace/rag-kag-emobility.git}"
DAEMON="${ELA_SERVICE_DAEMON:-${REPO_ROOT}/target/release/agent-service-daemon}"
OPERATOR="${ELA_OPERATOR:-${REPO_ROOT}/target/release/agent-operator}"
DESKTOP="${ELA_DESKTOP_BIN:-/usr/bin/agent-desktop}"

STATE_HOME="${XDG_STATE_HOME:-${HOME}/.local/state}/enterprise-local-agent"
RUNTIME_HOME="${XDG_RUNTIME_DIR:-/tmp}/ela-installed"
LOG_DIR="${STATE_HOME}/logs"
PID_DIR="${RUNTIME_HOME}/pids"
SSH_CONTROL="${RUNTIME_HOME}/model-tunnel.sock"

usage() {
  cat <<'EOF'
Usage: ./scripts/start-installed.sh [start|status|stop|restart]

  start    Start missing dependencies, verify readiness, and launch the installed desktop.
  status   Show bounded connectivity and process status.
  stop     Stop only processes and tunnel started by this script.
  restart  Stop script-owned processes, then start the stack again.

The default command is start. The script is directly executable from Fish.
Configuration can be overridden with ELA_INSTALLED_CONFIG and the ELA_* variables
defined near the top of this script. No credentials are stored by this script.
EOF
}

say() {
  printf 'installed-agent: %s\n' "$*"
}

fail() {
  printf 'installed-agent: %s\n' "$*" >&2
  exit 1
}

prepare_runtime() {
  mkdir -p "${LOG_DIR}" "${PID_DIR}"
  chmod 0700 "${STATE_HOME}" "${LOG_DIR}" "${RUNTIME_HOME}" "${PID_DIR}"
}

require_file() {
  [[ -f "$1" ]] || fail "required file is missing: $1"
}

require_executable() {
  [[ -x "$1" ]] || fail "required executable is missing: $1"
}

url_ready() {
  curl --fail --silent --show-error --max-time 5 "$1" >/dev/null 2>&1
}

service_ready() {
  [[ -S "${SERVICE_SOCKET}" ]] &&
    curl --fail --silent --show-error --max-time 5 \
      --unix-socket "${SERVICE_SOCKET}" http://localhost/healthz >/dev/null 2>&1
}

wait_for_url() {
  local label="$1"
  local url="$2"
  local attempts="$3"
  local index
  for ((index = 1; index <= attempts; index += 1)); do
    if url_ready "${url}"; then
      say "${label} is ready"
      return 0
    fi
    sleep 1
  done
  fail "${label} did not become ready; inspect ${LOG_DIR}"
}

wait_for_service() {
  local index
  for ((index = 1; index <= 180; index += 1)); do
    if service_ready; then
      say "agent service is ready"
      return 0
    fi
    sleep 1
  done
  fail "agent service did not become ready; inspect ${LOG_DIR}/agent-service.log"
}

pid_is_owned() {
  local pid_file="$1"
  local marker="$2"
  local pid
  [[ -f "${pid_file}" ]] || return 1
  read -r pid <"${pid_file}" || return 1
  [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
  [[ -r "/proc/${pid}/cmdline" ]] || return 1
  tr '\0' ' ' <"/proc/${pid}/cmdline" | grep --fixed-strings --quiet "${marker}"
}

start_model_tunnel() {
  if url_ready "${MODEL_URL}"; then
    say "model endpoint is ready"
    return
  fi

  command -v ssh >/dev/null 2>&1 || fail "ssh is required to start the model tunnel"
  rm -f "${SSH_CONTROL}"
  say "starting model tunnel through ${SSH_TARGET}"
  ssh -M -S "${SSH_CONTROL}" -fNT \
    -o ExitOnForwardFailure=yes \
    -o ServerAliveInterval=30 \
    -o ServerAliveCountMax=3 \
    -L "127.0.0.1:${SSH_LOCAL_PORT}:${SSH_REMOTE_HOST}:${SSH_REMOTE_PORT}" \
    "${SSH_TARGET}"
  wait_for_url "model endpoint" "${MODEL_URL}" 20
}

start_ocpp() {
  if url_ready "${OCPP_HEALTH_URL}"; then
    say "OCPP knowledge backend is ready"
    return
  fi

  local uvicorn="${OCPP_ROOT}/.venv/bin/uvicorn"
  require_executable "${uvicorn}"
  [[ -d "${OCPP_ROOT}/models" ]] || fail "OCPP model cache is missing: ${OCPP_ROOT}/models"

  say "starting OCPP knowledge backend"
  (
    cd "${OCPP_ROOT}"
    exec nohup env HF_HOME=./models "${uvicorn}" \
      'rag_ocpp.api.app:create_app' --factory --host 127.0.0.1 --port 8000
  ) >>"${LOG_DIR}/ocpp.log" 2>&1 &
  printf '%s\n' "$!" >"${PID_DIR}/ocpp.pid"
  wait_for_url "OCPP knowledge backend" "${OCPP_HEALTH_URL}" 90
}

start_daemon() {
  if service_ready; then
    say "agent service is ready"
    return
  fi

  require_file "${CONFIG}"
  require_executable "${DAEMON}"
  require_executable "${OPERATOR}"

  if [[ -S "${SERVICE_SOCKET}" ]]; then
    rm -f "${SERVICE_SOCKET}"
  fi

  say "starting agent service daemon"
  (
    cd "${REPO_ROOT}"
    exec nohup env ELA_DEPLOYMENT_CONFIG="${CONFIG}" RUST_LOG=info "${DAEMON}"
  ) >>"${LOG_DIR}/agent-service.log" 2>&1 &
  printf '%s\n' "$!" >"${PID_DIR}/agent-service.pid"
  wait_for_service
}

run_operator_readiness() {
  say "running operator readiness check"
  "${OPERATOR}" --config "${CONFIG}" readiness
}

start_desktop() {
  if pgrep -x agent-desktop >/dev/null 2>&1; then
    say "installed desktop is already running"
    return
  fi

  require_executable "${DESKTOP}"
  say "launching installed desktop"
  nohup env ELA_DESKTOP_SERVICE_SOCKET="${SERVICE_SOCKET}" "${DESKTOP}" \
    >>"${LOG_DIR}/desktop.log" 2>&1 &
  printf '%s\n' "$!" >"${PID_DIR}/desktop.pid"
  sleep 2
  pid_is_owned "${PID_DIR}/desktop.pid" "agent-desktop" ||
    fail "desktop exited during startup; inspect ${LOG_DIR}/desktop.log"
}

stop_owned_process() {
  local label="$1"
  local pid_file="$2"
  local marker="$3"
  local pid

  if ! pid_is_owned "${pid_file}" "${marker}"; then
    rm -f "${pid_file}"
    say "${label} is not owned by this script"
    return
  fi

  read -r pid <"${pid_file}"
  say "stopping ${label} (pid ${pid})"
  kill -TERM "${pid}"
  local index
  for ((index = 1; index <= 20; index += 1)); do
    if ! kill -0 "${pid}" 2>/dev/null; then
      rm -f "${pid_file}"
      return
    fi
    sleep 0.25
  done
  fail "${label} did not stop after SIGTERM"
}

stop_tunnel() {
  if [[ ! -S "${SSH_CONTROL}" ]]; then
    say "model tunnel is not owned by this script"
    return
  fi
  say "stopping script-owned model tunnel"
  ssh -S "${SSH_CONTROL}" -O exit "${SSH_TARGET}" >/dev/null 2>&1 || true
  rm -f "${SSH_CONTROL}"
}

start_stack() {
  prepare_runtime
  command -v curl >/dev/null 2>&1 || fail "curl is required"
  start_model_tunnel
  start_ocpp
  start_daemon
  run_operator_readiness
  start_desktop
  say "installed desktop stack is active"
}

stop_stack() {
  prepare_runtime
  stop_owned_process "desktop" "${PID_DIR}/desktop.pid" "agent-desktop"
  stop_owned_process "agent service" "${PID_DIR}/agent-service.pid" "agent-service-daemon"
  stop_owned_process "OCPP knowledge backend" "${PID_DIR}/ocpp.pid" "uvicorn"
  stop_tunnel
}

print_status() {
  prepare_runtime
  if url_ready "${MODEL_URL}"; then say "model: ready"; else say "model: unavailable"; fi
  if url_ready "${OCPP_HEALTH_URL}"; then say "OCPP: ready"; else say "OCPP: unavailable"; fi
  if service_ready; then say "agent service: ready"; else say "agent service: unavailable"; fi
  if pgrep -x agent-desktop >/dev/null 2>&1; then say "desktop: running"; else say "desktop: stopped"; fi
}

main() {
  local command="${1:-start}"
  if (($# > 1)); then
    usage >&2
    exit 2
  fi

  case "${command}" in
    start) start_stack ;;
    status) print_status ;;
    stop) stop_stack ;;
    restart)
      stop_stack
      start_stack
      ;;
    -h|--help|help) usage ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
}

main "$@"

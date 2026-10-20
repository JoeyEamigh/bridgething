#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEV_DIR="${BRIDGETHING_DEV_DIR:-${ROOT}/.dev}"
PIDFILE="${DEV_DIR}/dev-daemon.pid"
LOGFILE="${DEV_DIR}/dev-daemon.log"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT}/target}"
BIN="${TARGET_DIR}/debug/bridgething"
GATEWAY_HOST=127.0.0.1
GATEWAY_PORT=8892
START_TIMEOUT=60
STOP_TIMEOUT=20

running_pid() {
    [ -f "$PIDFILE" ] || return 1
    local pid
    pid="$(cat "$PIDFILE")"
    [ -n "$pid" ] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    echo "$pid"
}

gateway_up() {
    (exec 3<>"/dev/tcp/${GATEWAY_HOST}/${GATEWAY_PORT}") 2>/dev/null
}

claim_gateway() {
    local pid
    if pid="$(running_pid)"; then
        echo "[dev-daemon] already running (pid ${pid}); stop it first" >&2
        exit 1
    fi
    rm -f "$PIDFILE"
    if gateway_up; then
        echo "[dev-daemon] something already listens on ${GATEWAY_HOST}:${GATEWAY_PORT}; stop it first" >&2
        exit 1
    fi
}

enter_dev_env() {
    mkdir -p "${DEV_DIR}/state" "${DEV_DIR}/webapps" "${DEV_DIR}/webapp-blobs" "${DEV_DIR}/examples"

    local wakeword_model="${BRIDGETHING_WAKEWORD_MODEL:-${DEV_DIR}/wakeword/hey_bridgething.btww}"
    if [ ! -f "$wakeword_model" ]; then
        mkdir -p "$(dirname "$wakeword_model")"
        head -c 4096 /dev/zero | tr '\0' 'w' >"$wakeword_model"
        printf '%s\n' "${BRIDGETHING_WAKEWORD_VERSION:-0.1.0}" >"${wakeword_model%.btww}.version"
    fi

    export BRIDGETHING_STATE_DIR="${DEV_DIR}/state"
    export BRIDGETHING_WEBAPPS_DIR="${DEV_DIR}/webapps"
    export BRIDGETHING_WEBAPP_BLOBS_DIR="${DEV_DIR}/webapp-blobs"
    export BRIDGETHING_EXAMPLES_DIR="${DEV_DIR}/examples"
    export BRIDGETHING_WAKEWORD_MODEL="$wakeword_model"
    export RUST_LOG="${RUST_LOG:-bridgething=debug,bridgething::chrome=info,libbridgething=info}"
}

do_run() {
    claim_gateway
    enter_dev_env
    exec cargo run -p bridgething --features test-tap -- --dev
}

do_start() {
    local pid
    claim_gateway

    echo "[dev-daemon] building"
    cargo build -p bridgething --features test-tap

    enter_dev_env
    echo "[dev-daemon] launching, logging to ${LOGFILE}"
    : >"$LOGFILE"
    nohup "$BIN" --dev </dev/null >>"$LOGFILE" 2>&1 &
    pid=$!
    echo "$pid" >"$PIDFILE"

    local waited=0
    while [ "$waited" -lt "$START_TIMEOUT" ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            rm -f "$PIDFILE"
            echo "[dev-daemon] daemon exited during startup; see ${LOGFILE}" >&2
            exit 1
        fi
        if gateway_up; then
            echo "[dev-daemon] up (pid ${pid}), gateway on ws://${GATEWAY_HOST}:${GATEWAY_PORT}/"
            return
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "[dev-daemon] running (pid ${pid}) but ${GATEWAY_HOST}:${GATEWAY_PORT} did not open in ${START_TIMEOUT}s" >&2
    exit 1
}

do_stop() {
    local pid
    if ! pid="$(running_pid)"; then
        rm -f "$PIDFILE"
        echo "[dev-daemon] not running"
        return
    fi

    echo "[dev-daemon] sending SIGTERM to ${pid}"
    kill -TERM "$pid" 2>/dev/null || true

    local waited=0
    while [ "$waited" -lt "$STOP_TIMEOUT" ]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            rm -f "$PIDFILE"
            echo "[dev-daemon] stopped (pid ${pid})"
            return
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "[dev-daemon] pid ${pid} still alive after ${STOP_TIMEOUT}s" >&2
    exit 1
}

do_status() {
    local pid
    if pid="$(running_pid)"; then
        echo "[dev-daemon] running (pid ${pid})"
    else
        echo "[dev-daemon] not running"
    fi

    if gateway_up; then
        echo "[dev-daemon] ${GATEWAY_HOST}:${GATEWAY_PORT} reachable"
    else
        echo "[dev-daemon] ${GATEWAY_HOST}:${GATEWAY_PORT} unreachable"
    fi

    [ -n "${pid:-}" ] || exit 1
}

case "${1:-}" in
run) do_run ;;
start) do_start ;;
stop) do_stop ;;
status) do_status ;;
*)
    echo "usage: dev-daemon.sh {run|start|stop|status}" >&2
    exit 2
    ;;
esac

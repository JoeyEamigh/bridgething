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
    timeout 1 bash -c "exec 3<>/dev/tcp/${GATEWAY_HOST}/${GATEWAY_PORT}" 2>/dev/null
}

do_start() {
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

    mkdir -p "${DEV_DIR}/state" "${DEV_DIR}/webapps" "${DEV_DIR}/examples"

    echo "[dev-daemon] building"
    cargo build -p bridgething --features test-tap

    echo "[dev-daemon] launching, logging to ${LOGFILE}"
    : >"$LOGFILE"
    BRIDGETHING_STATE_DIR="${DEV_DIR}/state" \
        BRIDGETHING_WEBAPPS_DIR="${DEV_DIR}/webapps" \
        BRIDGETHING_EXAMPLES_DIR="${DEV_DIR}/examples" \
        RUST_LOG="${RUST_LOG:-bridgething=debug,bridgething::chrome=info,libbridgething=info}" \
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
start) do_start ;;
stop) do_stop ;;
status) do_status ;;
*)
    echo "usage: dev-daemon.sh {start|stop|status}" >&2
    exit 2
    ;;
esac

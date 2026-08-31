// Generated from @bridgething/webapp-shared.
/// <reference types="vite/client" />

const OFF_DEVICE_URL = 'ws://127.0.0.1:8891/';

export const DAEMON_PROXY_PATH = '/__bridgething';

export function daemonUrl(): string {
  const override = import.meta.env.VITE_BRIDGETHING_URL;
  if (override) return override;
  if (typeof window === 'undefined') return OFF_DEVICE_URL;
  const { host } = window.location;
  return import.meta.env.DEV ? `ws://${host}${DAEMON_PROXY_PATH}/` : `ws://${host}/`;
}

export * from './bindings/shared.js';
export * from './bindings/stock.js';
export * from './bindings/wire.js';

export * from './logger.js';
export * from './uuid.js';

export const BRIDGETHING_PROFILE_UUID = 'dead0000-854d-408e-81f0-fb6147f918fd';
export const BRIDGETHING_RFCOMM_CHANNEL = 1;
export const BRIDGETHING_MANUFACTURER_ID = 0xdead;

export const BRIDGETHING_WS_PORT = 8891;
export const BRIDGETHING_FILE_PORT = 8891;
export const BRIDGETHING_NETWORK_GATEWAY_PORT = 8892;

export const BRIDGETHING_MDNS_SERVICE_TYPE = 'bridgething';
export const BRIDGETHING_DEFAULT_HOST = 'bridgething.local';
export const BRIDGETHING_NETWORK_GATEWAY_URL = `ws://${BRIDGETHING_DEFAULT_HOST}:${BRIDGETHING_NETWORK_GATEWAY_PORT}/`;

import { version } from './version.js';
export const LIB_VERSION = `v${version}`;
export const LIBBRIDGETHING_VERSION = 'v0.12.1';

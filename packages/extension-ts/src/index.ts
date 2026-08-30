import type { ExtensionSpec } from './context.js';
import { denoHost } from './deno.js';
import type { ExtensionHost } from './host.js';
import { ExtensionRuntime } from './runtime.js';

export type {
  AuthBridge,
  ConfigListener,
  Device,
  DeviceEvent,
  DeviceListener,
  DeviceRef,
  ExtensionContext,
  ExtensionLog,
  ExtensionSpec,
  KvStore,
  MessageListener,
  Unsubscribe,
} from './context.js';
export { denoHost } from './deno.js';
export { ExtensionError, type ExtensionHost } from './host.js';
export { asBinary, asJson, asText, binary, json, text, type ForwardMessage, type SendableMessage } from './message.js';
export {
  EXTENSION_API_VERSION,
  type ExtensionMessage,
  type ExtensionWebapp,
  type HostMessage,
  type LogLevel,
  type WireForwardMessage,
} from './protocol.js';

/**
 * Runs an extension. Call it once at the top level of the entry module.
 *
 * The host protocol owns stdin and stdout, so `console.log` corrupts it. Log with `ctx.log`.
 *
 * ```ts
 * defineExtension({
 *   start(ctx) {
 *     ctx.on('message', (device, message) => device.send(asJson(message) ?? 'pong'));
 *   },
 * });
 * ```
 *
 * The returned promise settles after `stop()` runs, once the host sends `stop` or closes stdin.
 * Pass `host` to run the same runtime over streams you own.
 */
export function defineExtension(spec: ExtensionSpec, host: ExtensionHost = denoHost()): Promise<void> {
  return new ExtensionRuntime(spec, host).run();
}

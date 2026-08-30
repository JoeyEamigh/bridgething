import { ExtensionError, type ExtensionHost } from './host.js';

type DenoRuntime = {
  stdin: { readable: ReadableStream<Uint8Array> };
  stdout: { writable: WritableStream<Uint8Array> };
  exit(code: number): never;
};

/** The host protocol over `Deno.stdin` and `Deno.stdout`. `defineExtension` uses this transport by default. */
export function denoHost(): ExtensionHost {
  const runtime = (globalThis as unknown as { Deno?: DenoRuntime }).Deno;
  if (!runtime) {
    throw new ExtensionError('extensions run under deno; no Deno global in this process', 'no-runtime');
  }
  return {
    readable: runtime.stdin.readable,
    writable: runtime.stdout.writable,
    exit: code => runtime.exit(code),
  };
}

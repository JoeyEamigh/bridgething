import type { ExtensionMessage } from './protocol.js';

/**
 * The transport an extension runs over. The two byte streams carry the newline-delimited host
 * protocol, and `exit` ends the process. Pass in-memory streams to drive a runtime from a test.
 */
export type ExtensionHost = {
  readable: ReadableStream<Uint8Array>;
  writable: WritableStream<Uint8Array>;
  exit(code: number): void;
};

/** Every failure this package raises. Branch on `kind`. */
export class ExtensionError extends Error {
  constructor(
    message: string,
    public readonly kind: 'host-error' | 'disconnected' | 'write-failed' | 'no-runtime',
  ) {
    super(message);
    this.name = 'ExtensionError';
  }
}

export async function* readLines(reader: ReadableStreamDefaultReader<Uint8Array>): AsyncGenerator<string> {
  const decoder = new TextDecoder();
  let buffered = '';
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffered += decoder.decode(value, { stream: true });
      for (let cut = buffered.indexOf('\n'); cut >= 0; cut = buffered.indexOf('\n')) {
        const line = buffered.slice(0, cut);
        buffered = buffered.slice(cut + 1);
        if (line.trim().length > 0) yield line;
      }
    }
    buffered += decoder.decode();
    if (buffered.trim().length > 0) yield buffered;
  } finally {
    reader.releaseLock();
  }
}

export class LineWriter {
  private readonly writer: WritableStreamDefaultWriter<Uint8Array>;
  private readonly encoder = new TextEncoder();
  private tail: Promise<void> = Promise.resolve();

  constructor(writable: WritableStream<Uint8Array>) {
    this.writer = writable.getWriter();
  }

  write(message: ExtensionMessage): Promise<void> {
    const chunk = this.encoder.encode(`${JSON.stringify(message)}\n`);
    const written = this.tail.then(() => this.writer.write(chunk));
    this.tail = written.catch(() => undefined);
    return written;
  }

  async close(): Promise<void> {
    await this.tail;
    try {
      await this.writer.close();
    } catch {
      this.writer.releaseLock();
    }
  }
}

import { createInterface } from 'node:readline/promises';

export interface Asker {
  ask(question: string, fallback: string): Promise<string>;
  confirm(question: string): Promise<boolean>;
  close(): void;
}

class Interactive implements Asker {
  private readonly rl = createInterface({ input: process.stdin, output: process.stdout });

  async ask(question: string, fallback: string): Promise<string> {
    const answer = (await this.rl.question(`${question}${fallback ? ` (${fallback})` : ''}: `)).trim();
    return answer || fallback;
  }

  async confirm(question: string): Promise<boolean> {
    return /^y(es)?$/i.test((await this.rl.question(`${question} (y/N): `)).trim());
  }

  close(): void {
    this.rl.close();
  }
}

class Silent implements Asker {
  async ask(_question: string, fallback: string): Promise<string> {
    return fallback;
  }

  async confirm(): Promise<boolean> {
    return false;
  }

  close(): void {}
}

export function asker(interactive: boolean): Asker {
  return interactive && process.stdin.isTTY ? new Interactive() : new Silent();
}

export function slugify(value: string): string {
  return (
    value
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '') || 'app'
  );
}

export function isSlug(value: string): boolean {
  return /^[a-z0-9][a-z0-9-]*$/.test(value);
}

import { Database, type SQLQueryBindings } from 'bun:sqlite';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const MIGRATIONS = join(import.meta.dir, '..', 'migrations');

export type FakeD1 = D1Database & { sqlite: Database; statements: string[] };

function migrated(): Database {
  const sqlite = new Database(':memory:', { strict: false });
  sqlite.exec('PRAGMA foreign_keys = ON');
  for (const file of readdirSync(MIGRATIONS)
    .filter(name => name.endsWith('.sql'))
    .sort()) {
    sqlite.exec(readFileSync(join(MIGRATIONS, file), 'utf8'));
  }
  return sqlite;
}

class FakeStatement {
  constructor(
    private readonly db: FakeD1,
    readonly sql: string,
    readonly params: SQLQueryBindings[] = [],
  ) {}

  bind(...values: unknown[]): FakeStatement {
    for (const value of values) {
      if (value === undefined) throw new Error('D1_TYPE_ERROR: undefined is not a supported bind value');
    }
    return new FakeStatement(this.db, this.sql, values as SQLQueryBindings[]);
  }

  rows(): Record<string, unknown>[] {
    this.db.statements.push(this.sql);
    return this.db.sqlite.query(this.sql).all(...this.params) as Record<string, unknown>[];
  }

  async first<T>(column?: string): Promise<T | null> {
    const row = this.rows()[0];
    if (row === undefined) return null;
    return (column === undefined ? row : row[column]) as T;
  }

  async all<T>(): Promise<D1Result<T>> {
    return { results: this.rows() as T[], success: true, meta: {} } as D1Result<T>;
  }

  async run<T>(): Promise<D1Result<T>> {
    return this.all<T>();
  }

  async raw<T>(): Promise<T[]> {
    return this.rows().map(row => Object.values(row)) as T[];
  }
}

export function fakeD1(): FakeD1 {
  const db = { sqlite: migrated(), statements: [] as string[] } as unknown as FakeD1;

  Object.assign(db, {
    prepare(sql: string) {
      return new FakeStatement(db, sql);
    },
    async batch(statements: FakeStatement[]) {
      return db.sqlite.transaction(() => statements.map(statement => ({ results: statement.rows(), success: true })))();
    },
    async exec(sql: string) {
      db.sqlite.exec(sql);
      return { count: 1, duration: 0 };
    },
  });

  return db;
}

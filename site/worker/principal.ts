import type { JudgeRecord, Principal } from '../src/lib/principal.ts';
import { kvOf, type Env } from './env.ts';
import { readRecord, walkEntries, type KvLike } from './store.ts';

export type { JudgeRecord, Principal };

export const JUDGE_PREFIX = 'judge:';
export const HANDLE_MAX_LEN = 40;
export const TOKEN_BYTES = 32;

const HANDLE_SHAPE = /^[A-Za-z0-9._-]+$/;

export function principalLabel(caller: Principal): string {
  return caller.role === 'admin' ? 'admin' : caller.handle;
}

export function scoringHandle(caller: Principal): string | null {
  return caller.role === 'judge' ? caller.handle : null;
}

export function tokenMatches(provided: string, expected: string): boolean {
  if (!expected) return false;
  const a = new TextEncoder().encode(provided);
  const b = new TextEncoder().encode(expected);
  let diff = a.byteLength ^ b.byteLength;
  for (let i = 0; i < Math.max(a.byteLength, b.byteLength); i += 1) {
    diff |= (a[i] ?? 0) ^ (b[i] ?? 0);
  }
  return diff === 0;
}

function hex(bytes: Uint8Array): string {
  return [...bytes].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

export async function hashToken(token: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(token));
  return hex(new Uint8Array(digest));
}

export function mintToken(): string {
  const bytes = new Uint8Array(TOKEN_BYTES);
  crypto.getRandomValues(bytes);
  return hex(bytes);
}

export function judgeKeyFor(hash: string): string {
  return `${JUDGE_PREFIX}${hash}`;
}

export function bearerToken(request: Request): string | null {
  const header = request.headers.get('authorization') ?? '';
  const prefix = 'Bearer ';
  if (!header.startsWith(prefix)) return null;
  return header.slice(prefix.length).trim() || null;
}

export function normalizeHandle(raw: string): string | null {
  const trimmed = raw.trim().replace(/^@+/, '');
  if (!trimmed || trimmed.length > HANDLE_MAX_LEN) return null;
  return HANDLE_SHAPE.test(trimmed) ? trimmed : null;
}

export async function principal(request: Request, env: Env): Promise<Principal | null> {
  const token = bearerToken(request);
  if (token === null) return null;
  if (tokenMatches(token, env.ADMIN_TOKEN ?? '')) return { role: 'admin' };

  const judge = await readRecord<JudgeRecord>(kvOf(env), judgeKeyFor(await hashToken(token)));
  return judge === null ? null : { role: 'judge', handle: judge.handle };
}

async function judgeEntries(kv: KvLike): Promise<{ key: string; record: JudgeRecord }[]> {
  return walkEntries<JudgeRecord>(kv, JUDGE_PREFIX);
}

export async function listJudges(kv: KvLike): Promise<JudgeRecord[]> {
  return (await judgeEntries(kv))
    .map(entry => entry.record)
    .sort((a, b) => a.handle.localeCompare(b.handle) || a.created_at.localeCompare(b.created_at));
}

export type CreateJudgeOutcome =
  | { ok: true; token: string; judge: JudgeRecord }
  | { ok: false; status: number; reason: string };

export async function createJudge(args: { kv: KvLike; rawHandle: unknown; now: string }): Promise<CreateJudgeOutcome> {
  const { kv, rawHandle, now } = args;

  if (typeof rawHandle !== 'string')
    return { ok: false, status: 400, reason: 'send a json body with a "handle" string' };
  const handle = normalizeHandle(rawHandle);
  if (handle === null) {
    return {
      ok: false,
      status: 400,
      reason: `"handle" must be 1 to ${HANDLE_MAX_LEN} characters of letters, digits, dot, dash, or underscore`,
    };
  }

  const held = await listJudges(kv);
  if (held.some(judge => judge.handle.toLowerCase() === handle.toLowerCase())) {
    return { ok: false, status: 409, reason: `${handle} is already a judge` };
  }

  const token = mintToken();
  const judge: JudgeRecord = { handle, created_at: now };
  await kv.put(judgeKeyFor(await hashToken(token)), JSON.stringify(judge));

  return { ok: true, token, judge };
}

export type RevokeJudgeOutcome = { ok: true; handle: string } | { ok: false; status: number; reason: string };

export async function revokeJudge(args: { kv: KvLike; rawHandle: unknown }): Promise<RevokeJudgeOutcome> {
  const { kv, rawHandle } = args;

  if (typeof rawHandle !== 'string')
    return { ok: false, status: 400, reason: 'send a json body with a "handle" string' };
  const handle = normalizeHandle(rawHandle);
  if (handle === null) return { ok: false, status: 400, reason: '"handle" is not a judge handle' };

  const found = (await judgeEntries(kv)).find(entry => entry.record.handle.toLowerCase() === handle.toLowerCase());
  if (!found) return { ok: false, status: 404, reason: `${handle} is not a judge` };

  await kv.delete(found.key);
  return { ok: true, handle: found.record.handle };
}

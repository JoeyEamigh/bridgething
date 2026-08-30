import type { Memory } from './memory.ts';

export type Stats = {
  type: 'stats';
  host: { name: string; os: string; arch: string; release: string };
  cores: number;
  load: [number, number, number];
  memory: Memory;
  uptimeSeconds: number;
  addresses: { name: string; address: string }[];
  at: number;
};

function isNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

export function parseStats(message: unknown): Stats | null {
  if (!message || typeof message !== 'object') return null;
  const candidate = message as Record<string, unknown>;
  if (candidate.type !== 'stats') return null;
  const host = candidate.host as Record<string, unknown> | undefined;
  const memory = candidate.memory as Record<string, unknown> | undefined;
  const load = candidate.load;
  if (!host || typeof host.name !== 'string') return null;
  if (!memory || !isNumber(memory.total) || !isNumber(memory.used)) return null;
  if (!Array.isArray(load) || load.length !== 3 || !load.every(isNumber)) return null;
  if (!isNumber(candidate.cores) || !isNumber(candidate.uptimeSeconds)) return null;
  return candidate as unknown as Stats;
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

export function formatBytes(bytes: number): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

export function formatUptime(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const days = Math.floor(whole / 86_400);
  const hours = Math.floor((whole % 86_400) / 3_600);
  const minutes = Math.floor((whole % 3_600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, Math.round(value)));
}

export function loadPercent(load: number, cores: number): number {
  if (cores <= 0) return 0;
  return clampPercent((load / cores) * 100);
}

export function percentOf(part: number, whole: number): number {
  if (whole <= 0) return 0;
  return clampPercent((part / whole) * 100);
}

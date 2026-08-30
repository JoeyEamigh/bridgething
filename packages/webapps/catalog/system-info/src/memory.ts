export type Memory = {
  total: number;
  used: number;
  cached: number | null;
  swapTotal: number;
  swapFree: number;
};

export type PhysicalMemory = Pick<Memory, 'total' | 'used' | 'cached'>;

function pages(text: string, label: string): number | null {
  const match = text.match(new RegExp(`^${label}:\\s+(\\d+)\\.?$`, 'm'));
  return match ? Number(match[1]) : null;
}

export function memoryFromVmStat(text: string, total: number): PhysicalMemory | null {
  const pageSize = Number(text.match(/page size of (\d+) bytes/)?.[1]);
  const active = pages(text, 'Pages active');
  const wired = pages(text, 'Pages wired down');
  const fileBacked = pages(text, 'File-backed pages');
  if (!pageSize || active === null || wired === null || fileBacked === null) return null;
  return { total, used: (active + wired) * pageSize, cached: fileBacked * pageSize };
}

function kib(text: string, field: string): number | null {
  const match = text.match(new RegExp(`^${field}:\\s+(\\d+) kB$`, 'm'));
  return match ? Number(match[1]) : null;
}

export function memoryFromMeminfo(text: string): PhysicalMemory | null {
  const total = kib(text, 'MemTotal');
  const available = kib(text, 'MemAvailable');
  const cached = kib(text, 'Cached');
  const buffers = kib(text, 'Buffers');
  if (total === null || available === null) return null;
  return {
    total: total * 1024,
    used: (total - available) * 1024,
    cached: cached === null || buffers === null ? null : (cached + buffers) * 1024,
  };
}

import type { TimeInfo } from '@bridgething/client';

type Clock = Pick<TimeInfo, 'wallClockUnixS' | 'tzIana' | 'locale'>;

export function clockText(time: Clock | null, receivedAt: number, now: number): string {
  if (!time?.wallClockUnixS) return 'n/a';
  const d = new Date(time.wallClockUnixS * 1000 + (now - receivedAt));
  try {
    return d.toLocaleString(time.locale ?? undefined, {
      timeZone: time.tzIana ?? undefined,
      hour: '2-digit',
      minute: '2-digit',
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    });
  } catch {
    return d.toISOString();
  }
}

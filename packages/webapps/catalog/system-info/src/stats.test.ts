import { describe, expect, test } from 'bun:test';
import { formatBytes, formatUptime, loadPercent, parseStats, percentOf, type Stats } from './stats.ts';

const SAMPLE: Stats = {
  type: 'stats',
  host: { name: 'astra', os: 'darwin', arch: 'aarch64', release: '25.5.0' },
  cores: 8,
  load: [2.5, 1.2, 0.8],
  memory: { total: 16 * 1024 ** 3, used: 12 * 1024 ** 3, cached: 2 * 1024 ** 3, swapTotal: 0, swapFree: 0 },
  uptimeSeconds: 90_061,
  addresses: [{ name: 'en0', address: '192.168.1.20' }],
  at: 1_700_000_000_000,
};

describe('parseStats', () => {
  test('accepts what the extension sends', () => {
    expect(parseStats(SAMPLE)).toEqual(SAMPLE);
  });

  test('rejects other forwards and malformed samples', () => {
    expect(parseStats({ type: 'refresh' })).toBeNull();
    expect(parseStats('stats')).toBeNull();
    expect(parseStats({ ...SAMPLE, load: [1, 2] })).toBeNull();
    expect(parseStats({ ...SAMPLE, memory: { total: 'lots' } })).toBeNull();
    expect(parseStats({ ...SAMPLE, host: undefined })).toBeNull();
  });
});

describe('formatting', () => {
  test('bytes pick a unit and drop decimals once three digits show', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(120 * 1024 ** 2)).toBe('120 MB');
    expect(formatBytes(15.9 * 1024 ** 3)).toBe('15.9 GB');
  });

  test('uptime shows the two largest units that matter', () => {
    expect(formatUptime(90_061)).toBe('1d 1h');
    expect(formatUptime(3_720)).toBe('1h 2m');
    expect(formatUptime(59)).toBe('0m');
  });

  test('percentages are clamped and survive a zero denominator', () => {
    expect(loadPercent(2.5, 8)).toBe(31);
    expect(loadPercent(20, 8)).toBe(100);
    expect(loadPercent(1, 0)).toBe(0);
    expect(percentOf(12, 16)).toBe(75);
    expect(percentOf(0, 0)).toBe(0);
  });
});

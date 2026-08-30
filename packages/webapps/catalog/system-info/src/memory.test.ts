import { describe, expect, test } from 'bun:test';
import { memoryFromMeminfo, memoryFromVmStat } from './memory.ts';

const VM_STAT = `Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                                   450375.
Pages active:                                1294851.
Pages inactive:                              1257690.
Pages speculative:                             34668.
Pages throttled:                                   0.
Pages wired down:                             317956.
Pages purgeable:                               12321.
"Translation faults":                     8707305236.
Pages copy-on-write:                       308117595.
Pages zero filled:                        6971407642.
Pages reactivated:                         508665909.
Pages purged:                              102190094.
File-backed pages:                            559798.
Anonymous pages:                             2119778.
Pages stored in compressor:                  4370404.
Pages occupied by compressor:                 760456.
Decompressions:                            548540140.
Compressions:                              903939826.
Pageins:                                   177458011.
Pageouts:                                    1462069.
Swapins:                                    64060640.
Swapouts:                                   76083793.
`;

const MEMINFO = `MemTotal:        7999992 kB
MemFree:          401664 kB
MemAvailable:    5322512 kB
Buffers:          210020 kB
Cached:          4593716 kB
SwapCached:            0 kB
Active:          3000000 kB
SwapTotal:       2097148 kB
SwapFree:        2097148 kB
`;

describe('memoryFromVmStat', () => {
  test('counts active and wired pages as used and file-backed pages as cache, the way btop does', () => {
    const page = 16384;
    expect(memoryFromVmStat(VM_STAT, 64 * 1024 ** 3)).toEqual({
      total: 64 * 1024 ** 3,
      used: (1294851 + 317956) * page,
      cached: 559798 * page,
    });
  });

  test('gives up on output missing a line rather than guessing', () => {
    expect(memoryFromVmStat(VM_STAT.replace(/^Pages active:.*\n/m, ''), 1)).toBeNull();
    expect(memoryFromVmStat('nothing here', 1)).toBeNull();
  });
});

describe('memoryFromMeminfo', () => {
  test('used is total minus available, cache is page cache plus buffers', () => {
    expect(memoryFromMeminfo(MEMINFO)).toEqual({
      total: 7999992 * 1024,
      used: (7999992 - 5322512) * 1024,
      cached: (4593716 + 210020) * 1024,
    });
  });

  test('a kernel without MemAvailable is not guessed at', () => {
    expect(memoryFromMeminfo(MEMINFO.replace(/^MemAvailable:.*\n/m, ''))).toBeNull();
  });
});

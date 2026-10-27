import { otaPhaseLabel, otaRunDetail } from '../lib/ota';
import { otaRun } from './fixtures';

describe('ota run label', () => {
  test('a phase reads as one label, never a phase glued to a step name', () => {
    const run = otaRun({ phase: 'downloading', stepId: 4 });

    expect(otaPhaseLabel(run)).toBe('downloading');
  });

  test('a store install says installing where firmware says writing', () => {
    expect(otaPhaseLabel(otaRun({ phase: 'writing' }))).toBe('writing');
    expect(
      otaPhaseLabel(
        otaRun({
          phase: 'writing',
          lane: 'webapp',
          otaKind: 'installedWebapp',
        }),
      ),
    ).toBe('installing');
  });

  test('an image reboots and a daemon restarts', () => {
    expect(otaPhaseLabel(otaRun({ phase: 'reboot' }))).toBe('rebooting');
    expect(otaPhaseLabel(otaRun({ phase: 'reboot', otaKind: 'daemon' }))).toBe(
      'restarting',
    );
  });
});

describe('ota run detail', () => {
  test('a transfer shows its bytes, rate and time left', () => {
    const run = otaRun({
      phase: 'streaming',
      stageReceived: 1024 * 1024,
      stageTotal: 4 * 1024 * 1024,
      ratePerSec: 150 * 1024,
    });

    expect(otaRunDetail(run, 20)).toBe('1.0 MB / 4.0 MB · 150 KB/s · 20s left');
  });

  test('the write has no bytes to show, so it shows none', () => {
    const run = otaRun({
      phase: 'writing',
      stageReceived: 0,
      stageTotal: 0,
      writePercent: 40,
    });

    expect(otaRunDetail(run, 300)).toBe('5m left');
  });

  test('nothing to say is no line at all', () => {
    const run = otaRun({
      phase: 'writing',
      stageReceived: undefined,
      stageTotal: undefined,
    });

    expect(otaRunDetail(run, 0)).toBeNull();
    expect(otaRunDetail(run, undefined)).toBeNull();
  });
});

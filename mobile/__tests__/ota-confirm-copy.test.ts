import type {
  BridgethingOtaAvailable,
  BridgethingOtaRelease,
} from '@bridgething/session-react-native';

import { describeOtaInstall, describeOtaOffer } from '../lib/ota';

const BUILD_VOCAB =
  /\b(daemon|image|channel|wire|adapter|libbridgething|webapp|lib)\b/i;

function release(
  over: Partial<BridgethingOtaRelease> = {},
): BridgethingOtaRelease {
  return {
    version: '0.7.0+image.2026.7.1',
    daemonVersion: '0.7.0',
    imageVersion: '2026.7.1',
    yanked: false,
    deprecated: false,
    ...over,
  };
}

function available(
  over: Partial<BridgethingOtaAvailable> = {},
): BridgethingOtaAvailable {
  return { deviceId: 'aa:bb:cc:dd:ee:ff', ...over };
}

function authored(text: string, ...values: (string | undefined)[]): string {
  return values.reduce<string>(
    (acc, value) => (value ? acc.split(value).join(' ') : acc),
    text,
  );
}

describe('the firmware install question', () => {
  test('names the release and states the consequence in plain language', () => {
    const r = release();
    const copy = describeOtaInstall(r, 'stable', 'stable');

    expect(copy.title).toContain(r.version);
    expect(authored(copy.title, r.version)).not.toMatch(BUILD_VOCAB);
    expect(copy.body).not.toMatch(BUILD_VOCAB);
    expect(copy.body).toMatch(/restart/);
  });

  test('the build pair lives on a labelled detail line, not in the question', () => {
    const r = release();
    const copy = describeOtaInstall(r, 'stable', 'stable');

    expect(copy.detail).toContain(r.daemonVersion);
    expect(copy.detail).toContain(r.imageVersion);
    expect(copy.detail.startsWith('detail:')).toBe(true);
    expect(authored(copy.body, r.version)).not.toContain(r.daemonVersion);
  });

  test('staying on the same releases carries no warning', () => {
    expect(
      describeOtaInstall(release(), 'stable', 'stable').warning,
    ).toBeNull();
  });

  test('a device whose build is unreadable is not treated as a crossing', () => {
    expect(
      describeOtaInstall(release(), 'stable', undefined).warning,
    ).toBeNull();
  });

  test('crossing to another track warns in plain language', () => {
    const r = release({ version: '1.4.2' });
    const copy = describeOtaInstall(r, 'beta', 'stable');

    expect(copy.warning).toBe(
      '1.4.2 is a beta release. your car thing is on stable.',
    );
    expect(authored(copy.warning ?? '', '1.4.2', 'beta', 'stable')).not.toMatch(
      BUILD_VOCAB,
    );
  });

  test('crossing back the other way is detected too', () => {
    expect(describeOtaInstall(release(), 'stable', 'beta').warning).toContain(
      'your car thing is on beta',
    );
  });
});

describe('the update check result', () => {
  const NOW = 1_700_000_000_000;

  test('nothing on offer reads as a stated result, not silence', () => {
    expect(describeOtaOffer({ lastCheckedAt: null, now: NOW })).toMatchObject({
      version: null,
      value: 'up to date',
      detail: null,
    });
  });

  test('an offer names the version it would install', () => {
    const offer = describeOtaOffer({
      available: available({ releaseVersion: '1.4.2' }),
      lastCheckedAt: null,
      now: NOW,
    });

    expect(offer).toMatchObject({ version: '1.4.2', value: '1.4.2 available' });
  });

  test('an offer with no release name still reads as an offer', () => {
    const offer = describeOtaOffer({
      available: available({ daemonVersion: '0.8.0' }),
      lastCheckedAt: null,
      now: NOW,
    });

    expect(offer).toMatchObject({ version: null, value: 'update available' });
  });

  test('a check that just ran says when it ran', () => {
    expect(
      describeOtaOffer({ lastCheckedAt: NOW - 5 * 60_000, now: NOW }).detail,
    ).toBe('checked 5m ago');
  });

  test('a failed check reports the reason instead of a timestamp', () => {
    expect(
      describeOtaOffer({
        lastCheckedAt: NOW - 5 * 60_000,
        error: { kind: 'networkUnreachable' },
        now: NOW,
      }).detail,
    ).toBe('network unreachable');
  });

  test('a failed check with nothing on offer does not claim the device is current', () => {
    expect(
      describeOtaOffer({
        lastCheckedAt: null,
        error: { kind: 'networkUnreachable' },
        now: NOW,
      }).value,
    ).toBe('check failed');
  });

  test('a stale offer still reads as an offer when a later check fails', () => {
    expect(
      describeOtaOffer({
        available: available({ releaseVersion: '1.4.2' }),
        lastCheckedAt: null,
        error: { kind: 'networkUnreachable' },
        now: NOW,
      }).value,
    ).toBe('1.4.2 available');
  });
});

import { describe, expect, test } from 'bun:test';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parse } from 'yaml';

const SCRIPT = resolve(import.meta.dirname, 'apply-webapp-publish.ts');
const ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const SHA = 'a'.repeat(64);

type StateRow = { slug: string; versions: Record<string, unknown>[]; [k: string]: unknown };

function published(overrides: Record<string, unknown> = {}) {
  return {
    slug: 'calendar',
    id: ID,
    name: 'Calendar',
    description: 'Upcoming events.',
    version: '0.2.0',
    permissions: ['net.fetch'],
    icon: `https://apps.bridgething.com/icons/${ID}.svg`,
    download: { url: `https://apps.bridgething.com/r/${ID}/0.2.0.zip`, size: 100, sha256: SHA },
    ...overrides,
  };
}

async function run(state: string, apps: Record<string, unknown>[]) {
  const dir = await mkdtemp(join(tmpdir(), 'btapps-'));
  const statePath = join(dir, 'apps-published.yaml');
  const payloadPath = join(dir, 'payload.json');
  await writeFile(statePath, state);
  await writeFile(payloadPath, JSON.stringify({ apps }));

  const proc = Bun.spawn(
    [
      'bun',
      'run',
      SCRIPT,
      '--payload',
      payloadPath,
      '--state-path',
      statePath,
      '--released-at',
      '2026-08-01T00:00:00Z',
    ],
    { stdout: 'pipe', stderr: 'pipe' },
  );
  const [code, stderr] = await Promise.all([proc.exited, new Response(proc.stderr).text()]);
  const doc = parse(await readFile(statePath, 'utf-8')) as { apps: StateRow[] };
  return { code, stderr, doc };
}

const EXISTING = `recommended_sources: []
apps:
  - slug: calendar
    id: ${ID}
    name: Calendar
    description: Upcoming events.
    icon: https://apps.bridgething.com/icons/${ID}.svg
    versions:
      - version: 0.1.0
        released_at: 2026-05-31T00:00:00Z
        download:
          url: https://apps.bridgething.com/r/${ID}/0.1.0.zip
          size: 90
          sha256: ${'b'.repeat(64)}
        permissions:
          - net.fetch
        min_libbridgething_version: 0.4.0
        changelog: Initial release.
`;

describe('apply-webapp-publish', () => {
  test('seeds a new app with publish-owned fields only', async () => {
    const { code, doc } = await run('recommended_sources: []\napps: []\n', [published()]);

    expect(code).toBe(0);
    expect(Object.keys(doc.apps[0]!).sort()).toEqual(['description', 'icon', 'id', 'name', 'slug', 'versions']);
  });

  test('leaves curation alone by never carrying attribution', async () => {
    const { doc } = await run('recommended_sources: []\napps: []\n', [published()]);

    expect(doc.apps[0]!['author']).toBeUndefined();
    expect(doc.apps[0]!['homepage']).toBeUndefined();
    expect(doc.apps[0]!['source']).toBeUndefined();
  });

  test('appends a version newest-first alongside the published history', async () => {
    const { doc } = await run(EXISTING, [published()]);

    expect(doc.apps[0]!.versions.map(v => v['version'])).toEqual(['0.2.0', '0.1.0']);
    expect(doc.apps[0]!.versions[1]!['changelog']).toBe('Initial release.');
  });

  test('rewrites the artifact metadata of a version already published', async () => {
    const { doc } = await run(EXISTING, [
      published({
        version: '0.1.0',
        download: { url: `https://apps.bridgething.com/r/${ID}/0.1.0.zip`, size: 111, sha256: SHA },
      }),
    ]);

    expect(doc.apps[0]!.versions).toHaveLength(1);
    expect(doc.apps[0]!.versions[0]!['download']).toEqual({
      url: `https://apps.bridgething.com/r/${ID}/0.1.0.zip`,
      size: 111,
      sha256: SHA,
    });
    expect(doc.apps[0]!.versions[0]!['changelog']).toBe('Initial release.');
  });

  test('folds a manifest extension block into the catalog shape', async () => {
    const { doc } = await run('apps: []\n', [
      published({ extension: { entry: 'extension/desktop.mjs', permissions: ['all', 'run:osascript'], api: 1 } }),
    ]);

    expect(doc.apps[0]!.versions[0]!['extension']).toEqual({ desktop: true, permissions: ['all', 'run:osascript'] });
  });

  test('drops the extension block when a rebuilt version no longer ships one', async () => {
    const state = [
      'apps:',
      '  - slug: calendar',
      `    id: ${ID}`,
      '    name: Calendar',
      '    description: Upcoming events.',
      '    icon: null',
      '    versions:',
      '      - version: 0.2.0',
      '        released_at: 2026-07-01T00:00:00Z',
      '        download:',
      `          url: https://apps.bridgething.com/r/${ID}/0.2.0.zip`,
      '          size: 100',
      `          sha256: ${SHA}`,
      '        permissions: []',
      '        extension:',
      '          desktop: true',
      '          permissions:',
      '            - all',
      '        min_libbridgething_version: 0.4.0',
      '        changelog: null',
      '',
    ].join('\n');

    const { doc } = await run(state, [published()]);

    expect(doc.apps[0]!.versions[0]!['extension']).toBeUndefined();
  });

  test('carries the slot fields the launcher and overlay roles need', async () => {
    const { doc } = await run('apps: []\n', [published({ role: 'launcher', provides_overlay: true })]);

    expect(doc.apps[0]!.versions[0]!['role']).toBe('launcher');
    expect(doc.apps[0]!.versions[0]!['provides_overlay']).toBe(true);
  });

  test('keeps recommended_sources, which the directory sync owns', async () => {
    const dir = await mkdtemp(join(tmpdir(), 'btapps-'));
    const statePath = join(dir, 'apps-published.yaml');
    await writeFile(
      statePath,
      'recommended_sources:\n  - name: vouched\n    url: https://a.example/c.json\n    description: null\n    attested: true\napps: []\n',
    );
    const payloadPath = join(dir, 'payload.json');
    await writeFile(payloadPath, JSON.stringify({ apps: [published()] }));

    const proc = Bun.spawn(['bun', 'run', SCRIPT, '--payload', payloadPath, '--state-path', statePath]);
    expect(await proc.exited).toBe(0);

    const doc = parse(await readFile(statePath, 'utf-8')) as { recommended_sources: unknown[] };
    expect(doc.recommended_sources).toHaveLength(1);
  });

  test('refuses a payload entry missing a publish-owned field', async () => {
    const { code, stderr } = await run('apps: []\n', [published({ id: '' })]);

    expect(code).not.toBe(0);
    expect(stderr).toContain('id');
  });
});

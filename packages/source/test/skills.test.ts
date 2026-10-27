import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { installedStamp, skills, toolkitDrift, type Stamp } from '../src/skills.ts';

const packageDir = resolve(import.meta.dir, '..');
const scaffold = resolve(packageDir, '..', 'create-bridgething');
const running = (JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8')) as { version: string }).version;

let root: string;

function writeScripts(scripts: Record<string, string>): void {
  writeFileSync(join(root, 'package.json'), JSON.stringify({ name: 'test-source', scripts }));
}

function stamp(toolkit?: Stamp['toolkit']): Stamp {
  return { package: 'create-bridgething', version: '0.14.0', fetched_at: '2026-10-27T00:00:00Z', toolkit };
}

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'bridgething-source-skills-'));
  mkdirSync(join(root, 'apps'));
  writeFileSync(join(root, 'source.json'), JSON.stringify({ name: 'test', description: 'test' }));
  process.env.BRIDGETHING_SOURCE_ROOT = root;
});

afterAll(() => {
  rmSync(root, { recursive: true, force: true });
});

describe('toolkitDrift', () => {
  test('a stamp that records no toolkit expects nothing', async () => {
    writeScripts({});
    expect(await toolkitDrift(stamp(), root)).toEqual([]);
  });

  test('a source whose toolkit and scripts match the skill has nothing to report', async () => {
    writeScripts({ submit: 'bridgething-source submit', check: 'bridgething-source check' });
    const drift = await toolkitDrift(
      stamp({ version: running, scripts: { submit: 'bridgething-source submit', check: 'bridgething-source check' } }),
      root,
    );
    expect(drift).toEqual([]);
  });

  test('a toolkit older than the skill expects is told to upgrade', async () => {
    writeScripts({ submit: 'bridgething-source submit' });
    const drift = await toolkitDrift(stamp({ version: '999.0.0', scripts: {} }), root);
    expect(drift).toEqual([
      `the skill expects @bridgething/source 999.0.0 and this source has ${running}. ` +
        'run "bun add -d @bridgething/source@latest"',
    ]);
  });

  test('a script the skill uses but package.json lacks is named with the line to add', async () => {
    writeScripts({ check: 'bun run something-else' });
    const drift = await toolkitDrift(
      stamp({ version: running, scripts: { submit: 'bridgething-source submit', check: 'bridgething-source check' } }),
      root,
    );
    expect(drift).toEqual([
      'package.json is missing scripts the skill uses. add them to "scripts":\n  "submit": "bridgething-source submit"',
    ]);
  });
});

describe('skills --from', () => {
  test('records the toolkit the vendored skill was written against', async () => {
    writeScripts({});
    await skills(['--from', scaffold]);

    const pins = JSON.parse(readFileSync(join(scaffold, 'template-versions.json'), 'utf8')) as Record<string, string>;
    const recorded = (await installedStamp(root))?.toolkit;
    expect(recorded?.version).toBe(pins['@bridgething/source']?.replace(/^\^/, ''));
    expect(recorded?.scripts['submit']).toBe('bridgething-source submit');
    expect(Object.values(recorded?.scripts ?? {}).every(command => command.startsWith('bridgething-source '))).toBe(
      true,
    );
  });
});

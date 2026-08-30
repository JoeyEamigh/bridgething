import { describe, expect, test } from 'bun:test';
import {
  declaresExtension,
  describeExtensionPermission,
  describeExtensionPermissions,
  extensionOf,
  extensionRepoLabel,
  isExtensionPermission,
} from '../src/extension.ts';
import type { AppEntry, AppVersion } from '../src/types.ts';

function version(overrides: Partial<AppVersion> = {}): AppVersion {
  return {
    version: '0.1.0',
    released_at: '2026-08-01T00:00:00Z',
    download: { url: 'https://apps.bridgething.com/r/x.zip', size: 1, sha256: '0'.repeat(64) },
    permissions: [],
    min_libbridgething_version: '0.5.0',
    changelog: null,
    ...overrides,
  };
}

function app(versions: AppVersion[]): AppEntry {
  return {
    id: '019e6701-13f8-71b5-ba04-85d326630e98',
    name: 'Discord Presence',
    description: 'Shows what you are listening to in Discord.',
    author: 'JoeyEamigh',
    icon: null,
    homepage: null,
    source: 'https://github.com/JoeyEamigh/bridgething-discord',
    versions,
  };
}

describe('isExtensionPermission()', () => {
  test('accepts the bare verbs', () => {
    for (const p of ['all', 'net', 'read', 'write', 'run', 'env', 'sys', 'ffi']) {
      expect(isExtensionPermission(p)).toBe(true);
    }
  });

  test('accepts scoped forms, including paths with spaces', () => {
    for (const p of ['net:discord.com', 'net:127.0.0.1:6463', 'read:~/Library/Application Support', 'env:HOME']) {
      expect(isExtensionPermission(p)).toBe(true);
    }
  });

  test('rejects an unknown verb, a scoped all, an empty scope, and a comma', () => {
    for (const p of ['hid', 'all:everything', 'net:', 'net:a.com,b.com', '', 'NET']) {
      expect(isExtensionPermission(p)).toBe(false);
    }
  });
});

describe('describeExtensionPermission()', () => {
  test('renders all as full host access', () => {
    expect(describeExtensionPermission('all')).toBe('full host access');
  });

  test('renders bare verbs as their widest meaning', () => {
    expect(describeExtensionPermission('net')).toBe('reach any network host');
    expect(describeExtensionPermission('read')).toBe('read any file');
    expect(describeExtensionPermission('write')).toBe('write any file');
    expect(describeExtensionPermission('run')).toBe('run any program');
    expect(describeExtensionPermission('env')).toBe('read every environment variable');
    expect(describeExtensionPermission('sys')).toBe('read system information');
    expect(describeExtensionPermission('ffi')).toBe('load any native library');
  });

  test('keeps the scope verbatim, host:port included', () => {
    expect(describeExtensionPermission('net:127.0.0.1:6463')).toBe('reach 127.0.0.1:6463');
    expect(describeExtensionPermission('read:~/Music')).toBe('read ~/Music');
    expect(describeExtensionPermission('run:osascript')).toBe('run osascript');
    expect(describeExtensionPermission('env:HOME')).toBe('read the HOME environment variable');
  });

  test('falls back to the raw descriptor it cannot read', () => {
    expect(describeExtensionPermission('hid:0x1234')).toBe('hid:0x1234');
    expect(describeExtensionPermission('all:everything')).toBe('all:everything');
  });
});

describe('describeExtensionPermissions()', () => {
  test('maps in declared order', () => {
    expect(describeExtensionPermissions({ desktop: true, permissions: ['run:deno', 'all'] })).toEqual([
      'run deno',
      'full host access',
    ]);
  });

  test('says so when an extension asks for nothing', () => {
    expect(describeExtensionPermissions({ desktop: true, permissions: [] })).toEqual([
      'nothing beyond talking to your Car Thing',
    ]);
  });
});

describe('extensionOf() and declaresExtension()', () => {
  test('extensionOf returns null for a plain version and for nothing at all', () => {
    expect(extensionOf(version())).toBeNull();
    expect(extensionOf(null)).toBeNull();
    expect(extensionOf(undefined)).toBeNull();
  });

  test('extensionOf returns the block when the version ships one', () => {
    const ext = { desktop: true as const, permissions: ['all'] };
    expect(extensionOf(version({ extension: ext }))).toBe(ext);
  });

  test('declaresExtension is true when any version ships one', () => {
    expect(
      declaresExtension(
        app([version({ version: '0.2.0' }), version({ extension: { desktop: true, permissions: [] } })]),
      ),
    ).toBe(true);
    expect(declaresExtension(app([version()]))).toBe(false);
  });
});

describe('extensionRepoLabel()', () => {
  test('shortens a repo url to owner/repo, trailing slash or not', () => {
    expect(extensionRepoLabel('https://github.com/JoeyEamigh/bridgething')).toBe('JoeyEamigh/bridgething');
    expect(extensionRepoLabel('https://github.com/JoeyEamigh/bridgething/')).toBe('JoeyEamigh/bridgething');
  });

  test('leaves anything else alone, and passes nothing through', () => {
    expect(extensionRepoLabel('https://example.com/code')).toBe('https://example.com/code');
    expect(extensionRepoLabel(null)).toBeNull();
    expect(extensionRepoLabel(undefined)).toBeNull();
  });
});

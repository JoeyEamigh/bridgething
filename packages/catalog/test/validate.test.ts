import { describe, expect, test } from 'bun:test';
import { declaresExtension, isExtensionPermission } from '../src/extension.ts';
import type { Catalog } from '../src/types.ts';
import { CatalogValidationError, validate, validateInvariants, validateSchema } from '../src/validate.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const WEATHER_ID = '019e6701-13f8-71b5-ba04-81f347137de2';
const SHA = '0'.repeat(64);

function version(v: string, releasedAt: string) {
  return {
    version: v,
    released_at: releasedAt,
    download: { url: 'https://apps.bridgething.com/r/x.zip', size: 1, sha256: SHA },
    permissions: ['net.fetch'],
    min_libbridgething_version: '0.5.0',
    changelog: null,
  };
}

function extensionApp(overrides: Partial<Catalog['apps'][number]> = {}): Catalog['apps'][number] {
  const v = version('0.1.0', '2026-05-31T00:00:00Z') as Catalog['apps'][number]['versions'][number];
  v.extension = { desktop: true, permissions: ['all'] };
  return {
    id: WEATHER_ID,
    name: 'Discord Presence',
    description: 'Shows what you are listening to in Discord.',
    author: 'JoeyEamigh',
    icon: null,
    homepage: null,
    source: 'https://github.com/JoeyEamigh/bridgething-discord',
    versions: [v],
    ...overrides,
  };
}

function fixture(): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: { name: 'bridgething apps', description: 'official', homepage: null, icon: null },
    apps: [
      {
        id: CALENDAR_ID,
        name: 'Calendar',
        description: 'Upcoming events.',
        author: 'JoeyEamigh',
        icon: null,
        homepage: null,
        source: null,
        versions: [version('0.2.0', '2026-05-31T00:00:00Z'), version('0.1.0', '2026-05-01T00:00:00Z')],
      },
    ],
    recommended_sources: [],
  };
}

describe('validateSchema()', () => {
  test('happy path passes', () => {
    expect(() => validateSchema(fixture())).not.toThrow();
  });

  test('rejects unknown top-level key', () => {
    const m = fixture() as Record<string, unknown>;
    m['extra'] = true;
    expect(() => validateSchema(m)).toThrow(CatalogValidationError);
  });

  test('tolerates a key on an app or a version that this client has never heard of', () => {
    const m = fixture();
    (m.apps[0] as unknown as Record<string, unknown>)['badge'] = 'staff pick';
    (m.apps[0]!.versions[0] as unknown as Record<string, unknown>)['runtime'] = { wasm: true };

    expect(() => validateSchema(m)).not.toThrow();
  });

  test('still checks every app and version key it does know', () => {
    const badRole = fixture();
    (badRole.apps[0]!.versions[0] as unknown as Record<string, unknown>)['role'] = 'overlord';
    expect(() => validateSchema(badRole)).toThrow(CatalogValidationError);

    const badIcon = fixture();
    (badIcon.apps[0] as unknown as Record<string, unknown>)['icon'] = 42;
    expect(() => validateSchema(badIcon)).toThrow(CatalogValidationError);

    const badExtension = fixture();
    (badExtension.apps[0]!.versions[0] as unknown as Record<string, unknown>)['extension'] = {
      desktop: true,
      permissions: ['all'],
      api: 1,
    };
    expect(() => validateSchema(badExtension)).toThrow(CatalogValidationError);
  });

  test('rejects a non-catalog schema discriminant', () => {
    const m = fixture() as unknown as { schema: string };
    m.schema = 'catalog.v2';
    expect(() => validateSchema(m)).toThrow(/schema validation/);
  });

  test('rejects a malformed sha256', () => {
    const m = fixture();
    m.apps[0]!.versions[0]!.download.sha256 = 'nope';
    expect(() => validateSchema(m)).toThrow(CatalogValidationError);
  });

  test('rejects an app with no versions', () => {
    const m = fixture();
    m.apps[0]!.versions = [];
    expect(() => validateSchema(m)).toThrow(CatalogValidationError);
  });
});

describe('validateInvariants()', () => {
  test('happy path passes', () => {
    expect(() => validateInvariants(fixture())).not.toThrow();
  });

  test('fails on duplicate app ids', () => {
    const m = fixture();
    m.apps.push({ ...m.apps[0]!, name: 'Calendar Clone' });
    expect(() => validateInvariants(m)).toThrow(/used by both/);
  });

  test('fails when an app id is not a uuidv7', () => {
    const m = fixture();
    m.apps[0]!.id = '00000000-0000-4000-8000-000000000000';
    expect(() => validateInvariants(m)).toThrow(/not a valid uuidv7/);
  });

  test('fails on a duplicate version within one app', () => {
    const m = fixture();
    m.apps[0]!.versions.push(version('0.1.0', '2026-04-01T00:00:00Z'));
    expect(() => validateInvariants(m)).toThrow(/more than once/);
  });

  test('fails when versions are not newest-first', () => {
    const m = fixture();
    m.apps[0]!.versions = [version('0.1.0', '2026-05-01T00:00:00Z'), version('0.2.0', '2026-05-31T00:00:00Z')];
    expect(() => validateInvariants(m)).toThrow(/not newest-first/);
  });
});

describe('extension versions', () => {
  test('schema accepts an extension block', () => {
    const m = fixture();
    m.apps.push(extensionApp());
    expect(() => validateSchema(m)).not.toThrow();
  });

  test('schema rejects an unknown key inside the extension block', () => {
    const m = fixture();
    const app = extensionApp();
    (app.versions[0]!.extension as unknown as Record<string, unknown>)['entry'] = 'extension/desktop.mjs';
    m.apps.push(app);
    expect(() => validateSchema(m)).toThrow(CatalogValidationError);
  });

  test('schema rejects desktop: false, since there is no other host', () => {
    const m = fixture();
    const app = extensionApp();
    (app.versions[0]!.extension as unknown as { desktop: boolean }).desktop = false;
    m.apps.push(app);
    expect(() => validateSchema(m)).toThrow(CatalogValidationError);
  });

  test('schema rejects an extension block with no permissions key', () => {
    const m = fixture();
    const app = extensionApp();
    delete (app.versions[0]!.extension as unknown as Record<string, unknown>)['permissions'];
    m.apps.push(app);
    expect(() => validateSchema(m)).toThrow(CatalogValidationError);
  });

  test('invariants accept every descriptor shape in the grammar', () => {
    const m = fixture();
    const app = extensionApp();
    app.versions[0]!.extension!.permissions = [
      'all',
      'net',
      'net:discord.com',
      'net:127.0.0.1:6463',
      'read',
      'read:~/Library/Application Support',
      'write',
      'write:/tmp/out',
      'run',
      'run:osascript',
      'env',
      'env:HOME',
      'sys',
      'sys:hostname',
      'ffi',
      'ffi:/usr/lib/libfoo.dylib',
    ];
    m.apps.push(app);
    expect(() => validateInvariants(m)).not.toThrow();
  });

  test('invariants reject a descriptor outside the grammar', () => {
    const m = fixture();
    const app = extensionApp();
    app.versions[0]!.extension!.permissions = ['hid'];
    m.apps.push(app);
    expect(() => validateInvariants(m)).toThrow(/not a permission descriptor/);
  });

  test('invariants reject a scoped form of all', () => {
    const m = fixture();
    const app = extensionApp();
    app.versions[0]!.extension!.permissions = ['all:everything'];
    m.apps.push(app);
    expect(() => validateInvariants(m)).toThrow(/not a permission descriptor/);
  });

  test('invariants require a github source when any version ships an extension', () => {
    const m = fixture();
    m.apps.push(extensionApp({ source: 'https://example.com/code' }));
    expect(() => validateInvariants(m)).toThrow(/must be a github\.com repo url/);
  });

  test('invariants reject a null source on an extension app', () => {
    const m = fixture();
    m.apps.push(extensionApp({ source: null }));
    expect(() => validateInvariants(m)).toThrow(/not null/);
  });

  test('invariants reject a github url deeper than owner\/repo', () => {
    const m = fixture();
    m.apps.push(extensionApp({ source: 'https://github.com/JoeyEamigh/bridgething/tree/main/apps' }));
    expect(() => validateInvariants(m)).toThrow(/must be a github\.com repo url/);
  });

  test('invariants accept a trailing slash on the repo url', () => {
    const m = fixture();
    m.apps.push(extensionApp({ source: 'https://github.com/JoeyEamigh/bridgething-discord/' }));
    expect(() => validateInvariants(m)).not.toThrow();
  });

  test('the source rule fires even when only an older version ships the extension', () => {
    const m = fixture();
    const app = extensionApp({ source: null });
    app.versions.unshift(version('0.2.0', '2026-06-01T00:00:00Z'));
    m.apps.push(app);
    expect(() => validateInvariants(m)).toThrow(/must be a github\.com repo url/);
  });

  test('an app with no extension keeps a null source', () => {
    expect(() => validateInvariants(fixture())).not.toThrow();
  });

  test('the descriptors the invariants accept are exactly the ones isExtensionPermission accepts', () => {
    const descriptors = [
      'all',
      'net',
      'net:discord.com',
      'net:127.0.0.1:6463',
      'read:~/Library/Application Support',
      'env:HOME',
      'ffi:/usr/lib/libfoo.dylib',
      'hid',
      'all:everything',
      'net:',
      'net:a.com,b.com',
      '',
      'NET',
    ];

    for (const permission of descriptors) {
      const m = fixture();
      const app = extensionApp();
      app.versions[0]!.extension!.permissions = [permission];
      m.apps.push(app);

      let accepted = true;
      try {
        validateInvariants(m);
      } catch {
        accepted = false;
      }

      expect(accepted).toBe(isExtensionPermission(permission));
    }
  });

  test('the source rule fires for exactly the apps declaresExtension flags', () => {
    const plain = fixture();
    expect(declaresExtension(plain.apps[0]!)).toBe(false);
    expect(() => validateInvariants(plain)).not.toThrow();

    const shipping = fixture();
    const app = extensionApp({ source: null });
    shipping.apps.push(app);
    expect(declaresExtension(app)).toBe(true);
    expect(() => validateInvariants(shipping)).toThrow(/github\.com repo url/);
  });
});

describe('validate()', () => {
  test('passes a multi-app catalog', () => {
    const m = fixture();
    m.apps.push({
      id: WEATHER_ID,
      name: 'Weather',
      description: 'Conditions and forecast.',
      author: 'JoeyEamigh',
      icon: null,
      homepage: null,
      source: null,
      versions: [version('0.1.0', '2026-05-31T00:00:00Z')],
    });
    expect(() => validate(m)).not.toThrow();
  });
});

describe('screenshots', () => {
  test('an app without the key still validates, so catalogs published before it keep working', () => {
    expect(() => validateSchema(fixture())).not.toThrow();
  });

  test('a list of https captures validates', () => {
    const doc = fixture();
    doc.apps[0]!.screenshots = ['https://apps.bridgething.com/shots/calendar-1.png'];
    expect(() => validateSchema(doc)).not.toThrow();
  });

  test('an empty array is refused, so a card cannot promise a picture it does not have', () => {
    const doc = fixture();
    doc.apps[0]!.screenshots = [];
    expect(() => validateSchema(doc)).toThrow(CatalogValidationError);
  });

  test('a non-url entry is refused', () => {
    const doc = fixture();
    doc.apps[0]!.screenshots = ['not a url'];
    expect(() => validateSchema(doc)).toThrow(CatalogValidationError);
  });
});

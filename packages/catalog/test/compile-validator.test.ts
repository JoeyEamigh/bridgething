import { describe, expect, test } from 'bun:test';
import { readFile } from 'node:fs/promises';
import { buildValidatorSources, VALIDATOR_PATH, VALIDATOR_TYPES_PATH } from '../scripts/compile-validator.ts';
import validateSchemaFn from '../src/generated/schema.v1.validator.mjs';

function catalogWith(extension: unknown) {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-08-28T00:00:00Z',
    repo: { name: 'r', description: 'd', homepage: null, icon: null },
    apps: [
      {
        id: '019e6701-13f8-71b5-ba04-85d326630e98',
        name: 'Discord Presence',
        description: 'd',
        author: 'JoeyEamigh',
        icon: null,
        homepage: null,
        source: 'https://github.com/JoeyEamigh/bridgething-discord',
        versions: [
          {
            version: '0.1.0',
            released_at: '2026-08-01T00:00:00Z',
            download: { url: 'https://apps.bridgething.com/r/x.zip', size: 1, sha256: '0'.repeat(64) },
            permissions: [],
            extension,
            min_libbridgething_version: '0.5.0',
            changelog: null,
          },
        ],
      },
    ],
    recommended_sources: [],
  };
}

describe('the committed validator', () => {
  test('matches what schema.v1.json compiles to', async () => {
    const [built, committed, committedTypes] = await Promise.all([
      buildValidatorSources(),
      readFile(VALIDATOR_PATH, 'utf-8'),
      readFile(VALIDATOR_TYPES_PATH, 'utf-8'),
    ]);

    expect(built.code).toBe(committed);
    expect(built.types).toBe(committedTypes);
  });

  test('imports nothing, since the workers runtime cannot resolve ajv helpers', async () => {
    const committed = await readFile(VALIDATOR_PATH, 'utf-8');
    expect(committed).not.toMatch(/^\s*import\s.+\sfrom\s/m);
  });
});

describe('the committed validator knows the extension block', () => {
  test('accepts a well-formed one', () => {
    expect(validateSchemaFn(catalogWith({ desktop: true, permissions: ['all'] }))).toBe(true);
  });

  test('rejects a stray key, a false desktop flag, and a duplicate permission', () => {
    expect(validateSchemaFn(catalogWith({ desktop: true, permissions: ['all'], api: 1 }))).toBe(false);
    expect(validateSchemaFn(catalogWith({ desktop: false, permissions: [] }))).toBe(false);
    expect(validateSchemaFn(catalogWith({ desktop: true, permissions: ['net', 'net'] }))).toBe(false);
  });
});

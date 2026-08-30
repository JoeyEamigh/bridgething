import { describe, expect, test } from 'bun:test';
import { validate } from '@bridgething/catalog';
import {
  keyFor,
  normalizeSourceUrl,
  SourceUrlError,
  toCatalogDocument,
  toDirectoryView,
  type SourceRecord,
  type SourceStatus,
} from './directory.ts';

function record(url: string, status: SourceStatus, name = url): SourceRecord {
  return {
    url,
    name,
    description: 'a source',
    homepage: null,
    icon: null,
    status,
    submitted_at: '2026-07-01T00:00:00.000Z',
    reviewed_at: null,
    reviewed_by: null,
    app_count: 2,
    last_checked_at: '2026-07-20T00:00:00.000Z',
    last_check_ok: true,
    last_check_error: null,
    downloads_cors_ok: true,
    note: 'internal only',
  };
}

describe('normalizeSourceUrl', () => {
  test('drops the fragment and keeps path and query', () => {
    expect(normalizeSourceUrl('  https://Example.com/c.json?v=2#frag ')).toBe('https://example.com/c.json?v=2');
  });

  test('a submitted bare host becomes an https catalog url rather than an error', () => {
    expect(normalizeSourceUrl('example.com')).toBe('https://example.com/catalog.json');
  });

  test('a submitted directory url is completed, so the record is the manifest and not the folder', () => {
    expect(normalizeSourceUrl('https://example.com/apps/')).toBe('https://example.com/apps/catalog.json');
  });

  test('rejects http because a browser will not read it from an https page', () => {
    expect(() => normalizeSourceUrl('http://example.com/c.json')).toThrow(SourceUrlError);
  });

  test('rejects embedded credentials', () => {
    expect(() => normalizeSourceUrl('https://user:pw@example.com/c.json')).toThrow(SourceUrlError);
  });

  test('rejects garbage', () => {
    expect(() => normalizeSourceUrl('not a url')).toThrow(SourceUrlError);
  });

  test('is the kv identity, so two spellings of one url collide', () => {
    expect(keyFor(normalizeSourceUrl('https://example.com/c.json#a'))).toBe(
      keyFor(normalizeSourceUrl('https://example.com/c.json')),
    );
  });
});

describe('toCatalogDocument', () => {
  const records = [
    record('https://a.example/c.json', 'quarantined', 'quarantined source'),
    record('https://b.example/c.json', 'listed', 'listed source'),
    record('https://c.example/c.json', 'attested', 'attested source'),
    record('https://d.example/c.json', 'rejected', 'rejected source'),
  ];

  const doc = toCatalogDocument(records, '2026-07-24T00:00:00.000Z');

  test('is a valid catalog.v1 document that offers no apps of its own', () => {
    expect(() => validate(doc)).not.toThrow();
    expect(doc.apps).toEqual([]);
  });

  test('publishes only listed and attested', () => {
    expect(doc.recommended_sources.map(s => s.url)).toEqual(['https://c.example/c.json', 'https://b.example/c.json']);
  });

  test('quarantined never reaches a companion quick-add list', () => {
    expect(doc.recommended_sources.some(s => s.url.includes('a.example'))).toBe(false);
  });

  test('carries the attestation flag through', () => {
    expect(doc.recommended_sources.map(s => s.attested)).toEqual([true, false]);
  });
});

describe('toDirectoryView', () => {
  const view = toDirectoryView([
    record('https://a.example/c.json', 'quarantined'),
    record('https://b.example/c.json', 'rejected'),
    record('https://c.example/c.json', 'attested'),
  ]);

  test('shows quarantined to the site but hides rejected', () => {
    expect(view.map(s => s.status)).toEqual(['attested', 'quarantined']);
  });

  test('never leaks the admin note', () => {
    for (const entry of view) expect(entry).not.toHaveProperty('note');
  });
});

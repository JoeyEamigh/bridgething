import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { fetchJamCatalog, fetchJamGallery, submitJamEntry } from './jam-client';

type Call = { url: string; init: RequestInit | undefined };

const original = globalThis.fetch;
let calls: Call[] = [];

beforeEach(() => {
  calls = [];
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    return new Response(JSON.stringify({ entries: [], catalog: { apps: [] }, entry: { app_id: 'x' } }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = original;
});

describe('jam client caching', () => {
  test('the gallery bypasses the browser cache so a fresh entry shows up on reload', async () => {
    await fetchJamGallery();

    expect(calls[0]?.url).toBe('/api/jam/entries.json');
    expect(calls[0]?.init?.cache).toBe('no-store');
  });

  test('the picker never reads a stale catalog out of the browser cache', async () => {
    await fetchJamCatalog('https://third.example/catalog.json');

    expect(calls[0]?.url).toBe('/api/jam/catalog?url=https%3A%2F%2Fthird.example%2Fcatalog.json');
    expect(calls[0]?.init?.cache).toBe('no-store');
  });

  test('a submission posts json', async () => {
    await submitJamEntry({
      source_url: 'https://third.example/catalog.json',
      app_id: 'x',
      category: 'utility',
      video_url: 'https://youtu.be/x',
      discord: 'somebody',
      wishlist: '',
      claim: null,
    });

    expect(calls[0]?.init?.method).toBe('POST');
  });
});

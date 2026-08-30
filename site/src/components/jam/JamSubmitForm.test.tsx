import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { mount, type Mounted } from '../mount';
import { JamSubmitForm } from './JamSubmitForm';

const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const CATALOG_URL = 'https://third.example/catalog.json';
const CLAIM = 'a'.repeat(64);

const CATALOG = {
  schema: 'catalog.v1',
  updated_at: '2026-07-01T00:00:00Z',
  repo: { name: 'third party apps', description: 'some apps', homepage: null, icon: null },
  apps: [
    {
      id: APP_ID,
      name: 'Thing',
      description: 'Does a thing.',
      author: 'somebody',
      icon: null,
      homepage: null,
      source: 'https://github.com/someone/thing',
      versions: [],
    },
  ],
  recommended_sources: [],
};

const CATALOG_CALL = `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`;

let calls: { method: string; url: string }[] = [];
let copied: string[] = [];
let known = true;
let sourceBudgetSpent = false;
let emptyCatalog = false;
let held: Mounted | null = null;
const originalFetch = globalThis.fetch;
const originalNavigator = globalThis.navigator;

function body(payload: unknown, status: number): Response {
  return new Response(JSON.stringify(payload), { status, headers: { 'content-type': 'application/json' } });
}

function reply(url: string): Response {
  if (url.startsWith('/api/sources')) {
    if (sourceBudgetSpent) return body({ error: 'at most 5 submissions per hour. try again later.' }, 429);
    known = true;
    return body({ source: { url: CATALOG_URL, status: 'quarantined' } }, 200);
  }

  if (url.startsWith('/api/jam/catalog')) {
    if (!known) return body({ error: `${CATALOG_URL} is not in the directory; submit it first` }, 404);
    return body({ url: CATALOG_URL, catalog: emptyCatalog ? { ...CATALOG, apps: [] } : CATALOG }, 200);
  }

  return body({ entry: { app_id: APP_ID }, claim: CLAIM }, 200);
}

async function enter(form: Mounted): Promise<void> {
  await form.fill('#jam-source', CATALOG_URL);
  await form.submit('form');
  await form.click('li button');
  await form.fill('#jam-video', 'https://youtu.be/abcdef');
  await form.fill('#jam-discord', 'somebody');
  await form.submit('form:last-of-type');
}

beforeEach(() => {
  calls = [];
  copied = [];
  known = true;
  sourceBudgetSpent = false;
  emptyCatalog = false;
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ method: init?.method ?? 'GET', url });
    return reply(url);
  }) as unknown as typeof fetch;
  Object.defineProperty(globalThis, 'navigator', {
    value: { clipboard: { writeText: async (value: string) => void copied.push(value) } },
    configurable: true,
    writable: true,
  });
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  Object.defineProperty(globalThis, 'navigator', { value: originalNavigator, configurable: true, writable: true });
  held?.unmount();
  held = null;
});

describe('loading the apps out of a catalog', () => {
  test('a source the directory already has costs nothing out of the submission budget', async () => {
    held = mount(<JamSubmitForm />);

    await held.fill('#jam-source', CATALOG_URL);
    await held.submit('form');

    expect(calls).toEqual([{ method: 'GET', url: CATALOG_CALL }]);
    expect(held.text()).toContain('Does a thing.');
  });

  test('a source the directory has never seen is registered, then relayed', async () => {
    known = false;
    held = mount(<JamSubmitForm />);

    await held.fill('#jam-source', CATALOG_URL);
    await held.submit('form');

    expect(calls).toEqual([
      { method: 'GET', url: CATALOG_CALL },
      { method: 'POST', url: '/api/sources' },
      { method: 'GET', url: CATALOG_CALL },
    ]);
    expect(held.text()).toContain('Does a thing.');
  });

  test('an empty url says so rather than looking like a dead button', async () => {
    held = mount(<JamSubmitForm />);

    await held.submit('form');

    expect(calls).toEqual([]);
    expect(held.text()).toContain('a source url cannot be empty');
  });

  test('a url with no scheme loads instead of tripping the browser into blocking the submit', async () => {
    held = mount(<JamSubmitForm />);

    await held.fill('#jam-source', 'third.example/catalog.json');
    await held.submit('form');

    expect(calls).toEqual([{ method: 'GET', url: CATALOG_CALL }]);
    expect(held.text()).toContain('Does a thing.');
    expect((held.find('#jam-source') as HTMLInputElement).value).toBe(CATALOG_URL);
  });

  test('editing the url drops the loaded catalog so a stale app id cannot be submitted', async () => {
    held = mount(<JamSubmitForm />);

    await held.fill('#jam-source', CATALOG_URL);
    await held.submit('form');
    expect(held.text()).toContain('Does a thing.');

    await held.fill('#jam-source', 'https://other.example/catalog.json');

    expect(held.text()).not.toContain('Does a thing.');
    expect(held.all('li button[aria-pressed]')).toHaveLength(0);
  });

  test('a catalog with no apps says what to do about it', async () => {
    emptyCatalog = true;
    held = mount(<JamSubmitForm />);

    await held.fill('#jam-source', CATALOG_URL);
    await held.submit('form');

    expect(held.text()).toContain('its apps list is empty');
  });

  test('a spent submission budget says so instead of telling you to submit the source', async () => {
    known = false;
    sourceBudgetSpent = true;
    held = mount(<JamSubmitForm />);

    await held.fill('#jam-source', CATALOG_URL);
    await held.submit('form');

    expect(held.text()).toContain('no new sources from this network');
    expect(held.text()).not.toContain('submit it first');
  });
});

describe('the claim token panel', () => {
  test('offers a copy button that puts the token on the clipboard', async () => {
    held = mount(<JamSubmitForm />);
    await enter(held);

    expect(held.text()).toContain(CLAIM);
    await held.click('button.btn-sm');

    expect(copied).toEqual([CLAIM]);
    expect(held.text()).toContain('copied');
  });

  test('points at the directory rather than a store listing no reviewer has published yet', async () => {
    held = mount(<JamSubmitForm />);
    await enter(held);

    expect(held.all('a').map(anchor => anchor.getAttribute('href'))).not.toContain(`/apps/${APP_ID}`);
    expect(held.text()).toContain('the store listing appears once a reviewer lists');

    const directory = held.all('a').find(anchor => anchor.textContent?.includes('app directory'));
    expect(directory?.getAttribute('href')).toBe('/apps');
    expect(directory?.getAttribute('target')).toBe('_blank');
    expect(directory?.getAttribute('rel')).toBe('noreferrer');
  });
});

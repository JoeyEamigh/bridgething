import { beforeEach, describe, expect, test } from 'bun:test';

import type { SettingsContext, Theme } from '../src/settings-protocol.js';
import { THEME_DEFAULTS, THEME_DEFAULTS_CSS, themeCss, themeVariable } from '../src/theme.js';

import { bridge, deliver, page, resetBridge, session } from './host.js';

const { settings } = await import('../src/settings.js');

const LIGHT: Theme = {
  scheme: 'light',
  colors: { ...THEME_DEFAULTS.colors.light, accent: '#ff00aa' },
  text: { ...THEME_DEFAULTS.text, body: 16 },
  fonts: { ...THEME_DEFAULTS.fonts, body: 'Georgia, serif' },
};

const DARK: Theme = {
  scheme: 'dark',
  colors: THEME_DEFAULTS.colors.dark,
  text: THEME_DEFAULTS.text,
  fonts: THEME_DEFAULTS.fonts,
};

function context(theme?: Theme): SettingsContext {
  return { webappId: 'w', name: 'weather', version: '1.0.0', deviceId: 'd', ...(theme ? { theme } : {}) };
}

function painted(): HTMLStyleElement {
  const style = page.document.querySelector('style[data-bridgething-theme]');
  if (!style) throw new Error('the sdk painted no theme');
  return style as unknown as HTMLStyleElement;
}

function root(variable: string): string {
  return page.getComputedStyle(page.document.documentElement).getPropertyValue(variable).trim();
}

beforeEach(() => {
  resetBridge();
  deliver({ event: 'themeChanged', theme: DARK });
});

describe('theme', () => {
  test('paints the defaults ahead of every page stylesheet as soon as the sdk loads', () => {
    expect(page.document.head.firstChild).toBe(painted() as never);
  });

  test('applies the host theme from the context reply', async () => {
    bridge.handle = () => context(LIGHT);

    await settings.context();

    expect(painted().textContent).toBe(THEME_DEFAULTS_CSS + themeCss(LIGHT));
    expect(painted().textContent).toContain(`${themeVariable('color', 'accent')}:#ff00aa;`);
    expect(painted().textContent).toContain(`${themeVariable('text', 'body')}:16px;`);
    expect(painted().textContent).toContain('--bt-scheme:light;');
  });

  test('keeps the defaults for a host that sends no theme', async () => {
    deliver({ event: 'themeChanged', theme: LIGHT });
    bridge.handle = () => context();

    const reply = await settings.context();

    expect(reply.theme).toBeUndefined();
    expect(painted().textContent).toBe(THEME_DEFAULTS_CSS + themeCss(LIGHT));
  });

  test('repaints and tells listeners when the host scheme flips', () => {
    const heard: Theme[] = [];
    const off = settings.onThemeChanged(theme => heard.push(theme));

    deliver({ event: 'themeChanged', theme: LIGHT });
    off();
    deliver({ event: 'themeChanged', theme: DARK });

    expect(heard).toEqual([LIGHT]);
    expect(painted().textContent).toBe(THEME_DEFAULTS_CSS + themeCss(DARK));
  });

  test('repaints from a context change and hands listeners the new context', () => {
    const heard: SettingsContext[] = [];
    const off = settings.onContextChanged(next => heard.push(next));

    session.retarget('app', context(LIGHT));
    off();

    expect(heard).toEqual([context(LIGHT)]);
    expect(painted().textContent).toBe(THEME_DEFAULTS_CSS + themeCss(LIGHT));
  });

  test('loses to any page rule on :root', () => {
    const own = page.document.createElement('style');
    own.textContent = ':root{--bt-color-accent:#00ff00}';
    page.document.head.appendChild(own);
    deliver({ event: 'themeChanged', theme: LIGHT });

    expect(root('--bt-color-accent')).toBe('#00ff00');
    expect(root('--bt-color-bg')).toBe(LIGHT.colors.bg);

    own.remove();
  });

  test('carries no specificity, so a page rule ahead of it in the document still wins', () => {
    const own = page.document.createElement('style');
    own.textContent = 'html{--bt-color-accent:#00ff00}';
    page.document.head.insertBefore(own, painted() as never);
    deliver({ event: 'themeChanged', theme: LIGHT });

    expect(page.document.head.firstChild).toBe(own as never);
    expect(root('--bt-color-accent')).toBe('#00ff00');
    expect(root('--bt-color-bg')).toBe(LIGHT.colors.bg);

    own.remove();
  });
});

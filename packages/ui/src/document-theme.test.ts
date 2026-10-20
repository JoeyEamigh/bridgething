import { THEME_DEFAULTS, themeToken } from '@bridgething/client/theme';
import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { readTheme, schemeFor } from './document-theme.ts';

const CSS = readFileSync(join(import.meta.dir, 'theme.css'), 'utf8');
const THEME_BLOCK = CSS.slice(CSS.indexOf('@theme'), CSS.indexOf('\n}\n'));

function declared(group: 'color' | 'font' | 'text', tokens: object): string[] {
  return Object.keys(tokens).filter(token => !THEME_BLOCK.includes(`--${themeToken(group, token)}:`));
}

describe('settings theme', () => {
  test('theme.css declares every token a settings page paints, so reading it never comes up empty', () => {
    expect(THEME_BLOCK).toStartWith('@theme static');
    expect(declared('color', THEME_DEFAULTS.colors.dark)).toEqual([]);
    expect(declared('font', THEME_DEFAULTS.fonts)).toEqual([]);
    expect(declared('text', THEME_DEFAULTS.text)).toEqual([]);
  });

  test('reads tokens by their css names and turns rem sizes into pixels', () => {
    const css: Record<string, string> = {
      '--color-bg': ' #f2f4f6',
      '--color-rule-strong': 'color-mix(in srgb, #0a0c0e 15%, transparent)',
      '--font-body': "'Inter', system-ui, sans-serif",
      '--text-row-lg': '1.0625rem',
      '--text-hint': '12px',
    };

    const theme = readTheme('light', variable => css[variable] ?? '', 16);

    expect(theme.scheme).toBe('light');
    expect(theme.colors.bg).toBe('#f2f4f6');
    expect(theme.colors.ruleStrong).toBe('color-mix(in srgb, #0a0c0e 15%, transparent)');
    expect(theme.fonts.body).toBe("'Inter', system-ui, sans-serif");
    expect(theme.text.rowLg).toBe(17);
    expect(theme.text.hint).toBe(12);
    expect(Object.keys(theme.colors).sort()).toEqual(Object.keys(THEME_DEFAULTS.colors.dark).sort());
  });

  test('follows a forced theme, and the os preference under auto', () => {
    expect(schemeFor('light', false)).toBe('light');
    expect(schemeFor('dark', true)).toBe('dark');
    expect(schemeFor('auto', true)).toBe('light');
    expect(schemeFor('auto', false)).toBe('dark');
  });

  test('a root with no data-theme is dark whatever the os prefers, as theme.css paints it', () => {
    expect(CSS).toMatch(/\n:root \{\n\s*color-scheme: only dark;/);
    expect(schemeFor(undefined, false)).toBe('dark');
    expect(schemeFor(undefined, true)).toBe('dark');
  });
});

import { THEME_DEFAULTS } from '@bridgething/client/theme';
import { readFileSync } from 'fs';
import { join } from 'path';

import {
  PALETTE,
  settingsTheme,
  TYPE,
  type Palette,
  type Scheme,
} from '../lib/theme';

const CSS = readFileSync(join(__dirname, '..', 'global.css'), 'utf8');
const DARK_AT = CSS.indexOf('@media (prefers-color-scheme: dark)');

const DECLARED: Record<Scheme, Record<string, string>> = {
  light: declarations(rootBlock(CSS.slice(0, DARK_AT))),
  dark: declarations(rootBlock(CSS.slice(DARK_AT))),
};

const SCHEMES: Scheme[] = ['light', 'dark'];

function rootBlock(source: string): string {
  const open = source.indexOf('{', source.indexOf(':root'));
  return source.slice(open + 1, source.indexOf('}', open));
}

function declarations(block: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [, name, value] of block.matchAll(/--([\w-]+):\s*([^;]+);/g)) {
    out[camel(name)] = value.trim();
  }
  return out;
}

function camel(name: string): string {
  return name.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());
}

function rgba(value: string): [number, number, number, number] {
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  if (hex) {
    const packed = parseInt(hex[1], 16);
    return [(packed >> 16) & 0xff, (packed >> 8) & 0xff, packed & 0xff, 1];
  }

  const fn = /^rgba?\(([^)]+)\)$/i.exec(value);
  if (fn) {
    const parts = fn[1].split(',').map(part => Number(part.trim()));
    if (parts.length >= 3 && parts.every(part => Number.isFinite(part))) {
      return [parts[0], parts[1], parts[2], parts.length > 3 ? parts[3] : 1];
    }
  }

  throw new Error(`unreadable colour: ${value}`);
}

describe('theme tokens', () => {
  test('both palettes carry the same keys', () => {
    expect(Object.keys(PALETTE.light).sort()).toEqual(
      Object.keys(PALETTE.dark).sort(),
    );
  });

  test.each(SCHEMES)('%s css declares exactly the palette keys', scheme => {
    expect(Object.keys(DECLARED[scheme]).sort()).toEqual(
      Object.keys(PALETTE[scheme]).sort(),
    );
  });

  test.each(SCHEMES)('%s css resolves to the palette values', scheme => {
    for (const [key, value] of Object.entries(PALETTE[scheme])) {
      expect({ [key]: rgba(DECLARED[scheme][key]) }).toEqual({
        [key]: rgba(value as Palette[keyof Palette]),
      });
    }
  });

  test.each(SCHEMES)('%s settings form defaults match the palette', scheme => {
    expect(THEME_DEFAULTS.colors[scheme]).toEqual(PALETTE[scheme]);
  });

  test('settings form type scale matches the app', () => {
    expect(THEME_DEFAULTS.text).toEqual(TYPE);
  });

  test.each(SCHEMES)(
    '%s settings theme carries every token the page sdk paints',
    scheme => {
      const theme = settingsTheme(scheme);
      expect(theme.scheme).toBe(scheme);
      expect(theme.colors).toEqual(THEME_DEFAULTS.colors[scheme]);
      expect(theme.text).toEqual(THEME_DEFAULTS.text);
      expect(Object.keys(theme.fonts).sort()).toEqual(
        Object.keys(THEME_DEFAULTS.fonts).sort(),
      );
    },
  );
});

import {
  DarkTheme,
  DefaultTheme,
  type Theme as NavigationTheme,
} from '@react-navigation/native';
import { useColorScheme, type ColorSchemeName } from 'react-native';

import type { Theme } from '@bridgething/client/settings-protocol';
import type { BoxSize, Tone } from '@bridgething/ui/tokens';

export type { BoxSize, Tone };

export type Scheme = 'light' | 'dark';

export type Palette = {
  bg: string;
  screen: string;
  fg: string;
  muted: string;
  accent: string;
  ok: string;
  err: string;
  warn: string;
  experimental: string;
  rule: string;
  ruleStrong: string;
  edge: string;
  dim: string;
  soft: string;
  near: string;
  neutralSoft: string;
  accentSoft: string;
  okSoft: string;
  errSoft: string;
  warnSoft: string;
  experimentalSoft: string;
  scrim: string;
};

export const PALETTE: Record<Scheme, Palette> = {
  dark: {
    bg: '#0a0c0e',
    screen: '#060809',
    fg: '#efefef',
    muted: '#a7adb5',
    accent: '#00a8e8',
    ok: '#3ddc84',
    err: '#dc3d3d',
    warn: '#ff7070',
    experimental: '#ffb066',
    rule: 'rgba(239, 239, 239, 0.1)',
    ruleStrong: 'rgba(239, 239, 239, 0.15)',
    edge: 'rgba(239, 239, 239, 0.25)',
    dim: 'rgba(239, 239, 239, 0.35)',
    soft: 'rgba(239, 239, 239, 0.55)',
    near: 'rgba(239, 239, 239, 0.8)',
    neutralSoft: 'rgba(239, 239, 239, 0.08)',
    accentSoft: 'rgba(0, 168, 232, 0.14)',
    okSoft: 'rgba(61, 220, 132, 0.14)',
    errSoft: 'rgba(220, 61, 61, 0.14)',
    warnSoft: 'rgba(255, 112, 112, 0.14)',
    experimentalSoft: 'rgba(255, 176, 102, 0.14)',
    scrim: 'rgba(0, 0, 0, 0.55)',
  },
  light: {
    bg: '#f2f4f6',
    screen: '#ffffff',
    fg: '#0a0c0e',
    muted: '#5c6670',
    accent: '#0072a3',
    ok: '#106e3f',
    err: '#b93030',
    warn: '#a34848',
    experimental: '#8a5210',
    rule: 'rgba(10, 12, 14, 0.1)',
    ruleStrong: 'rgba(10, 12, 14, 0.15)',
    edge: 'rgba(10, 12, 14, 0.25)',
    dim: 'rgba(10, 12, 14, 0.35)',
    soft: 'rgba(10, 12, 14, 0.55)',
    near: 'rgba(10, 12, 14, 0.8)',
    neutralSoft: 'rgba(10, 12, 14, 0.06)',
    accentSoft: 'rgba(0, 114, 163, 0.12)',
    okSoft: 'rgba(16, 110, 63, 0.12)',
    errSoft: 'rgba(185, 48, 48, 0.12)',
    warnSoft: 'rgba(163, 72, 72, 0.12)',
    experimentalSoft: 'rgba(138, 82, 16, 0.12)',
    scrim: 'rgba(0, 0, 0, 0.55)',
  },
};

export const TYPE = {
  eyebrow: 11,
  hint: 12,
  body: 14,
  row: 15,
  rowLg: 17,
  title: 20,
  hero: 22,
  screenTitle: 34,
} as const;

export const FONT = {
  display: "'Outfit', system-ui, sans-serif",
  body: "'Inter', system-ui, sans-serif",
  mono: "'JetBrains Mono', ui-monospace, monospace",
} as const;

export function settingsTheme(scheme: Scheme): Theme {
  return { scheme, colors: PALETTE[scheme], text: TYPE, fonts: FONT };
}

export const SPACE = {
  gutter: 16,
  rowX: 16,
  rowY: 12,
  headingGap: 8,
  section: 32,
  screenHeader: 24,
} as const;

const EYEBROW_TRACKING = 2;

export const TEXT = {
  eyebrow: { fontSize: TYPE.eyebrow, letterSpacing: EYEBROW_TRACKING },
  hint: { fontSize: TYPE.hint },
  body: { fontSize: TYPE.body },
  row: { fontSize: TYPE.row },
  rowLg: { fontSize: TYPE.rowLg },
  title: { fontSize: TYPE.title },
  hero: { fontSize: TYPE.hero, lineHeight: Math.round(TYPE.hero * 1.2) },
  screenTitle: {
    fontSize: TYPE.screenTitle,
    lineHeight: Math.round(TYPE.screenTitle * 1.1),
    letterSpacing: TYPE.screenTitle * -0.03,
  },
} as const;

export const BOX: Record<BoxSize, number> = { sm: 32, md: 44, lg: 56, xl: 80 };

export const BOX_TEXT: Record<BoxSize, number> = {
  sm: TYPE.body,
  md: TYPE.rowLg,
  lg: TYPE.title,
  xl: TYPE.screenTitle,
};

const TONE_KEY: Record<Tone, keyof Palette> = {
  neutral: 'soft',
  accent: 'accent',
  ok: 'ok',
  err: 'err',
  warn: 'warn',
  experimental: 'experimental',
};

const TONE_SOFT_KEY: Record<Tone, keyof Palette> = {
  neutral: 'neutralSoft',
  accent: 'accentSoft',
  ok: 'okSoft',
  err: 'errSoft',
  warn: 'warnSoft',
  experimental: 'experimentalSoft',
};

export function toneColor(palette: Palette, tone: Tone): string {
  return palette[TONE_KEY[tone]];
}

export function toneSoftColor(palette: Palette, tone: Tone): string {
  return palette[TONE_SOFT_KEY[tone]];
}

export function schemeOf(name: ColorSchemeName | null | undefined): Scheme {
  return name === 'dark' ? 'dark' : 'light';
}

export function useScheme(): Scheme {
  return schemeOf(useColorScheme());
}

export function usePalette(): Palette {
  return PALETTE[useScheme()];
}

export function useToneColor(tone: Tone): string {
  return toneColor(usePalette(), tone);
}

export const navTheme: Record<Scheme, NavigationTheme> = {
  dark: navigationTheme('dark'),
  light: navigationTheme('light'),
};

function navigationTheme(scheme: Scheme): NavigationTheme {
  const palette = PALETTE[scheme];
  const base = scheme === 'dark' ? DarkTheme : DefaultTheme;
  return {
    ...base,
    dark: scheme === 'dark',
    colors: {
      ...base.colors,
      background: palette.bg,
      card: palette.bg,
      text: palette.fg,
      border: palette.rule,
      primary: palette.accent,
      notification: palette.err,
    },
  };
}

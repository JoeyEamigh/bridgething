export type ThemeScheme = 'light' | 'dark';

export type ThemeColor =
  | 'bg'
  | 'screen'
  | 'fg'
  | 'muted'
  | 'accent'
  | 'ok'
  | 'err'
  | 'warn'
  | 'experimental'
  | 'rule'
  | 'ruleStrong'
  | 'edge'
  | 'dim'
  | 'soft'
  | 'near'
  | 'neutralSoft'
  | 'accentSoft'
  | 'okSoft'
  | 'errSoft'
  | 'warnSoft'
  | 'experimentalSoft'
  | 'scrim';

export type ThemeFont = 'display' | 'body' | 'mono';
export type ThemeText = 'eyebrow' | 'hint' | 'body' | 'row' | 'rowLg' | 'title' | 'hero' | 'screenTitle';

export type ThemeDefaults = {
  colors: Record<ThemeScheme, Record<ThemeColor, string>>;
  fonts: Record<ThemeFont, string>;
  text: Record<ThemeText, number>;
};

export const THEME_DEFAULTS: ThemeDefaults = {
  colors: {
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
  },
  fonts: {
    display: "'Outfit', system-ui, sans-serif",
    body: "'Inter', system-ui, sans-serif",
    mono: "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
  },
  text: {
    eyebrow: 11,
    hint: 12,
    body: 14,
    row: 15,
    rowLg: 17,
    title: 20,
    hero: 22,
    screenTitle: 34,
  },
};

export type ThemeGroup = 'color' | 'font' | 'text';

export type Theme = {
  scheme: ThemeScheme;
  colors: Record<ThemeColor, string>;
  text: Record<ThemeText, number>;
  fonts: Record<ThemeFont, string>;
};

function kebab(token: string): string {
  return token.replace(/[A-Z]/g, letter => `-${letter.toLowerCase()}`);
}

export function themeToken(group: ThemeGroup, token: string): string {
  return `${group}-${kebab(token)}`;
}

export function themeVariable(group: ThemeGroup, token: string): string {
  return `--bt-${themeToken(group, token)}`;
}

function declarations(
  colors: Record<ThemeColor, string>,
  type?: { fonts: Record<ThemeFont, string>; text: Record<ThemeText, number> },
): string {
  const lines = Object.entries(colors).map(([token, value]) => `${themeVariable('color', token)}:${value};`);
  if (type) {
    for (const [token, value] of Object.entries(type.fonts)) lines.push(`${themeVariable('font', token)}:${value};`);
    for (const [token, value] of Object.entries(type.text)) lines.push(`${themeVariable('text', token)}:${value}px;`);
  }
  return lines.join('');
}

export const THEME_DEFAULTS_CSS =
  `:where(:root){--bt-scheme:light dark;${declarations(THEME_DEFAULTS.colors.dark, THEME_DEFAULTS)}}` +
  `@media (prefers-color-scheme: light){:where(:root){${declarations(THEME_DEFAULTS.colors.light)}}}`;

export function themeCss(theme: Theme): string {
  return `:where(:root){--bt-scheme:${theme.scheme};${declarations(theme.colors, theme)}}`;
}

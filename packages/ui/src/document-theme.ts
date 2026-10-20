import type { Theme, ThemeScheme } from '@bridgething/client/settings-protocol';
import { THEME_DEFAULTS, themeToken, type ThemeGroup } from '@bridgething/client/theme';

const PREFERS_LIGHT = '(prefers-color-scheme: light)';

export function schemeFor(mode: string | undefined, prefersLight: boolean): ThemeScheme {
  if (mode === 'light') return 'light';
  return mode === 'auto' && prefersLight ? 'light' : 'dark';
}

function tokens<K extends string, V>(keys: Record<K, unknown>, value: (token: K) => V): Record<K, V> {
  return Object.fromEntries(Object.keys(keys).map(token => [token, value(token as K)])) as Record<K, V>;
}

function pixels(length: string, remPx: number): number {
  const size = Number.parseFloat(length);
  return length.endsWith('rem') ? size * remPx : size;
}

export function readTheme(scheme: ThemeScheme, read: (variable: string) => string, remPx: number): Theme {
  const value = (group: ThemeGroup, token: string) => read(`--${themeToken(group, token)}`).trim();
  return {
    scheme,
    colors: tokens(THEME_DEFAULTS.colors.dark, token => value('color', token)),
    text: tokens(THEME_DEFAULTS.text, token => pixels(value('text', token), remPx)),
    fonts: tokens(THEME_DEFAULTS.fonts, token => value('font', token)),
  };
}

export function documentTheme(): Theme {
  const root = document.documentElement;
  const style = getComputedStyle(root);
  return readTheme(
    schemeFor(root.dataset.theme, matchMedia(PREFERS_LIGHT).matches),
    variable => style.getPropertyValue(variable),
    Number.parseFloat(style.fontSize),
  );
}

export function watchTheme(changed: (theme: Theme) => void): () => void {
  const media = matchMedia(PREFERS_LIGHT);
  let scheme = documentTheme().scheme;
  const fire = () => {
    const theme = documentTheme();
    if (theme.scheme === scheme) return;
    scheme = theme.scheme;
    changed(theme);
  };
  const observer = new MutationObserver(fire);
  media.addEventListener('change', fire);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  return () => {
    media.removeEventListener('change', fire);
    observer.disconnect();
  };
}

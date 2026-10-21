const { hairlineWidth } = require('nativewind/theme');

/** @type {import('tailwindcss').Config} */
module.exports = {
  darkMode: 'class',
  content: [
    './App.tsx',
    './app/**/*.{ts,tsx}',
    './components/**/*.{ts,tsx}',
    './screens/**/*.{ts,tsx}',
    './lib/**/*.{ts,tsx}',
    '../packages/ui/src/tokens.ts',
  ],
  presets: [require('nativewind/preset')],
  theme: {
    extend: {
      colors: {
        bg: 'var(--bg)',
        screen: 'var(--screen)',
        fg: 'var(--fg)',
        rule: 'var(--rule)',
        'rule-strong': 'var(--rule-strong)',
        edge: 'var(--edge)',
        dim: 'var(--dim)',
        soft: 'var(--soft)',
        near: 'var(--near)',
        'neutral-soft': 'var(--neutral-soft)',
        muted: {
          DEFAULT: 'var(--muted)',
          foreground: 'var(--muted)',
        },
        accent: {
          DEFAULT: 'var(--accent)',
          soft: 'var(--accent-soft)',
          foreground: 'var(--bg)',
        },
        ok: {
          DEFAULT: 'var(--ok)',
          soft: 'var(--ok-soft)',
        },
        err: {
          DEFAULT: 'var(--err)',
          soft: 'var(--err-soft)',
        },
        warn: {
          DEFAULT: 'var(--warn)',
          soft: 'var(--warn-soft)',
        },
        experimental: {
          DEFAULT: 'var(--experimental)',
          soft: 'var(--experimental-soft)',
        },

        // aliases so components not yet ported resolve to terminal tokens
        background: 'var(--bg)',
        foreground: 'var(--fg)',
        border: 'var(--rule)',
        'border-strong': 'var(--rule-strong)',
        input: 'var(--rule-strong)',
        ring: 'var(--accent)',
        surface: {
          DEFAULT: 'var(--screen)',
          foreground: 'var(--fg)',
          subtle: 'var(--neutral-soft)',
        },
        card: {
          DEFAULT: 'var(--screen)',
          foreground: 'var(--fg)',
        },
        popover: {
          DEFAULT: 'var(--screen)',
          foreground: 'var(--fg)',
        },
        primary: {
          DEFAULT: 'var(--accent)',
          foreground: 'var(--bg)',
          soft: 'var(--accent-soft)',
        },
        secondary: {
          DEFAULT: 'var(--neutral-soft)',
          foreground: 'var(--fg)',
        },
        destructive: {
          DEFAULT: 'var(--err)',
          foreground: 'var(--bg)',
          soft: 'var(--err-soft)',
        },
        success: {
          DEFAULT: 'var(--ok)',
          foreground: 'var(--bg)',
          soft: 'var(--ok-soft)',
          'soft-foreground': 'var(--ok)',
        },
        warning: {
          DEFAULT: 'var(--warn)',
          foreground: 'var(--bg)',
        },
      },
      borderRadius: {
        none: '0',
        sm: '0',
        DEFAULT: '0',
        md: '0',
        lg: '0',
        xl: '0',
        '2xl': '0',
        '3xl': '0',
        full: '0',
      },
      borderWidth: {
        hairline: hairlineWidth(),
      },
      fontFamily: {
        sans: ['Inter-Regular', 'System'],
        display: ['Outfit-Medium', 'System'],
        'display-light': ['Outfit-ExtraLight', 'System'],
        mono: ['JetBrainsMono-Regular'],
      },
    },
  },
  future: {
    hoverOnlyWhenSupported: true,
  },
  plugins: [require('tailwindcss-animate')],
};

import type { VNode } from 'preact';

const PATHS = {
  device: 'M4 6h16v12H4z M8 18v2h8v-2',
  plus: 'M12 5v14 M5 12h14',
  plug: 'M9 2v6 M15 2v6 M6 8h12v3a6 6 0 0 1-12 0z M12 17v5',
  grid: 'M3 3h7v7H3z M14 3h7v7h-7z M3 14h7v7H3z M14 14h7v7h-7z',
  layers: 'M12 3 3 8l9 5 9-5z M3 14l9 5 9-5',
  store: 'M3 7h18l-1.5 4a3 3 0 0 1-2.9 2.2H7.4A3 3 0 0 1 4.5 11z M5 13v8h14v-8 M3 7l2-4h14l2 4',
  download: 'M12 3v12 M7 11l5 5 5-5 M4 20h16',
  down: 'M12 4v15 M6 13l6 6 6-6',
  upload: 'M12 21V9 M7 13l5-5 5 5 M4 4h16',
  terminal: 'M4 5h16v14H4z M8 10l3 2-3 2 M13 14h4',
  gear: 'M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z M12 2v3 M12 19v3 M2 12h3 M19 12h3 M4.9 4.9l2.1 2.1 M17 17l2.1 2.1 M19.1 4.9 17 7 M7 17l-2.1 2.1',
  pencil: 'M4 20h4L20 8l-4-4L4 16z',
  trash: 'M4 7h16 M9 7V4h6v3 M6 7l1 13h10l1-13',
  play: 'M8 5l11 7-11 7z',
  refresh: 'M20 12a8 8 0 1 1-2.3-5.6 M20 4v5h-5',
  check: 'M5 13l4 4L19 7',
  back: 'M15 6l-6 6 6 6',
  link: 'M10 14a4 4 0 0 0 6 .5l3-3a4 4 0 0 0-5.7-5.7L11.6 7.5 M14 10a4 4 0 0 0-6-.5l-3 3a4 4 0 0 0 5.7 5.7l1.7-1.7',
  globe:
    'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M3 12h18 M12 3c2.5 2.6 3.8 5.6 3.8 9s-1.3 6.4-3.8 9c-2.5-2.6-3.8-5.6-3.8-9S9.5 5.6 12 3z',
  wifi: 'M2.5 8.5a15 15 0 0 1 19 0 M5.5 12a11 11 0 0 1 13 0 M8.5 15.5a7 7 0 0 1 7 0 M12 19h0',
  pin: 'M12 21s7-6.2 7-11a7 7 0 1 0-14 0c0 4.8 7 11 7 11z M12 12a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z',
  bell: 'M6 9a6 6 0 1 1 12 0c0 5 2 6 2 6H4s2-1 2-6z M10 20a2 2 0 0 0 4 0',
  speaker: 'M4 9h4l5-4v14l-5-4H4z M17 9a4 4 0 0 1 0 6',
  mic: 'M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3z M5 11a7 7 0 0 0 14 0 M12 18v3',
  shield: 'M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z',
  user: 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8z M4 20a8 8 0 0 1 16 0',
  signIn: 'M10 5H5v14h5 M14 8l4 4-4 4 M18 12H9',
  signOut: 'M14 5h5v14h-5 M10 8l-4 4 4 4 M6 12h9',
  power: 'M12 3v9 M7.5 6.5a7 7 0 1 0 9 0',
  info: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M12 11v6 M12 8h0',
  undo: 'M4 9h10a5 5 0 0 1 0 10H9 M4 9l4-4 M4 9l4 4',
  search: 'M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M16.5 16.5 21 21',
  file: 'M6 3h8l4 4v14H6z M14 3v4h4',
  copy: 'M9 9h10v11H9z M5 15V4h10',
  clock: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z M12 7v5l3 2',
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({ name, size = 16, class: className }: { name: IconName; size?: number; class?: string }): VNode {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      stroke-width="1.7"
      stroke-linecap="square"
      stroke-linejoin="miter"
      class={className}>
      <path d={PATHS[name]} />
    </svg>
  );
}

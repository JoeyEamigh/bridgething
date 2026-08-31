import type { SourceStatus } from './directory-client.ts';

export const JAM_CATEGORY_IDS = ['launcher', 'music', 'utility', 'desk', 'cursed', 'joey'] as const;

export type JamCategory = (typeof JAM_CATEGORY_IDS)[number];

export const JAM_ENTRY_STATUSES = ['submitted', 'verified', 'disqualified'] as const;

export type JamEntryStatus = (typeof JAM_ENTRY_STATUSES)[number];

export type JamListing = {
  app_id: string;
  source_url: string;
  category: JamCategory;
  video_url: string;
  status: JamEntryStatus;
  submitted_at: string;
  name: string | null;
  description: string | null;
  author: string | null;
  icon: string | null;
  screenshot: string | null;
  repo: string | null;
};

export type JamEntry = {
  app_id: string;
  source_url: string;
  category: JamCategory;
  video_url: string;
  discord: string;
  wishlist: string;
  status: JamEntryStatus;
  submitted_at: string;
  updated_at: string;
  claim_hash: string;
};

export type JamEntryView = Omit<JamEntry, 'claim_hash'>;

export type JamScoreView = { category: JamCategory; score: number; note: string | null };

export type JamReviewEntry = JamListing & {
  discord: string;
  wishlist: string;
  installs: number;
  source_status: SourceStatus | null;
  scores: JamScoreView[];
};

export type JamTallyScore = { handle: string; score: number; note: string | null };

export type JamTallyEntry = {
  app_id: string;
  name: string | null;
  primary: boolean;
  mean: number | null;
  count: number;
  scores: JamTallyScore[];
};

export type JamTallyCategory = { category: JamCategory; entries: JamTallyEntry[] };

export type JamCategoryCopy = {
  id: JamCategory;
  label: string;
  brief: string;
  detail: string;
  first: number;
  second: number | null;
};

export const JAM_CATEGORIES: JamCategoryCopy[] = [
  {
    id: 'launcher',
    label: 'best launcher',
    brief: 'the whole ui',
    detail: 'design a new home screen for bridgething',
    first: 50,
    second: 10,
  },
  {
    id: 'music',
    label: 'best music app',
    brief: 'control now playing',
    detail: "spotify's is nice, yes... but i'm sure you can do better",
    first: 50,
    second: 10,
  },
  {
    id: 'utility',
    label: 'best utility',
    brief: 'something useful',
    detail: 'home assistant, task tracker, something more creative',
    first: 50,
    second: 10,
  },
  {
    id: 'desk',
    label: 'best desk app',
    brief: 'a la deskthing',
    detail: 'bridgething 0.12.0 added desktop extensions. make something cool with those',
    first: 50,
    second: 10,
  },
  {
    id: 'cursed',
    label: 'most cursed',
    brief: "let's get creative",
    detail: 'no rules, have fun',
    first: 10,
    second: null,
  },
  {
    id: 'joey',
    label: "joey's choice",
    brief: 'whatever joey likes',
    detail: "i'll pick my favorite of the submissions",
    first: 10,
    second: null,
  },
];

export const JAM_PRIZE_POOL: number = JAM_CATEGORIES.reduce(
  (total, category) => total + category.first + (category.second ?? 0),
  0,
);

export function jamPrizeLabel(category: JamCategoryCopy): string {
  return category.second === null ? `$${category.first}` : `$${category.first} / $${category.second}`;
}

export type JamSidePrize = { label: string; brief: string };

export const JAM_SIDE_PRIZES: JamSidePrize[] = [
  { label: 'community favorite', brief: 'a discord poll on results day. no cash, all bragging.' },
  { label: "maintainer's pick", brief: 'joey alone, on whatever grounds. no appeal.' },
];

export const JAM_RULES: string[] = [
  'must include a video of the app running on your car thing',
  'must be open source (link repo)',
  'must have a hosted store url (how you submit) with an icon and screenshots',
];

export const JAM_PANEL: string[] = ['ItsRiprod', '68p', 'itsnebulalol', 'espeon', 'lmore377', 'JoeyEamigh'];

export function githubProfile(handle: string): string {
  return `https://github.com/${handle}`;
}

export function githubAvatar(handle: string, size = 160): string {
  return `https://github.com/${handle}.png?size=${size}`;
}

export type JamTimeline = {
  opensAt: string | null;
  closesAt: string | null;
  resultsAt: string | null;
};

export const JAM_TIMELINE: JamTimeline = {
  opensAt: '2026-08-30T00:00:00.000Z',
  closesAt: '2026-09-13T23:59:59.999Z',
  resultsAt: null,
};

export const JAM_DATE_PENDING = 'dates announced soon';

export function jamDate(iso: string | null): string {
  if (iso === null) return JAM_DATE_PENDING;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return JAM_DATE_PENDING;
  return at.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC' });
}

function ordinal(day: number): string {
  const teens = day % 100;
  if (teens >= 11 && teens <= 13) return `${day}th`;
  switch (day % 10) {
    case 1:
      return `${day}st`;
    case 2:
      return `${day}nd`;
    case 3:
      return `${day}rd`;
    default:
      return `${day}th`;
  }
}

export function jamDateLong(iso: string | null): string {
  if (iso === null) return JAM_DATE_PENDING;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return JAM_DATE_PENDING;
  const month = at.toLocaleDateString('en-US', { month: 'long', timeZone: 'UTC' });
  return `${month} ${ordinal(at.getUTCDate())}, ${at.getUTCFullYear()}`;
}

export type JamWindow = { open: true } | { open: false; reason: 'before' | 'after' };

function instant(iso: string | null): number | null {
  if (iso === null) return null;
  const at = Date.parse(iso);
  return Number.isNaN(at) ? null : at;
}

export function jamWindow(timeline: JamTimeline, at: Date = new Date()): JamWindow {
  const now = at.getTime();
  const opens = instant(timeline.opensAt);
  const closes = instant(timeline.closesAt);

  if (opens !== null && now < opens) return { open: false, reason: 'before' };
  if (closes !== null && now > closes) return { open: false, reason: 'after' };
  return { open: true };
}

export function jamClosedReason(timeline: JamTimeline, window: Extract<JamWindow, { open: false }>): string {
  return window.reason === 'before'
    ? `the jam opens ${jamDate(timeline.opensAt)}.`
    : `the jam closed ${jamDate(timeline.closesAt)}.`;
}

export function jamCategoryLabel(id: JamCategory): string {
  return JAM_CATEGORIES.find(category => category.id === id)?.label ?? id;
}

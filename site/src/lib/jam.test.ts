import { describe, expect, test } from 'bun:test';
import {
  githubAvatar,
  githubProfile,
  JAM_CATEGORIES,
  JAM_CATEGORY_IDS,
  JAM_PANEL,
  JAM_PRIZE_POOL,
  JAM_CLOSE_OFFSET_MS,
  JAM_TIMELINE,
  jamPrizeLabel,
  jamCategoryLabel,
  jamClosedReason,
  jamDate,
  jamDateLong,
  jamWindow,
  JAM_DATE_PENDING,
  type JamCategory,
} from './jam';

describe('jam config', () => {
  test('the copy covers exactly the categories the worker accepts', () => {
    expect(JAM_CATEGORIES.map(category => category.id)).toEqual([...JAM_CATEGORY_IDS]);
  });

  test('a known category renders its copy', () => {
    expect(jamCategoryLabel('launcher')).toBe('best launcher');
  });

  test('a category with no copy still renders as itself rather than blank', () => {
    expect(jamCategoryLabel('nonesuch' as JamCategory)).toBe('nonesuch');
  });

  test('every category says something past its own label', () => {
    for (const category of JAM_CATEGORIES) {
      expect(category.detail.trim()).not.toBe('');
      expect(category.detail).not.toBe(category.label);
    }
  });

  test('the pool is the sum of the cards, so the hero cannot drift from the prizes', () => {
    expect(JAM_PRIZE_POOL).toBe(
      JAM_CATEGORIES.reduce((total, category) => total + category.first + (category.second ?? 0), 0),
    );
  });

  test('a category with no runner-up prints one number rather than a dangling slash', () => {
    expect(jamPrizeLabel({ ...JAM_CATEGORIES[0]!, first: 50, second: 10 })).toBe('$50 / $10');
    expect(jamPrizeLabel({ ...JAM_CATEGORIES[0]!, first: 10, second: null })).toBe('$10');
  });
});

describe('the panel', () => {
  test('handles are bare, so the github urls they build are not @-prefixed', () => {
    for (const handle of JAM_PANEL) {
      expect(handle.startsWith('@')).toBe(false);
    }
  });

  test('each handle resolves to a profile and an avatar on github', () => {
    expect(githubProfile('ItsRiprod')).toBe('https://github.com/ItsRiprod');
    expect(githubAvatar('ItsRiprod')).toBe('https://github.com/ItsRiprod.png?size=160');
    expect(githubAvatar('ItsRiprod', 80)).toBe('https://github.com/ItsRiprod.png?size=80');
  });
});

describe('jamDate', () => {
  test('an unset date reads as pending rather than as an invalid date', () => {
    expect(jamDate(null)).toBe(JAM_DATE_PENDING);
    expect(jamDate('not a date')).toBe(JAM_DATE_PENDING);
  });

  test('a set date renders in utc so the copy does not shift by timezone', () => {
    expect(jamDate('2026-09-01T00:00:00.000Z')).toBe('Sep 1, 2026');
  });

  test('the close offset reads the deadline as its anywhere-on-earth date', () => {
    expect(jamDate(JAM_TIMELINE.closesAt, JAM_CLOSE_OFFSET_MS)).toBe('Sep 13, 2026');
    expect(jamDateLong(JAM_TIMELINE.closesAt, JAM_CLOSE_OFFSET_MS)).toBe('September 13th, 2026');
  });

  test('an offset does not turn an unset or unparseable date into a real one', () => {
    expect(jamDate(null, JAM_CLOSE_OFFSET_MS)).toBe(JAM_DATE_PENDING);
    expect(jamDateLong('not a date', JAM_CLOSE_OFFSET_MS)).toBe(JAM_DATE_PENDING);
  });
});

describe('jamWindow', () => {
  const timeline = {
    opensAt: '2026-09-01T00:00:00.000Z',
    closesAt: '2026-09-15T00:00:00.000Z',
    resultsAt: '2026-09-22T00:00:00.000Z',
  };

  test('the shipped jam stays open through midnight in the last timezone on earth', () => {
    expect(jamWindow(JAM_TIMELINE, new Date('2026-09-14T11:59:59.000Z'))).toEqual({ open: true });
    expect(jamWindow(JAM_TIMELINE, new Date('2026-09-14T12:00:00.000Z'))).toEqual({ open: false, reason: 'after' });
  });

  test('it is shut before the open date and after the close date', () => {
    expect(jamWindow(timeline, new Date('2026-08-31T23:59:59.000Z'))).toEqual({ open: false, reason: 'before' });
    expect(jamWindow(timeline, new Date('2026-09-15T00:00:01.000Z'))).toEqual({ open: false, reason: 'after' });
  });

  test('it is open on the boundaries and in between', () => {
    expect(jamWindow(timeline, new Date('2026-09-01T00:00:00.000Z'))).toEqual({ open: true });
    expect(jamWindow(timeline, new Date('2026-09-08T00:00:00.000Z'))).toEqual({ open: true });
    expect(jamWindow(timeline, new Date('2026-09-15T00:00:00.000Z'))).toEqual({ open: true });
  });

  test('a null half of the window does not close the other half', () => {
    const closingOnly = { opensAt: null, closesAt: '2026-09-15T00:00:00.000Z', resultsAt: null };
    const openingOnly = { opensAt: '2026-09-01T00:00:00.000Z', closesAt: null, resultsAt: null };

    expect(jamWindow(closingOnly, new Date('2020-01-01T00:00:00.000Z'))).toEqual({ open: true });
    expect(jamWindow(openingOnly, new Date('2030-01-01T00:00:00.000Z'))).toEqual({ open: true });
  });

  test('results day alone never closes submissions', () => {
    const resultsOnly = { opensAt: null, closesAt: null, resultsAt: '2020-01-01T00:00:00.000Z' };
    expect(jamWindow(resultsOnly, new Date('2030-01-01T00:00:00.000Z'))).toEqual({ open: true });
  });

  test('an unparseable date is ignored rather than shutting the jam', () => {
    expect(jamWindow({ opensAt: 'soon', closesAt: 'later', resultsAt: null })).toEqual({ open: true });
  });

  test('the closed copy names the open date in utc and the close date anywhere on earth', () => {
    expect(jamClosedReason(timeline, { open: false, reason: 'before' })).toContain('Sep 1, 2026');
    expect(jamClosedReason(timeline, { open: false, reason: 'after' })).toContain('Sep 14, 2026');
  });
});

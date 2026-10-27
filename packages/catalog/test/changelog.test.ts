import { describe, expect, test } from 'bun:test';
import { changelogText } from '../src/changelog.ts';

describe('changelogText', () => {
  test('unwraps hard-wrapped paragraphs and keeps paragraph breaks', () => {
    expect(changelogText('A clock in four faces,\nand the colour is a setting.\n\nThree more things.')).toBe(
      'A clock in four faces, and the colour is a setting.\n\nThree more things.',
    );
  });

  test('turns list items into bullets and folds their continuation lines in', () => {
    expect(
      changelogText('Nine new styles:\n- **Speedometer**: one big dial,\n  the needle sweeping.\n- **Sheet music**: a score.'),
    ).toBe('Nine new styles:\n• Speedometer: one big dial, the needle sweeping.\n• Sheet music: a score.');
  });

  test('keeps ordered list numbers', () => {
    expect(changelogText('1. first\n2) second')).toBe('1. first\n2) second');
  });

  test('strips emphasis, code, links, images, headings, rules and html', () => {
    expect(
      changelogText('## Fixed\n\n*Faster* `boot`, ~~old~~ [docs](https://x.y) ![shot](a.png) <b>now</b>\n\n---\n\nend'),
    ).toBe('Fixed\n\nFaster boot, old docs shot now\n\nend');
  });

  test('leaves snake_case and lone asterisks alone', () => {
    expect(changelogText('set max_volume_level to 5 * 2')).toBe('set max_volume_level to 5 * 2');
  });

  test('an empty or missing changelog is null', () => {
    expect(changelogText(null)).toBeNull();
    expect(changelogText('  \n\n ')).toBeNull();
  });
});

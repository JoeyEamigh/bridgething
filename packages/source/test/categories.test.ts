import { CATEGORIES } from '@bridgething/catalog';
import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const template = resolve(import.meta.dir, '..', '..', 'create-bridgething');

const GUIDES = ['template-source/CLAUDE.md', 'template/_claude/skills/bridgething/reference/ship.md'];

describe('agent guides', () => {
  for (const guide of GUIDES) {
    test(`${guide} lists the category vocabulary in order`, () => {
      const text = readFileSync(resolve(template, guide), 'utf8').replace(/\s+/g, ' ');
      expect(text).toContain(CATEGORIES.map(slug => `\`${slug}\``).join(', '));
    });
  }
});

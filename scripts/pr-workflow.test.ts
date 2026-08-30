import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join, posix } from 'node:path';
import { parse } from 'yaml';
import { manifests, members, ROOT, type Member } from './ts-workspace';

const TS_TASKS = ['typecheck', 'test'];

type Step = { uses?: string; run?: string; with?: Record<string, string> };
type Workflow = { jobs: Record<string, { steps?: Step[] }> };

const workflow = parse(readFileSync(join(ROOT, '.github/workflows/pr.yml'), 'utf8')) as Workflow;

function steps(job: string): Step[] {
  const found = workflow.jobs[job]?.steps;
  if (!found) throw new Error(`pr.yml has no ${job} job with steps`);
  return found;
}

function filters(): Record<string, string[]> {
  const step = steps('scope').find(entry => entry.uses?.startsWith('dorny/paths-filter'));
  const source = step?.with?.['filters'];
  if (!source) throw new Error('pr.yml scope job no longer runs dorny/paths-filter with a filters block');
  const parsed = parse(source) as Record<string, (string | string[])[]>;
  return Object.fromEntries(Object.entries(parsed).map(([name, entries]) => [name, entries.flat()]));
}

function tsMembers(): Member[] {
  return members().filter(member => TS_TASKS.some(task => member.scripts[task] !== undefined));
}

function probes(member: Member): string[] {
  return [`${member.dir}/package.json`, ...member.reads.map(file => posix.join(member.dir, file))];
}

function matcher(patterns: string[]): (path: string) => boolean {
  const globs = patterns.map(pattern => new Bun.Glob(pattern));
  return path => globs.some(glob => glob.match(path));
}

describe('the pr gate routes a diff to the job that can fail on it', () => {
  const scopes = filters();

  test('the ts filter covers every file the ts job typechecks', () => {
    expect(scopes['ts']).toBeDefined();
    const matches = matcher(scopes['ts'] ?? []);
    const uncovered: string[] = [];
    for (const member of tsMembers()) {
      for (const probe of probes(member)) {
        if (!matches(probe)) uncovered.push(`${member.name}: ${probe}`);
      }
    }
    expect(uncovered).toEqual([]);
  });

  test('every member the ts job runs is checked against a read set, not an empty one', () => {
    const blind = tsMembers()
      .filter(member => member.reads.length === 0)
      .map(member => member.name);
    expect(blind).toEqual([]);
  });

  test('the ts filter reruns the manifest-derived gate for every workspace manifest', () => {
    const matches = matcher(scopes['ts'] ?? []);
    expect(manifests().filter(path => !matches(path))).toEqual([]);
  });

  test('the ts filter reruns the gate for the roots that decide what turbo builds', () => {
    const matches = matcher(scopes['ts'] ?? []);
    const roots = ['package.json', 'bun.lock', 'turbo.json', 'scripts/pr-workflow.test.ts', 'scripts/ts-workspace.ts'];
    expect(roots.filter(path => !matches(path))).toEqual([]);
  });

  test('every filter splices the shared anchor, so a change to the gate itself reruns the gate', () => {
    for (const [name, patterns] of Object.entries(scopes)) {
      if (name === 'shared') continue;
      expect({ name, has: patterns.includes('.github/workflows/pr.yml') }).toEqual({ name, has: true });
    }
  });

  test('the ts job runs the turbo suites and the script guards', () => {
    const commands = steps('ts')
      .map(step => step.run ?? '')
      .join('\n');
    expect(commands).toContain('turbo run typecheck test --affected');
    expect(commands).toContain('bun test ./scripts');
  });
});

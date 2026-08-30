import { describe, expect, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { hashed, members, READ_BY_TSC, ROOT } from './ts-workspace';

const TSC_TASKS = ['typecheck', 'lint'];
const PLANNED_TASKS = ['build', 'typecheck', 'test', 'lint'];

type PlannedTask = {
  taskId: string;
  package: string;
  task: string;
  command: string;
  dependents: string[];
  inputs: Record<string, string>;
};

function plan(): PlannedTask[] {
  const binary = join(ROOT, 'node_modules/.bin/turbo');
  const result = spawnSync(binary, ['run', ...PLANNED_TASKS, '--dry=json'], {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: 512 * 1024 * 1024,
  });
  if (result.status !== 0) throw new Error(`turbo --dry=json failed: ${result.stderr}`);
  return (JSON.parse(result.stdout) as { tasks: PlannedTask[] }).tasks;
}

const planned = plan();
const byId = new Map(planned.map(task => [task.taskId, task]));
const scanned = members();

const reachesWork = new Map<string, boolean>();
function matters(id: string): boolean {
  const cached = reachesWork.get(id);
  if (cached !== undefined) return cached;
  const task = byId.get(id);
  if (!task) return false;
  reachesWork.set(id, false);
  const answer = task.command !== '<NONEXISTENT>' || task.dependents.some(matters);
  reachesWork.set(id, answer);
  return answer;
}

const typechecked = scanned.filter(member => member.scripts['typecheck'] !== undefined);

describe('turbo hashes everything tsc reads', () => {
  test('the workspace scan reaches the packages that typecheck their whole tree', () => {
    const names = typechecked.map(member => member.name);
    expect(names).toContain('@bridgething/site');
    expect(names).toContain('@bridgething/mobile');
    expect(names).toContain('@bridgething/client');
    expect(names).toContain('@bridgething/extension');
    expect(names).toContain('create-bridgething');
  });

  test('site typecheck resolves the worker project its script names with -p', () => {
    const site = typechecked.find(member => member.name === '@bridgething/site');
    expect(site?.reads).toContain('worker/tsconfig.json');
    expect(site?.reads.some(file => file.startsWith('worker/') && file.endsWith('.ts'))).toBe(true);
  });

  test('mobile typecheck resolves the sources its ** include pulls in', () => {
    const mobile = typechecked.find(member => member.name === '@bridgething/mobile');
    expect(mobile?.reads.some(file => file.startsWith('lib/'))).toBe(true);
    expect(mobile?.reads.some(file => file.startsWith('components/'))).toBe(true);
  });

  for (const task of TSC_TASKS) {
    test(`every planned ${task} hashes every file tsc reads`, () => {
      const missing: string[] = [];
      for (const member of typechecked) {
        if (member.scripts[task] === undefined) continue;
        const id = `${member.name}#${task}`;
        const found = byId.get(id);
        if (!found) {
          missing.push(`${id}: turbo does not plan this task`);
          continue;
        }
        const inputs = new Set(Object.keys(found.inputs));
        for (const file of member.reads) {
          if (!inputs.has(file)) missing.push(`${id}: ${file}`);
        }
      }
      expect(missing).toEqual([]);
    });
  }
});

describe('the guard reads the tree, not just the workspace roots', () => {
  test('every member scan reaches source files, not just the tsconfig that names them', () => {
    const empty = typechecked
      .filter(member => !member.reads.some(file => READ_BY_TSC.some(extension => file.endsWith(extension))))
      .map(member => member.name);
    expect(empty).toEqual([]);
  });
});

describe('a rust edit does not invalidate a typescript hash', () => {
  test('no hash the ts lane depends on carries rust out of its own package', () => {
    const leaked: string[] = [];
    for (const task of planned) {
      if (!matters(task.taskId)) continue;
      for (const file of Object.keys(task.inputs)) {
        if (file.endsWith('.rs') && !file.startsWith('../')) leaked.push(`${task.taskId}: ${file}`);
      }
    }
    expect(leaked).toEqual([]);
  });

  test('the scriptless graph nodes that gate real work are the ones the leak check looks at', () => {
    const gates = planned.filter(task => task.command === '<NONEXISTENT>' && matters(task.taskId));
    expect(gates.map(task => task.taskId).sort()).toEqual([
      '@bridgething/companion-types#build',
      '@bridgething/core-node#build',
      '@bridgething/ui#build',
    ]);
  });

  test('the plan covers the members that carry rust beside their typescript', () => {
    const rusty = scanned.filter(member => hashed(member.dir).some(file => file.endsWith('.rs')));
    const names = rusty.map(member => member.name);
    expect(names).toContain('@bridgething/lib');
    expect(names).toContain('@bridgething/companion-types');
    expect(names).toContain('@bridgething/core-node');
    expect(names).toContain('@bridgething/desktop-frontend');
    expect(names.filter(name => !planned.some(task => task.package === name))).toEqual([]);
  });
});

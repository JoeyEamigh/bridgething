import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import type { RefObject } from 'react';
import type { Root } from 'react-dom/client';

import type { ConfigFormHandle } from '../src/form.js';
import type { ConfigField, SettingsContext, SettingsHost } from '../src/settings-protocol.js';

import { bridge, page, resetBridge, session } from './host.js';

const { act, createRef } = await import('react');
const { createRoot } = await import('react-dom/client');
const { ConfigForm, ConfigFormError, closeSettings } = await import('../src/form.js');
const { settings } = await import('../src/settings.js');

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const FIELDS: ConfigField[] = [
  { type: 'string', data: { key: 'zip', label: 'ZIP code', pattern: '[0-9]{5}', minLength: 5, maxLength: 5 } },
  { type: 'secret', data: { key: 'token', label: 'API token' } },
  { type: 'number', data: { key: 'refresh', label: 'refresh minutes', min: 1, max: 60, step: 1, default: 15 } },
  { type: 'boolean', data: { key: 'metric', label: 'metric units', default: false } },
  { type: 'enum', data: { key: 'theme', label: 'theme', choices: ['dark', 'light'], default: 'dark' } },
] as ConfigField[];

type Memory = { fields: ConfigField[]; stored: Map<string, string>; writes: Array<[string, string | null]> };

function contextFor(webappId: string): SettingsContext {
  return { webappId, name: webappId, version: '1.0.0', deviceId: 'd' };
}

function memoryHost(memory: Memory, failure?: string, context = contextFor('a')): SettingsHost {
  const unused = () => Promise.reject(new Error('unused in this test'));
  return {
    context: () => Promise.resolve(context),
    'config.fields': () => (failure ? Promise.reject(new Error(failure)) : Promise.resolve(memory.fields)),
    'config.list': () => Promise.resolve([...memory.stored].map(([key, value]) => ({ key, value }))),
    'config.set': ({ key, value }) => {
      memory.stored.set(key, value);
      memory.writes.push([key, value]);
      return Promise.resolve({ key, value });
    },
    'config.delete': ({ key }) => {
      memory.stored.delete(key);
      memory.writes.push([key, null]);
      return Promise.resolve({ key, value: null });
    },
    'doc.get': unused,
    'doc.list': unused,
    'doc.set': unused,
    'doc.delete': unused,
    fetch: unused,
    'auth.authorize': unused,
    pick: unused,
    'blob.create': unused,
    'blob.append': unused,
    'blob.put': unused,
    'blob.delete': unused,
    'blob.list': unused,
    done: () => {},
  };
}

type Gate<T> = { resolve: (value: T) => void; reject: (err: Error) => void };

function gated<T>(gates: Array<Gate<T>>): Promise<T> {
  return new Promise<T>((resolve, reject) => gates.push({ resolve, reject }));
}

let nextElsewhere = 0;

function elsewhere(): { target: string; memory: Memory } {
  const target = `elsewhere-${(nextElsewhere += 1)}`;
  const other: Memory = {
    fields: [{ type: 'string', data: { key: 'home', label: 'home page' } }] as ConfigField[],
    stored: new Map(),
    writes: [],
  };
  bridge.hosts[target] = memoryHost(other, undefined, contextFor(target));
  return { target, memory: other };
}

let root: Root;
let container: HTMLElement;
let memory: Memory;
let reported: Array<Record<string, string>>;
let form: RefObject<ConfigFormHandle | null>;

async function run(work: () => void): Promise<void> {
  await act(() => {
    work();
    return Promise.resolve();
  });
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i += 1) {
    await act(() => new Promise<void>(resolve => setTimeout(resolve, 0)));
  }
}

async function mount(fields: ConfigField[], stored: Record<string, string> = {}, failure?: string): Promise<void> {
  memory = { fields, stored: new Map(Object.entries(stored)), writes: [] };
  bridge.host = memoryHost(memory, failure);
  await settings.context();
  await run(() => {
    root.render(<ConfigForm ref={form} onChange={values => reported.push(values)} />);
  });
  await settle();
}

function flushed(): Promise<Record<string, string> | Error> {
  return form.current!.flush().catch((err: unknown) => err as Error);
}

function input(key: string): HTMLInputElement {
  const found = container.querySelector<HTMLInputElement>(`#bt-field-${key}`);
  if (!found) throw new Error(`no control for ${key}`);
  return found;
}

function text(): string {
  return container.textContent ?? '';
}

async function enter(key: string, value: string): Promise<void> {
  const field = input(key);
  await run(() => {
    Reflect.set(Object.getPrototypeOf(field) as object, 'value', value, field);
    field.dispatchEvent(new page.Event('input', { bubbles: true }) as unknown as Event);
  });
}

async function type(key: string, value: string): Promise<void> {
  await enter(key, value);
  await run(() => {
    input(key).dispatchEvent(new page.FocusEvent('focusout', { bubbles: true }) as unknown as Event);
  });
  await settle();
}

function choice(key: string, label: string): Element {
  const found = [...container.querySelectorAll(`#bt-field-${key} button`)].find(button => button.textContent === label);
  if (!found) throw new Error(`no ${label} choice for ${key}`);
  return found;
}

function retry(): Element {
  const found = container.querySelector('.bt-retry');
  if (!found) throw new Error('the form offers no retry');
  return found;
}

async function click(element: Element): Promise<void> {
  await run(() => {
    element.dispatchEvent(new page.MouseEvent('click', { bubbles: true }) as unknown as Event);
  });
  await settle();
}

beforeEach(() => {
  resetBridge();
  reported = [];
  form = createRef<ConfigFormHandle>();
  container = page.document.createElement('div') as unknown as HTMLElement;
  page.document.body.appendChild(container as never);
  root = createRoot(container);
});

afterEach(async () => {
  await run(() => {
    root.unmount();
  });
  container.remove();
});

describe('ConfigForm', () => {
  test('renders a control per field kind from the wire shape', async () => {
    await mount(FIELDS, { metric: 'true' });

    const zip = input('zip');
    expect(zip.type).toBe('text');
    expect(zip.getAttribute('pattern')).toBe('[0-9]{5}');
    expect(zip.getAttribute('maxlength')).toBe('5');
    expect(input('token').type).toBe('password');

    const refresh = input('refresh');
    expect(refresh.type).toBe('number');
    expect(refresh.value).toBe('15');
    expect(refresh.getAttribute('min')).toBe('1');
    expect(refresh.getAttribute('max')).toBe('60');

    expect(input('metric').type).toBe('checkbox');
    expect(input('metric').checked).toBe(true);

    const pressed = [...container.querySelectorAll('#bt-field-theme button')].map(button => [
      button.textContent,
      button.getAttribute('aria-pressed'),
    ]);
    expect(pressed).toEqual([
      ['dark', 'true'],
      ['light', 'false'],
    ]);

    expect(text()).toContain('ZIP code');
    expect(text()).toContain('at least 5 characters / at most 5 characters');
    expect(text()).toContain('min 1 / max 60 / step 1');
  });

  test('says so when the app declares no settings', async () => {
    await mount([]);
    expect(text()).toContain('no settings');
  });

  test('shows a failed load instead of a form', async () => {
    await mount(FIELDS, {}, 'the car thing went away');
    expect(text()).toContain('the car thing went away');
    expect(container.querySelector('input')).toBeNull();
  });

  test('retries a failed load from its own button', async () => {
    await mount(FIELDS, { refresh: '30' }, 'the car thing went away');
    bridge.host = memoryHost(memory);
    await click(retry());

    expect(text()).not.toContain('the car thing went away');
    expect(input('refresh').value).toBe('30');
    expect(container.querySelector('.bt-retry')).toBeNull();
  });

  test('retries a failed load when the host shows the page again', async () => {
    await mount(FIELDS, {}, 'the car thing went away');
    bridge.host = memoryHost(memory);
    await run(() => session.notify({ event: 'refresh', context: contextFor('app') }));
    await settle();

    expect(text()).toContain('ZIP code');
    expect(text()).not.toContain('the car thing went away');
  });

  test('picks up new fields and values on a refresh without dropping what is being typed', async () => {
    await mount([FIELDS[0], FIELDS[2]], { refresh: '30' });
    await enter('zip', '482');

    memory.fields = [...memory.fields, { type: 'string', data: { key: 'units', label: 'units' } } as ConfigField];
    memory.stored.set('refresh', '45');
    memory.stored.set('units', 'metric');
    await run(() => session.notify({ event: 'refresh', context: contextFor('app') }));
    await settle();

    expect(input('units').value).toBe('metric');
    expect(input('refresh').value).toBe('45');
    expect(input('zip').value).toBe('482');
    expect(reported.at(-1)).toEqual({ refresh: '45', units: 'metric' });
    expect(memory.writes).toEqual([]);
  });

  test('keeps the form and says so when a refresh cannot reach the device', async () => {
    await mount(FIELDS, { refresh: '30' });
    bridge.host = { ...bridge.host!, 'config.list': () => Promise.reject(new Error('the link dropped')) };
    await run(() => session.notify({ event: 'refresh', context: contextFor('app') }));
    await settle();

    expect(input('refresh').value).toBe('30');
    expect(text()).toContain('the link dropped');

    bridge.host = memoryHost(memory);
    await click(retry());

    expect(text()).not.toContain('the link dropped');
    expect(container.querySelector('.bt-retry')).toBeNull();
  });

  test('commits a text field on blur and reports the effective values', async () => {
    await mount(FIELDS);
    await type('zip', '48201');

    expect(memory.writes).toEqual([['zip', '48201']]);
    expect(input('zip').value).toBe('48201');
    expect(reported.at(-1)).toEqual({ zip: '48201', refresh: '15', metric: 'false', theme: 'dark' });
  });

  test('holds back a value the field constraints reject', async () => {
    await mount(FIELDS);
    await type('zip', 'abc');

    expect(memory.writes).toEqual([]);
    expect(input('zip').getAttribute('aria-invalid')).toBe('true');
  });

  test('clears the complaint when the original value is typed back', async () => {
    await mount(FIELDS, { zip: '48201' });
    await type('zip', 'abc');
    expect(input('zip').getAttribute('aria-invalid')).toBe('true');

    await type('zip', '48201');

    expect(input('zip').getAttribute('aria-invalid')).toBe('false');
    expect(container.querySelector('.bt-note[data-tone=err]')).toBeNull();
    expect(memory.writes).toEqual([]);
  });

  test('commits a number inside its bounds and holds back one outside them', async () => {
    await mount(FIELDS);
    await type('refresh', '30');
    await type('refresh', '99');

    expect(memory.writes).toEqual([['refresh', '30']]);
    expect(input('refresh').getAttribute('aria-invalid')).toBe('true');
    expect(reported.at(-1)?.refresh).toBe('30');
  });

  test('flips a boolean and picks an enum choice', async () => {
    await mount(FIELDS);
    await click(input('metric'));
    await click([...container.querySelectorAll('#bt-field-theme button')][1]);

    expect(memory.writes).toEqual([
      ['metric', 'true'],
      ['theme', 'light'],
    ]);
  });

  test('reset deletes the stored value and falls back to the default', async () => {
    await mount(FIELDS, { refresh: '30' });
    expect(input('refresh').value).toBe('30');

    const row = input('refresh').closest('.bt-field')!;
    await click(row.querySelector('.bt-reset')!);

    expect(memory.writes).toEqual([['refresh', null]]);
    expect(input('refresh').value).toBe('15');
  });

  test('reports a committed value before the host acknowledges it', async () => {
    await mount(FIELDS);
    bridge.host = { ...bridge.host!, 'config.set': () => new Promise(() => {}) };
    await type('token', 's3cret');

    expect(reported.at(-1)?.token).toBe('s3cret');
  });

  test('rolls a refused write back and shows why', async () => {
    await mount(FIELDS, { token: 'old' });
    bridge.host = { ...bridge.host!, 'config.set': () => Promise.reject(new Error('the device said no')) };
    await type('token', 'new');

    expect(input('token').value).toBe('old');
    expect(reported.at(-1)?.token).toBe('old');
    expect(text()).toContain('the device said no');
  });

  test('ends on what the device holds when two writes to one field both fail', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    bridge.host = { ...bridge.host!, 'config.set': () => gated(sets) };

    await click(input('metric'));
    await click(input('metric'));
    await run(() => sets[0].reject(new Error('timed out')));
    await settle();
    await run(() => sets[1].reject(new Error('timed out')));
    await settle();

    expect(sets).toHaveLength(2);
    expect(input('metric').checked).toBe(false);
    expect(reported.at(-1)?.metric).toBe('false');
    expect(text()).toContain('timed out');
  });

  test('shows the later write when an earlier one to the same field fails', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    const fields = [
      { type: 'enum', data: { key: 'theme', label: 'theme', choices: ['dark', 'light', 'auto'], default: 'dark' } },
    ] as ConfigField[];
    await mount(fields);
    const real = bridge.host!;
    bridge.host = {
      ...real,
      'config.set': payload => gated(sets).then(() => real['config.set'](payload, () => undefined)),
    };

    await click(choice('theme', 'light'));
    await click(choice('theme', 'auto'));
    await run(() => sets[0].reject(new Error('the device said no')));
    await settle();
    await run(() => sets[1].resolve({ key: 'theme', value: 'auto' }));
    await settle();

    expect(memory.stored.get('theme')).toBe('auto');
    expect(choice('theme', 'auto').getAttribute('aria-pressed')).toBe('true');
    expect(reported.at(-1)).toEqual({ theme: 'auto' });
    expect(text()).not.toContain('the device said no');
  });

  test('sends only the last of several edits made while a write is in flight', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    const asked: string[] = [];
    await mount(FIELDS);
    bridge.host = {
      ...bridge.host!,
      'config.set': ({ value }) => {
        asked.push(value);
        return gated(sets);
      },
    };

    await type('zip', '10001');
    await type('zip', '20002');
    await type('zip', '30003');
    await run(() => sets[0].resolve({ key: 'zip', value: '10001' }));
    await settle();
    await run(() => sets[1].resolve({ key: 'zip', value: '30003' }));
    await settle();

    expect(asked).toEqual(['10001', '30003']);
    expect(input('zip').value).toBe('30003');
  });

  test('a reset in flight does not undo an edit made to another field meanwhile', async () => {
    const deletes: Array<Gate<{ key: string; value: string | null }>> = [];
    await mount(FIELDS, { refresh: '30' });
    const real = bridge.host!;
    bridge.host = {
      ...real,
      'config.delete': payload => gated(deletes).then(() => real['config.delete'](payload, () => undefined)),
    };

    await click(input('refresh').closest('.bt-field')!.querySelector('.bt-reset')!);
    await type('token', 's3cret');
    expect(input('token').value).toBe('s3cret');
    await run(() => deletes[0].resolve({ key: 'refresh', value: null }));
    await settle();

    expect(memory.writes).toEqual([
      ['refresh', null],
      ['token', 's3cret'],
    ]);
    expect(input('refresh').value).toBe('15');
    expect(input('token').value).toBe('s3cret');
    expect(reported.at(-1)).toMatchObject({ refresh: '15', token: 's3cret' });
  });

  test('drops the complaint when a write that timed out turns out to have landed', async () => {
    await mount(FIELDS);
    const real = bridge.host!;
    bridge.host = {
      ...real,
      'config.set': async payload => {
        await real['config.set'](payload, () => undefined);
        throw new Error('timed out');
      },
    };

    await type('zip', '48201');

    expect(input('zip').value).toBe('48201');
    expect(reported.at(-1)?.zip).toBe('48201');
    expect(text()).not.toContain('timed out');
  });

  test('leaves what is being typed out of the reported values', async () => {
    await mount(FIELDS);
    await enter('token', 's3cret');

    expect(input('token').value).toBe('s3cret');
    expect(reported.at(-1)?.token).toBeUndefined();
    expect(memory.writes).toEqual([]);
  });

  test('flush writes a field that never lost focus and resolves with what the device holds', async () => {
    await mount(FIELDS, { refresh: '30' });
    await enter('token', 's3cret');
    await enter('zip', '48201');

    let values: Record<string, string> | Error = {};
    await run(() => void flushed().then(result => (values = result)));
    await settle();

    expect(memory.writes).toEqual([
      ['token', 's3cret'],
      ['zip', '48201'],
    ]);
    expect(values).toEqual({ zip: '48201', token: 's3cret', refresh: '30', metric: 'false', theme: 'dark' });
  });

  test('flush waits for a write that is already on its way', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    const real = bridge.host!;
    bridge.host = {
      ...real,
      'config.set': payload => gated(sets).then(() => real['config.set'](payload, () => undefined)),
    };
    await type('zip', '48201');

    let values: Record<string, string> | Error | null = null;
    await run(() => void flushed().then(result => (values = result)));
    await settle();
    expect(values).toBeNull();

    await run(() => sets[0].resolve({ key: 'zip', value: '48201' }));
    await settle();

    expect(values).toMatchObject({ zip: '48201' });
  });

  test('flush refuses by field when what was typed is not allowed', async () => {
    await mount(FIELDS);
    await enter('zip', 'abc');

    let outcome: Record<string, string> | Error = {};
    await run(() => void flushed().then(result => (outcome = result)));
    await settle();

    expect(outcome).toBeInstanceOf(ConfigFormError);
    const failure = outcome as unknown as InstanceType<typeof ConfigFormError>;
    expect(failure.kind).toBe('refused');
    expect(Object.keys(failure.fields)).toEqual(['zip']);
    expect(failure.message).toStartWith('ZIP code: ');
    expect(memory.writes).toEqual([]);
    expect(input('zip').getAttribute('aria-invalid')).toBe('true');
  });

  test('flush refuses by field when the device turns the write down', async () => {
    await mount(FIELDS, { token: 'old' });
    bridge.host = { ...bridge.host!, 'config.set': () => Promise.reject(new Error('the device said no')) };
    await enter('token', 'new');

    let outcome: Record<string, string> | Error = {};
    await run(() => void flushed().then(result => (outcome = result)));
    await settle();

    const failure = outcome as unknown as InstanceType<typeof ConfigFormError>;
    expect(failure).toBeInstanceOf(ConfigFormError);
    expect(failure.kind).toBe('refused');
    expect(failure.fields).toEqual({ token: 'the device said no' });
    expect(input('token').value).toBe('old');
  });

  test('flush says the form is unavailable when it never loaded', async () => {
    await mount(FIELDS, {}, 'the car thing went away');

    const failure = (await flushed()) as InstanceType<typeof ConfigFormError>;

    expect(failure).toBeInstanceOf(ConfigFormError);
    expect(failure.kind).toBe('unavailable');
    expect(failure.message).toBe('the car thing went away');
  });

  test('reloads for the new app on a context change and drops late replies for the old app', async () => {
    let release: (entries: Array<{ key: string; value: string }>) => void = () => {};
    await mount([{ type: 'string', data: { key: 'zip', label: 'ZIP code' } }] as ConfigField[]);
    expect(text()).toContain('ZIP code');

    bridge.host = {
      ...bridge.host!,
      'config.list': () => new Promise(resolve => (release = resolve)),
    };
    await run(() => session.retarget('a', contextFor('a')));
    await settle();
    const stale = release;

    memory = {
      fields: [{ type: 'secret', data: { key: 'token', label: 'API token' } }] as ConfigField[],
      stored: new Map(),
      writes: [],
    };
    bridge.host = memoryHost(memory);
    await run(() => session.retarget('b', contextFor('b')));
    await settle();
    await run(() => stale([{ key: 'zip', value: '48201' }]));
    await settle();

    expect(text()).toContain('API token');
    expect(text()).not.toContain('ZIP code');
    expect(reported.at(-1)).toEqual({});
  });

  test('writes an edit still queued at a retarget to the app it was made for', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    const asked: Array<[string, string]> = [];
    await mount(FIELDS);
    bridge.host = {
      ...bridge.host!,
      'config.set': ({ key, value }) => {
        asked.push([key, value]);
        return gated(sets);
      },
    };
    const other = elsewhere();
    await type('zip', '48201');
    await type('token', 's3cret');

    let outcome: Record<string, string> | Error = {};
    await run(() => void flushed().then(result => (outcome = result)));
    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();
    expect(text()).toContain('home page');
    await run(() => sets[0].resolve({ key: 'zip', value: '48201' }));
    await settle();
    await run(() => sets[1].resolve({ key: 'token', value: 's3cret' }));
    await settle();

    expect(asked).toEqual([
      ['zip', '48201'],
      ['token', 's3cret'],
    ]);
    expect(other.memory.writes).toEqual([]);
    expect(text()).toContain('home page');
    expect(text()).not.toContain('ZIP code');
    expect(outcome).toBeInstanceOf(ConfigFormError);
    expect((outcome as unknown as InstanceType<typeof ConfigFormError>).kind).toBe('replaced');
  });

  test('writes what is still being typed at a retarget to the app it was typed for', async () => {
    await mount(FIELDS);
    const other = elsewhere();
    await enter('zip', '48201');
    await enter('refresh', '99');

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();

    expect(memory.writes).toEqual([['zip', '48201']]);
    expect(other.memory.writes).toEqual([]);
    expect(text()).toContain('refresh minutes was not saved to the previous app: ');
  });

  test('says so on the new form when an edit for the previous app is refused after the retarget', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    bridge.host = { ...bridge.host!, 'config.set': () => gated(sets) };
    const other = elsewhere();
    await type('zip', '48201');

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await run(() => sets[0].reject(new Error('the device said no')));
    await settle();

    expect(text()).toContain('ZIP code was not saved to the previous app: the device said no');
    expect(text()).toContain('home page');
    expect(other.memory.writes).toEqual([]);
  });

  test('a late reply for the previous app never touches the form for the next one', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    bridge.host = { ...bridge.host!, 'config.set': () => gated(sets) };
    const other = elsewhere();
    await type('zip', '48201');

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();
    expect(text()).toContain('home page');
    await type('home', 'about:blank');
    await run(() => sets[0].resolve({ key: 'zip', value: '48201' }));
    await settle();

    expect(other.memory.writes).toEqual([['home', 'about:blank']]);
    expect(reported.at(-1)).toEqual({ home: 'about:blank' });
    expect(input('home').value).toBe('about:blank');
  });

  test('shows what an edit wrote when the page comes back to its app before the write lands', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    const first = bridge.host!;
    bridge.host = {
      ...first,
      'config.set': payload =>
        gated(sets).then(
          () => first['config.set'](payload, () => undefined) as Promise<{ key: string; value: string }>,
        ),
    };
    const other = elsewhere();
    await type('zip', '48201');

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();
    await run(() => session.retarget(`${other.target}-back`, contextFor('a')));
    await settle();
    expect(text()).toContain('loading settings');
    await run(() => sets[0].resolve({ key: 'zip', value: '48201' }));
    await settle();

    expect(input('zip').value).toBe('48201');
  });

  test('an app the page moves to straight away waits on the queue still writing to that same app', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    const first = bridge.host!;
    bridge.host = {
      ...first,
      'config.set': payload =>
        gated(sets).then(
          () => first['config.set'](payload, () => undefined) as Promise<{ key: string; value: string }>,
        ),
    };
    await type('zip', '48201');

    await run(() => session.retarget('a-again', contextFor('a')));
    await settle();
    expect(text()).toContain('loading settings');
    await run(() => sets[0].resolve({ key: 'zip', value: '48201' }));
    await settle();

    expect(input('zip').value).toBe('48201');
  });

  test('an unrelated app loads at once while a write for the previous app is still stuck', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    bridge.host = { ...bridge.host!, 'config.set': () => gated(sets) };
    const other = elsewhere();
    await type('zip', '48201');
    await type('token', 's3cret');

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();

    expect(text()).toContain('home page');
    expect(await flushed()).toEqual({});
    expect(sets).toHaveLength(1);
  });

  test('lets the host drop the previous app only once its queue has drained', async () => {
    const sets: Array<Gate<{ key: string; value: string }>> = [];
    await mount(FIELDS);
    bridge.host = { ...bridge.host!, 'config.set': () => gated(sets) };
    const other = elsewhere();
    await type('zip', '48201');
    const left = bridge.seen.find(request => request.verb === 'config.set')!.epoch;
    const released = () => bridge.seen.filter(request => request.verb === 'release' && request.epoch === left);

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();
    expect(released()).toEqual([]);

    await run(() => sets[0].resolve({ key: 'zip', value: '48201' }));
    await settle();

    expect(released()).toHaveLength(1);
  });

  test('a retired form never reports the previous app values after the switch', async () => {
    await mount(FIELDS);
    const other = elsewhere();
    await enter('zip', '48201');
    const before = reported.length;

    await run(() => session.retarget(other.target, contextFor(other.target)));
    await settle();

    expect(memory.writes).toEqual([['zip', '48201']]);
    expect(reported.slice(before)).toEqual([{}]);
  });

  describe('closeSettings', () => {
    test('writes what is being typed and then closes', async () => {
      await mount(FIELDS);
      await enter('zip', '48201');

      await run(() => void closeSettings(form.current));
      await settle();

      expect(memory.writes).toEqual([['zip', '48201']]);
      expect(bridge.dones).toBe(1);
    });

    test('closes a page whose form never loaded', async () => {
      await mount(FIELDS, {}, 'the car thing went away');

      await closeSettings(form.current);

      expect(bridge.dones).toBe(1);
    });

    test('closes a page with no form', async () => {
      await closeSettings(null);

      expect(bridge.dones).toBe(1);
    });

    test('stays open and hands back the refusal when the device turns the edit down', async () => {
      await mount(FIELDS);
      bridge.host = { ...bridge.host!, 'config.set': () => Promise.reject(new Error('the device said no')) };
      await enter('token', 'new');

      let outcome: unknown = null;
      await run(() => void closeSettings(form.current).catch((err: unknown) => (outcome = err)));
      await settle();

      expect(outcome).toBeInstanceOf(ConfigFormError);
      expect((outcome as InstanceType<typeof ConfigFormError>).kind).toBe('refused');
      expect(bridge.dones).toBe(0);
    });
  });
});

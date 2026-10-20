import { forwardRef, useEffect, useImperativeHandle, useRef, useState, type KeyboardEvent } from 'react';

import { settings, type ConfigEntry, type ConfigField, type SettingsContext } from './settings.js';

export type ConfigValues = Record<string, string>;
export type ConfigFormProps = { onChange?: (values: ConfigValues) => void };
export type ConfigFormHandle = { flush(): Promise<ConfigValues> };

export type ConfigFormErrorKind = 'unavailable' | 'replaced' | 'refused';

export class ConfigFormError extends Error {
  constructor(
    public readonly kind: ConfigFormErrorKind,
    message: string,
    public readonly fields: Record<string, string> = {},
  ) {
    super(message);
    this.name = 'ConfigFormError';
  }
}

type View =
  | { kind: 'loading' }
  | { kind: 'failed'; reason: string }
  | {
      kind: 'ready';
      fields: ConfigField[];
      values: ConfigValues;
      drafts: ConfigValues;
      errors: Record<string, string>;
      stale: string | null;
    };

type Write = { kind: 'write'; key: string; value: string | null };
type Op = { kind: 'load' } | Write;
type Phase = 'live' | 'retired' | 'gone';

type EngineOptions = {
  after: Promise<void>;
  show: (view: View) => void;
  report: (values: ConfigValues) => void;
  invalid: (key: string) => string | null;
  lost: (label: string, reason: string) => void;
};

type Engine = {
  reload(): void;
  draft(key: string, text: string): void;
  finish(key: string): void;
  commit(key: string, value: string): void;
  reset(key: string): void;
  flush(): Promise<ConfigValues>;
  retire(): Promise<void>;
  dispose(): void;
};

const UNLOADED = 'settings did not load';

function replaced(): ConfigFormError {
  return new ConfigFormError('replaced', 'the form moved to another app');
}

const STYLE = `
.bt-form{display:flex;flex-direction:column;gap:12px;color-scheme:var(--bt-scheme);font-family:var(--bt-font-body);color:var(--bt-color-fg)}
.bt-note{margin:0;font-family:var(--bt-font-mono);font-size:var(--bt-text-hint);color:var(--bt-color-muted)}
.bt-note[data-tone=err]{color:var(--bt-color-err)}
.bt-field{display:flex;flex-direction:column;gap:8px;padding:12px 16px;border:1px solid var(--bt-color-rule);background:var(--bt-color-screen)}
.bt-head{display:flex;align-items:center;justify-content:space-between;gap:12px}
.bt-label{flex:1;min-width:0;font-family:var(--bt-font-mono);font-size:var(--bt-text-eyebrow);letter-spacing:.18em;text-transform:uppercase;color:var(--bt-color-muted)}
.bt-reset,.bt-retry{flex-shrink:0;padding:0;border:0;border-radius:0;background:none;cursor:pointer;font-family:var(--bt-font-mono);font-size:var(--bt-text-eyebrow);letter-spacing:.18em;text-transform:uppercase;color:var(--bt-color-accent)}
.bt-retry{align-self:flex-start}
.bt-input{box-sizing:border-box;width:100%;padding:10px 12px;border:1px solid var(--bt-color-edge);border-radius:0;background:var(--bt-color-bg);color:var(--bt-color-fg);font:inherit;font-size:var(--bt-text-row)}
.bt-input:focus{outline:none;border-color:var(--bt-color-accent)}
.bt-input[aria-invalid=true]{border-color:var(--bt-color-err)}
.bt-toggle{display:flex;align-items:center;justify-content:space-between;gap:12px;font-size:var(--bt-text-row)}
.bt-switch{appearance:none;-webkit-appearance:none;position:relative;flex-shrink:0;width:44px;height:24px;margin:0;border:1px solid var(--bt-color-edge);background:var(--bt-color-neutral-soft);cursor:pointer}
.bt-switch::after{content:'';position:absolute;top:3px;left:3px;width:16px;height:16px;background:var(--bt-color-soft);transition:transform .15s}
.bt-switch:checked{border-color:var(--bt-color-accent);background:var(--bt-color-accent-soft)}
.bt-switch:checked::after{transform:translateX(20px);background:var(--bt-color-accent)}
.bt-choices{display:flex;flex-wrap:wrap}
.bt-choice{padding:8px 12px;margin:0 -1px -1px 0;border:1px solid var(--bt-color-edge);border-radius:0;background:none;color:var(--bt-color-muted);cursor:pointer;font-family:var(--bt-font-mono);font-size:var(--bt-text-hint)}
.bt-choice[aria-pressed=true]{position:relative;border-color:var(--bt-color-accent);background:var(--bt-color-accent-soft);color:var(--bt-color-fg)}
`;

function storedDefault(field: ConfigField): string | null {
  const value = field.data.default;
  return value === null || value === undefined ? null : String(value);
}

function effective(fields: ConfigField[], stored: ConfigValues): ConfigValues {
  const values: ConfigValues = {};
  for (const field of fields) {
    const value = stored[field.data.key] ?? storedDefault(field);
    if (value !== null) values[field.data.key] = value;
  }
  return values;
}

function toStored(entries: ConfigEntry[]): ConfigValues {
  return Object.fromEntries(entries.map(entry => [entry.key, entry.value]));
}

function reason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function constraints(field: ConfigField): string | null {
  const parts: string[] = [];
  if (field.type === 'number') {
    if (field.data.min != null) parts.push(`min ${field.data.min}`);
    if (field.data.max != null) parts.push(`max ${field.data.max}`);
    if (field.data.step != null) parts.push(`step ${field.data.step}`);
  }
  if (field.type === 'string' || field.type === 'secret') {
    if (field.data.minLength != null) parts.push(`at least ${field.data.minLength} characters`);
    if (field.data.maxLength != null) parts.push(`at most ${field.data.maxLength} characters`);
  }
  return parts.length > 0 ? parts.join(' / ') : null;
}

function without<V>(record: Record<string, V>, key: string): Record<string, V> {
  const next = { ...record };
  delete next[key];
  return next;
}

function only<V>(record: Record<string, V>, keys: Set<string>): Record<string, V> {
  return Object.fromEntries(Object.entries(record).filter(([key]) => keys.has(key)));
}

function createEngine({ after, show, report, invalid, lost }: EngineOptions): Engine {
  const config = settings.config.pin();
  let phase: Phase = 'live';
  let fields: ConfigField[] | null = null;
  let held: ConfigValues = {};
  let wanted: Record<string, string | null> = {};
  let drafts: ConfigValues = {};
  let errors: Record<string, string> = {};
  let refused: Record<string, string | null> = {};
  let failure: string | null = null;
  let reported: string | null = null;
  let running = false;
  const queue: Op[] = [];
  const idlers: Array<() => void> = [];

  const label = (key: string): string => fields?.find(field => field.data.key === key)?.data.label ?? key;

  const stored = (): ConfigValues => {
    const next = { ...held };
    for (const [key, value] of Object.entries(wanted)) {
      if (value === null) delete next[key];
      else next[key] = value;
    }
    return next;
  };

  const publish = (): void => {
    if (phase !== 'live') return;
    if (fields === null) {
      show(failure === null ? { kind: 'loading' } : { kind: 'failed', reason: failure });
      return;
    }
    const values = effective(fields, stored());
    show({ kind: 'ready', fields, values, drafts, errors, stale: failure });
    const signature = JSON.stringify(values);
    if (signature === reported) return;
    reported = signature;
    report(values);
  };

  const load = async (): Promise<void> => {
    await after;
    if (phase !== 'live') return;
    try {
      const [next, entries] = await Promise.all([config.fields(), config.list()]);
      if (phase !== 'live') return;
      const keys = new Set(next.map(field => field.data.key));
      fields = next;
      held = toStored(entries);
      failure = null;
      drafts = only(drafts, keys);
      errors = only(errors, keys);
      for (const [key, value] of Object.entries(refused)) {
        if ((held[key] ?? null) === value) errors = without(errors, key);
      }
      refused = {};
    } catch (err) {
      if (phase === 'live') failure = reason(err);
    }
  };

  const write = async ({ key, value }: Write): Promise<void> => {
    try {
      if (value === null) {
        const left = (await config.delete(key)).value;
        held = left === null ? without(held, key) : { ...held, [key]: left };
      } else {
        await config.set(key, value);
        held = { ...held, [key]: value };
      }
      errors = without(errors, key);
      refused = without(refused, key);
    } catch (err) {
      if (phase === 'retired') lost(label(key), reason(err));
      errors = { ...errors, [key]: reason(err) };
      refused = { ...refused, [key]: value };
      if (phase === 'live' && !queue.some(op => op.kind === 'load')) queue.push({ kind: 'load' });
    }
    if (!queue.some(op => op.kind === 'write' && op.key === key)) wanted = without(wanted, key);
  };

  const drain = async (): Promise<void> => {
    if (running) return;
    running = true;
    while (queue.length > 0) {
      const op = queue.shift()!;
      await (op.kind === 'load' ? load() : write(op));
      publish();
    }
    running = false;
    for (const wake of idlers.splice(0)) wake();
  };

  const enqueue = (op: Op): void => {
    if (op.kind === 'load') {
      if (queue.some(queued => queued.kind === 'load')) return;
    } else {
      for (let at = queue.length - 1; at >= 0; at -= 1) {
        const queued = queue[at];
        if (queued.kind === 'write' && queued.key === op.key) queue.splice(at, 1);
      }
    }
    queue.push(op);
    void drain();
  };

  const idle = (): Promise<void> =>
    running || queue.length > 0 ? new Promise(resolve => idlers.push(resolve)) : Promise.resolve();

  const send = (key: string, value: string | null): void => {
    drafts = without(drafts, key);
    errors = without(errors, key);
    wanted = { ...wanted, [key]: value };
    enqueue({ kind: 'write', key, value });
    publish();
  };

  const settle = (key: string): void => {
    const draft = drafts[key];
    if (fields === null || draft === undefined) return;
    if (draft === (effective(fields, stored())[key] ?? '')) {
      drafts = without(drafts, key);
      errors = without(errors, key);
      return;
    }
    const problem = invalid(key);
    if (problem !== null) {
      errors = { ...errors, [key]: problem };
      return;
    }
    send(key, draft);
  };

  const finish = (key: string): void => {
    if (phase !== 'live') return;
    settle(key);
    publish();
  };

  const reload = (): void => {
    if (phase !== 'live') return;
    failure = null;
    publish();
    enqueue({ kind: 'load' });
  };

  const offRefresh = settings.onRefresh(reload);
  enqueue({ kind: 'load' });

  return {
    reload,
    draft(key, text) {
      if (phase !== 'live') return;
      drafts = { ...drafts, [key]: text };
      publish();
    },
    finish,
    commit(key, value) {
      if (phase === 'live') send(key, value);
    },
    reset(key) {
      if (phase === 'live') send(key, null);
    },
    async flush() {
      if (phase !== 'live') throw replaced();
      for (const key of Object.keys(drafts)) finish(key);
      const touched = new Set([...Object.keys(drafts), ...Object.keys(wanted)]);
      await idle();
      if (phase !== 'live') throw replaced();
      if (fields === null) throw new ConfigFormError('unavailable', failure ?? UNLOADED);
      const failed = only(errors, touched);
      const [first] = Object.entries(failed);
      if (first) throw new ConfigFormError('refused', `${label(first[0])}: ${first[1]}`, failed);
      return effective(fields, held);
    },
    async retire() {
      if (phase !== 'live') return idle();
      phase = 'retired';
      offRefresh();
      for (let at = queue.length - 1; at >= 0; at -= 1) if (queue[at].kind === 'load') queue.splice(at, 1);
      for (const key of Object.keys(drafts)) {
        settle(key);
        if (errors[key] !== undefined && drafts[key] !== undefined) lost(label(key), errors[key]);
      }
      await idle();
      config.release();
    },
    dispose() {
      if (phase === 'live') {
        phase = 'gone';
        config.release();
      }
      offRefresh();
    },
  };
}

type Slot = { current: Engine | null };
type Retiring = { app: string | null; drained: Promise<void> };
type FormState = { generation: number; after: Promise<void>; lost: string[] };

function appOf(context: SettingsContext | null): string | null {
  return context === null ? null : `${context.deviceId}\n${context.webappId}`;
}

export const ConfigForm = forwardRef<ConfigFormHandle, ConfigFormProps>(function ConfigForm({ onChange }, ref) {
  const [state, setState] = useState<FormState>(() => ({ generation: 0, after: Promise.resolve(), lost: [] }));
  const slot = useRef<Engine | null>(null);
  const retiring = useRef(new Set<Retiring>());

  useEffect(
    () =>
      settings.onContextChanged((context, previous) => {
        const old = slot.current;
        slot.current = null;
        setState(current => ({ ...current, generation: current.generation + 1, lost: [] }));
        if (old) {
          const entry: Retiring = { app: appOf(previous), drained: old.retire() };
          retiring.current.add(entry);
          void entry.drained.finally(() => retiring.current.delete(entry));
        }
        const app = appOf(context);
        const blocking = [...retiring.current].filter(entry => entry.app === null || app === null || entry.app === app);
        const after = Promise.all(blocking.map(entry => entry.drained)).then(() => undefined);
        setState(current => ({ ...current, after }));
      }),
    [],
  );

  useImperativeHandle(ref, () => ({ flush: () => slot.current?.flush() ?? Promise.reject(replaced()) }), []);

  return (
    <Form
      key={state.generation}
      onChange={onChange}
      slot={slot}
      after={state.after}
      lost={state.lost}
      onLost={(field, why) =>
        setState(previous => ({
          ...previous,
          lost: [...previous.lost, `${field} was not saved to the previous app: ${why}`],
        }))
      }
    />
  );
});

export async function closeSettings(form: ConfigFormHandle | null): Promise<void> {
  try {
    await form?.flush();
  } catch (err) {
    if (!(err instanceof ConfigFormError && err.kind === 'unavailable')) throw err;
  }
  settings.done();
}

type FormProps = ConfigFormProps & {
  slot: Slot;
  after: Promise<void>;
  lost: string[];
  onLost: (label: string, reason: string) => void;
};

function Form({ onChange, slot, after, lost, onLost }: FormProps) {
  const [view, setView] = useState<View>({ kind: 'loading' });
  const listener = useRef(onChange);
  listener.current = onChange;
  const loser = useRef(onLost);
  loser.current = onLost;
  const inputs = useRef(new Map<string, HTMLInputElement>());

  useEffect(() => {
    const created = createEngine({
      after,
      show: setView,
      report: values => listener.current?.(values),
      invalid: key => {
        const input = inputs.current.get(key);
        if (!input || input.checkValidity()) return null;
        return input.validationMessage || 'this value is not allowed';
      },
      lost: (label, why) => loser.current(label, why),
    });
    slot.current = created;
    return () => {
      created.dispose();
      if (slot.current === created) slot.current = null;
    };
  }, [slot, after]);

  const notes = lost.map((note, at) => (
    <p key={at} className="bt-note" data-tone="err">
      {note}
    </p>
  ));

  const retry = (
    <button type="button" className="bt-retry" onClick={() => slot.current?.reload()}>
      retry
    </button>
  );

  if (view.kind !== 'ready') {
    return (
      <div className="bt-form">
        <style>{STYLE}</style>
        {notes}
        <p className="bt-note" data-tone={view.kind === 'failed' ? 'err' : undefined}>
          {view.kind === 'failed' ? view.reason : 'loading settings'}
        </p>
        {view.kind === 'failed' ? retry : null}
      </div>
    );
  }

  return (
    <div className="bt-form">
      <style>{STYLE}</style>
      {notes}
      {view.stale !== null ? (
        <>
          <p className="bt-note" data-tone="err">
            {view.stale}
          </p>
          {retry}
        </>
      ) : null}
      {view.fields.length === 0 ? <p className="bt-note">no settings</p> : null}
      {view.fields.map(field => {
        const key = field.data.key;
        return (
          <FieldRow
            key={key}
            field={field}
            value={view.values[key] ?? ''}
            draft={view.drafts[key]}
            error={view.errors[key] ?? null}
            bind={input => {
              if (input) inputs.current.set(key, input);
              else inputs.current.delete(key);
            }}
            onDraft={text => slot.current?.draft(key, text)}
            onFinish={() => slot.current?.finish(key)}
            onCommit={value => slot.current?.commit(key, value)}
            onReset={() => slot.current?.reset(key)}
          />
        );
      })}
    </div>
  );
}

type ControlProps = {
  field: ConfigField;
  value: string;
  draft: string | undefined;
  bind: (input: HTMLInputElement | null) => void;
  onDraft: (text: string) => void;
  onFinish: () => void;
  onCommit: (value: string) => void;
};

type RowProps = ControlProps & { error: string | null; onReset: () => void };

function FieldRow({ error, onReset, ...control }: RowProps) {
  const { field } = control;
  const id = `bt-field-${field.data.key}`;
  const hint = constraints(field);
  return (
    <div className="bt-field">
      <div className="bt-head">
        <label className="bt-label" htmlFor={id}>
          {field.data.label}
        </label>
        {storedDefault(field) !== null ? (
          <button type="button" className="bt-reset" onClick={onReset}>
            reset
          </button>
        ) : null}
      </div>
      <Control {...control} id={id} invalid={error !== null} />
      {hint ? <p className="bt-note">{hint}</p> : null}
      {error ? (
        <p className="bt-note" data-tone="err">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function Control({
  id,
  field,
  value,
  draft,
  invalid,
  bind,
  onDraft,
  onFinish,
  onCommit,
}: ControlProps & { id: string; invalid: boolean }) {
  if (field.type === 'boolean') {
    const on = value === 'true';
    return (
      <div className="bt-toggle">
        <span>{on ? 'enabled' : 'disabled'}</span>
        <input
          id={id}
          className="bt-switch"
          type="checkbox"
          role="switch"
          checked={on}
          onChange={() => onCommit(on ? 'false' : 'true')}
        />
      </div>
    );
  }

  if (field.type === 'enum') {
    if (field.data.choices.length === 0) return <p className="bt-note">this field declares no choices</p>;
    return (
      <div className="bt-choices" role="group" id={id}>
        {field.data.choices.map(choice => (
          <button
            key={choice}
            type="button"
            className="bt-choice"
            aria-pressed={choice === value}
            onClick={() => {
              if (choice !== value) onCommit(choice);
            }}>
            {choice}
          </button>
        ))}
      </div>
    );
  }

  const common = {
    id,
    ref: bind,
    className: 'bt-input',
    value: draft ?? value,
    'aria-invalid': invalid,
    onChange: (event: { currentTarget: HTMLInputElement }) => onDraft(event.currentTarget.value),
    onBlur: onFinish,
    onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => {
      if (event.key === 'Enter') event.currentTarget.blur();
    },
  };

  if (field.type === 'number') {
    return (
      <input
        {...common}
        type="number"
        inputMode="decimal"
        min={field.data.min ?? undefined}
        max={field.data.max ?? undefined}
        step={field.data.step ?? 'any'}
      />
    );
  }

  return (
    <input
      {...common}
      type={field.type === 'secret' ? 'password' : 'text'}
      autoCapitalize="none"
      autoCorrect="off"
      spellCheck={false}
      pattern={field.data.pattern ?? undefined}
      minLength={field.data.minLength ?? undefined}
      maxLength={field.data.maxLength ?? undefined}
      placeholder={field.type === 'secret' ? 'not set' : undefined}
    />
  );
}

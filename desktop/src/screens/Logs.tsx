import type { DeviceLogLine, LogLevel } from '@bridgething/companion-types';
import { Button, Field, Segmented, Spinner, Switch, cx, describeError } from '@bridgething/ui';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import type { RefObject, VNode } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { clock } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { pickLogFile } from '../lib/picker.ts';
import { deviceLogsFor, logLimit, logStreaming } from '../stores/session.ts';

const LEVELS = ['all', 'debug', 'info', 'warn', 'error'] as const;
type LevelFilter = (typeof LEVELS)[number];

const SEVERITY: Record<LogLevel, number> = { trace: 0, debug: 1, info: 2, warn: 3, error: 4 };

const LIMITS = [
  { value: '500', label: '500' },
  { value: '2000', label: '2k' },
] as const;

const LEVEL_TONE: Record<LogLevel, string> = {
  trace: 'text-dim',
  debug: 'text-soft',
  info: 'text-accent',
  warn: 'text-warn',
  error: 'text-err',
};

const BOTTOM_SLACK_PX = 8;

export function LogsScreen(): VNode {
  const session = useDesktop();
  const [level, setLevel] = useState<LevelFilter>('all');
  const [query, setQuery] = useState('');
  const [failure, setFailure] = useState<string | undefined>(undefined);
  const [copied, setCopied] = useState(false);
  const [saving, setSaving] = useState(false);

  const limit = logLimit.value;
  const lines = deviceLogsFor(limit);
  const streaming = logStreaming.data.value;
  const held = lines.data.value;
  const visible = useMemo(() => filter(held, level, query), [held, level, query]);
  const note = failure ?? lines.error.value;
  const tail = useTail(visible, [level, query, limit]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const toggle = (next: boolean) => {
    void session.setDeviceLogStreaming(next);
  };

  const copy = async () => {
    try {
      await writeText(render(visible));
      setFailure(undefined);
      setCopied(true);
    } catch (reason) {
      setFailure(describeError(reason));
    }
  };

  const save = async () => {
    const path = await pickLogFile(`bridgething-${stamp()}.log`).catch(() => null);
    if (!path) return;
    setSaving(true);
    try {
      await session.exportLogs(path, render(visible));
      setFailure(undefined);
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div class="flex min-h-0 min-w-0 flex-1 flex-col">
      <div class="flex shrink-0 flex-wrap items-center gap-3 border-b border-rule bg-screen px-6 py-3">
        <Segmented options={LEVELS} value={level} label="minimum level" size="sm" onChange={setLevel} />
        <Segmented
          options={LIMITS}
          value={String(limit)}
          label="tail depth"
          size="sm"
          onChange={next => {
            logLimit.value = Number(next);
          }}
        />
        <Field
          class="min-w-48 flex-1"
          value={query}
          onInput={setQuery}
          icon={<Icon name="search" size={14} />}
          type="search"
          placeholder="filter"
          clearable
        />
        <span class="shrink-0 font-mono text-hint text-muted tabular-nums">
          {visible.length === held.length ? held.length : `${visible.length}/${held.length}`}
        </span>
        {lines.pending.value ? <Spinner /> : null}
        <Button
          variant="ghost"
          size="sm"
          icon={<Icon name={copied ? 'check' : 'copy'} size={14} />}
          disabled={visible.length === 0}
          onClick={() => void copy()}>
          {copied ? 'copied' : 'copy'}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          icon={<Icon name="download" size={14} />}
          loading={saving}
          disabled={visible.length === 0}
          onClick={() => void save()}>
          export
        </Button>
        <label class="flex shrink-0 items-center gap-2 font-mono text-eyebrow text-muted uppercase">
          device
          <Switch checked={streaming} label="stream device logs" onChange={toggle} />
        </label>
      </div>

      <div class="relative flex min-h-0 min-w-0 flex-1 flex-col">
        <div ref={tail.scroller} onScroll={tail.track} class="min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-2">
          {visible.length === 0 ? (
            <p class="py-12 text-center text-body text-muted">{held.length === 0 ? 'no lines' : 'no matches'}</p>
          ) : (
            <ol ref={tail.list} class="m-0 list-none p-0 font-mono text-hint leading-5">
              {visible.map(line => (
                <li key={line.seq} data-seq={line.seq} class="flex min-w-0 gap-3 px-2 hover:bg-neutral-soft">
                  <span class="shrink-0 text-dim tabular-nums">{clock(line.tsUnixMs)}</span>
                  <span class={cx('w-[5ch] shrink-0 uppercase', LEVEL_TONE[line.level])}>{line.level}</span>
                  <span class="w-[6ch] shrink-0 text-dim">{line.origin}</span>
                  <span class="w-[22ch] shrink-0 truncate text-muted" title={line.target}>
                    {line.target}
                  </span>
                  <span class="min-w-0 flex-1 wrap-break-word whitespace-pre-wrap text-off-white">{line.message}</span>
                </li>
              ))}
            </ol>
          )}
          {note ? <p class="mt-3 text-hint text-err">{note}</p> : null}
        </div>
        {tail.following ? null : (
          <Button
            variant="primary"
            size="sm"
            class="absolute right-6 bottom-4"
            icon={<Icon name="down" size={14} />}
            onClick={tail.pin}>
            latest
          </Button>
        )}
      </div>
    </div>
  );
}

type Anchor = { seq: string; offset: number };

function useTail(
  rows: readonly DeviceLogLine[],
  resets: readonly unknown[],
): {
  scroller: RefObject<HTMLDivElement | null>;
  list: RefObject<HTMLOListElement | null>;
  following: boolean;
  track: () => void;
  pin: () => void;
} {
  const scroller = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLOListElement>(null);
  const pinned = useRef(true);
  const anchor = useRef<Anchor | null>(null);
  const [following, setFollowing] = useState(true);

  const pin = () => {
    pinned.current = true;
    anchor.current = null;
    setFollowing(true);
    const view = scroller.current;
    if (view) view.scrollTop = view.scrollHeight;
  };

  const track = () => {
    const view = scroller.current;
    if (!view) return;
    const bottom = view.scrollHeight - view.scrollTop - view.clientHeight <= BOTTOM_SLACK_PX;
    pinned.current = bottom;
    anchor.current = bottom ? null : topRow(view, list.current);
    setFollowing(bottom);
  };

  useLayoutEffect(pin, resets);

  useLayoutEffect(() => {
    const view = scroller.current;
    if (!view) return;
    if (pinned.current) {
      view.scrollTop = view.scrollHeight;
      return;
    }
    const held = anchor.current;
    const row = held ? list.current?.querySelector<HTMLElement>(`[data-seq="${held.seq}"]`) : null;
    if (held && row) view.scrollTop += offsetIn(row, view) - held.offset;
    anchor.current = topRow(view, list.current);
  }, [rows]);

  useEffect(() => {
    const view = scroller.current;
    if (!view) return;
    const observer = new ResizeObserver(() => {
      if (pinned.current) view.scrollTop = view.scrollHeight;
    });
    observer.observe(view);
    return () => observer.disconnect();
  }, []);

  return { scroller, list, following, track, pin };
}

function offsetIn(row: HTMLElement, view: HTMLElement): number {
  return row.getBoundingClientRect().top - view.getBoundingClientRect().top;
}

function topRow(view: HTMLElement, list: HTMLOListElement | null): Anchor | null {
  if (!list) return null;
  const top = view.getBoundingClientRect().top;
  const rows = list.children;
  let low = 0;
  let high = rows.length - 1;
  let found: HTMLElement | null = null;
  while (low <= high) {
    const mid = (low + high) >> 1;
    const row = rows[mid] as HTMLElement;
    if (row.getBoundingClientRect().bottom > top) {
      found = row;
      high = mid - 1;
    } else {
      low = mid + 1;
    }
  }
  const seq = found?.dataset['seq'];
  return found && seq !== undefined ? { seq, offset: offsetIn(found, view) } : null;
}

function filter(lines: DeviceLogLine[], level: LevelFilter, query: string): DeviceLogLine[] {
  const floor = level === 'all' ? -1 : SEVERITY[level];
  const needle = query.trim().toLowerCase();
  return lines.filter(line => {
    if (floor >= 0 && SEVERITY[line.level] < floor) return false;
    if (!needle) return true;
    return line.message.toLowerCase().includes(needle) || line.target.toLowerCase().includes(needle);
  });
}

function render(lines: DeviceLogLine[]): string {
  return lines
    .map(
      line =>
        `${new Date(line.tsUnixMs).toISOString()} ${line.level.padEnd(5)} ${line.origin.padEnd(6)} ${line.target} ${line.message}`,
    )
    .join('\n');
}

function stamp(): string {
  return new Date().toISOString().replace(/[:.]/g, '-').replace(/Z$/, '');
}

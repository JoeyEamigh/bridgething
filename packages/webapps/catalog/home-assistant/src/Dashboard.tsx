import { memo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import type { Tile } from './App';
import { brightnessPct, controlKind, domainOf, friendlyName, isActive, num } from './domains';
import type { HaState, HaStatus } from './ha';
import { DomainIcon } from './icons';

type Props = {
  tiles: Tile[];
  status: HaStatus;
  toast: string | null;
  onActivate: (s: HaState) => void;
  onSetTemp: (entityId: string, target: number) => void;
  onSetBrightness: (entityId: string, pct: number) => void;
  onOpenPicker: () => void;
};

const TILE_MAX_REM = 16;
const MAX_COLS_ON_SCREEN = 4;
const SWIPE_THRESHOLD_PX = 12;

function gridShape(count: number): { cols: number; rows: number; fits: boolean } {
  const rows = count <= 2 ? 1 : Math.min(3, Math.max(2, Math.ceil(count / MAX_COLS_ON_SCREEN)));
  const cols = Math.ceil(count / rows);
  return { cols, rows, fits: cols <= MAX_COLS_ON_SCREEN };
}

export default function Dashboard({
  tiles,
  status,
  toast,
  onActivate,
  onSetTemp,
  onSetBrightness,
  onOpenPicker,
}: Props) {
  const live = tiles.some(t => t.state);
  const shape = gridShape(tiles.length);
  if (!live && status.kind === 'error') return <FullError message={status.message} />;

  return (
    <div className="relative flex h-full w-full flex-col bg-bg text-off-white">
      <header className="mb-3 flex items-center justify-between border-b border-rule px-6 pt-4 pb-2">
        <div className="flex items-baseline gap-3">
          <span className="font-mono text-eyebrow tracking-[0.25em] text-dim uppercase">home assistant</span>
          {status.kind !== 'ready' && <span className="font-mono text-hint text-warn">{statusLabel(status)}</span>}
        </div>
        <button
          onClick={onOpenPicker}
          className="border border-rule px-4 py-1.5 font-mono text-eyebrow text-soft active:bg-neutral-soft">
          edit tiles
        </button>
      </header>

      {shape.fits ? (
        <div
          className="mx-auto grid w-full flex-1 gap-3 px-6 pb-5"
          style={{
            gridTemplateColumns: `repeat(${shape.cols}, minmax(0,1fr))`,
            gridTemplateRows: `repeat(${shape.rows}, minmax(0,1fr))`,
            maxWidth: `${shape.cols * TILE_MAX_REM + (shape.cols - 1) * 0.75 + 3}rem`,
          }}>
          {tiles.map(t => (
            <TileView key={t.entityId} tile={t} onActivate={onActivate} onSetTemp={onSetTemp} onSetBrightness={onSetBrightness} />
          ))}
        </div>
      ) : (
        <div className="grid flex-1 grid-flow-col grid-rows-3 auto-cols-44 gap-3 overflow-x-auto px-6 pb-5">
          {tiles.map(t => (
            <TileView key={t.entityId} tile={t} onActivate={onActivate} onSetTemp={onSetTemp} onSetBrightness={onSetBrightness} />
          ))}
        </div>
      )}

      {toast && (
        <div className="pointer-events-none absolute inset-x-0 bottom-4 flex justify-center">
          <div className="border border-edge bg-screen px-5 py-2 font-mono text-hint text-near">{toast}</div>
        </div>
      )}
    </div>
  );
}

type TileProps = {
  tile: Tile;
  onActivate: Props['onActivate'];
  onSetTemp: Props['onSetTemp'];
  onSetBrightness: Props['onSetBrightness'];
};

export function sameTile(a: Tile, b: Tile): boolean {
  return (
    a.entityId === b.entityId &&
    a.pendingTemp === b.pendingTemp &&
    a.pendingBrightness === b.pendingBrightness &&
    sameState(a.state, b.state)
  );
}

function sameState(a: HaState | null, b: HaState | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.entityId === b.entityId && a.state === b.state && a.attributes === b.attributes;
}

const TileView = memo(TileBody, (a: TileProps, b: TileProps) => {
  return (
    a.onActivate === b.onActivate &&
    a.onSetTemp === b.onSetTemp &&
    a.onSetBrightness === b.onSetBrightness &&
    sameTile(a.tile, b.tile)
  );
});

function TileBody({ tile, onActivate, onSetTemp, onSetBrightness }: TileProps) {
  const { entityId, state } = tile;
  if (!state || state.state === 'unavailable') {
    return (
      <div className="flex flex-col justify-between border border-rule border-dashed bg-screen p-4 opacity-60">
        <DomainIcon entityId={entityId} />
        <div>
          <div className="truncate font-mono text-hint">{entityId}</div>
          <div className="font-mono text-eyebrow tracking-[0.12em] text-dim uppercase">unavailable</div>
        </div>
      </div>
    );
  }

  const kind = controlKind(entityId);
  if (kind === 'climate') return <ClimateTile tile={tile} state={state} onSetTemp={onSetTemp} />;
  if (kind === 'readonly') return <ReadonlyTile state={state} />;
  if (domainOf(entityId) === 'light')
    return <LightTile tile={tile} state={state} onActivate={onActivate} onSetBrightness={onSetBrightness} />;
  return <ActionTile state={state} kind={kind} onActivate={onActivate} />;
}

function ActionTile({
  state,
  kind,
  onActivate,
}: {
  state: HaState;
  kind: 'toggle' | 'lock' | 'momentary';
  onActivate: Props['onActivate'];
}) {
  const active = isActive(state);
  const accent = kind !== 'momentary' && active;
  return (
    <button
      onClick={() => onActivate(state)}
      className={`flex flex-col justify-between border p-4 text-left transition-colors ${
        accent
          ? 'border-accent bg-accent text-screen'
          : 'border-rule bg-screen text-off-white active:border-edge active:bg-neutral-soft'
      }`}>
      <DomainIcon entityId={state.entityId} />
      <div>
        <div className="truncate text-row font-medium">{friendlyName(state)}</div>
        <div className={`font-mono text-eyebrow tracking-[0.12em] uppercase ${accent ? 'text-screen/70' : 'text-dim'}`}>
          {actionLabel(state, kind)}
        </div>
      </div>
    </button>
  );
}

function LightTile({
  tile,
  state,
  onActivate,
  onSetBrightness,
}: {
  tile: Tile;
  state: HaState;
  onActivate: Props['onActivate'];
  onSetBrightness: Props['onSetBrightness'];
}) {
  const elRef = useRef<HTMLButtonElement | null>(null);
  const dragRef = useRef<{ id: number; startX: number; swiping: boolean } | null>(null);
  const swipedRef = useRef(false);
  const [swipePct, setSwipePct] = useState<number | null>(null);

  const active = isActive(state);
  const brightness = tile.pendingBrightness ?? brightnessPct(state);

  const pctAt = (clientX: number): number => {
    const el = elRef.current;
    if (!el) return 0;
    const rect = el.getBoundingClientRect();
    return clamp(Math.round(((clientX - rect.left) / rect.width) * 100), 0, 100);
  };

  const handlePointerDown = (e: ReactPointerEvent<HTMLButtonElement>) => {
    swipedRef.current = false;
    dragRef.current = { id: e.pointerId, startX: e.clientX, swiping: false };
    elRef.current?.setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: ReactPointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current;
    if (!drag || e.pointerId !== drag.id) return;
    if (!drag.swiping) {
      if (Math.abs(e.clientX - drag.startX) < SWIPE_THRESHOLD_PX) return;
      drag.swiping = true;
    }
    setSwipePct(pctAt(e.clientX));
  };

  const endDrag = (e: ReactPointerEvent<HTMLButtonElement>, commit: boolean) => {
    const drag = dragRef.current;
    dragRef.current = null;
    setSwipePct(null);
    if (!drag || e.pointerId !== drag.id) return;
    if (drag.swiping && commit) {
      swipedRef.current = true;
      onSetBrightness(state.entityId, pctAt(e.clientX));
    }
  };

  const handleClick = () => {
    if (swipedRef.current) {
      swipedRef.current = false;
      return;
    }
    onActivate(state);
  };

  return (
    <button
      ref={elRef}
      onClick={handleClick}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={e => endDrag(e, true)}
      onPointerCancel={e => endDrag(e, false)}
      style={{ touchAction: 'pan-y' }}
      className={`relative flex flex-col justify-between border p-4 text-left select-none ${
        active
          ? 'border-accent bg-accent text-screen'
          : 'border-rule bg-screen text-off-white active:border-edge active:bg-neutral-soft'
      }`}>
      <DomainIcon entityId={state.entityId} />
      <div>
        <div className="truncate text-row font-medium">{friendlyName(state)}</div>
        <div className={`font-mono text-eyebrow tracking-[0.12em] uppercase ${active ? 'text-screen/70' : 'text-dim'}`}>
          {actionLabel(state, 'toggle')}
          {brightness != null && ` · ${brightness}%`}
        </div>
      </div>
      {swipePct != null && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-screen/75">
          <span className="font-display text-5xl font-medium tracking-display text-off-white tabular-nums">
            {swipePct}
            <span className="text-3xl">%</span>
          </span>
        </div>
      )}
    </button>
  );
}

function ClimateTile({ tile, state, onSetTemp }: { tile: Tile; state: HaState; onSetTemp: Props['onSetTemp'] }) {
  const current = num(state.attributes['current_temperature']);
  const target = tile.pendingTemp ?? num(state.attributes['temperature']);
  const step = num(state.attributes['target_temp_step']) ?? 0.5;
  const min = num(state.attributes['min_temp']) ?? 7;
  const max = num(state.attributes['max_temp']) ?? 35;
  const adjust = (delta: number) => {
    if (target == null) return;
    onSetTemp(state.entityId, clamp(round(target + delta, step), min, max));
  };
  return (
    <div className="flex flex-col justify-between border border-rule bg-screen p-4">
      <div className="flex items-center justify-between">
        <DomainIcon entityId={state.entityId} />
        <span className="font-mono text-eyebrow tracking-[0.12em] text-dim uppercase">{state.state}</span>
      </div>
      <div className="truncate text-row font-medium">{friendlyName(state)}</div>
      <div className="flex items-center justify-between">
        <button
          onClick={() => adjust(-step)}
          className="size-9 border border-edge text-lg text-near active:bg-neutral-soft"
          disabled={target == null}>
          -
        </button>
        <div className="text-center">
          <div className="font-display text-2xl font-medium tracking-wordmark tabular-nums">
            {target != null ? `${fmt(target)}°` : '--'}
          </div>
          {current != null && <div className="font-mono text-eyebrow tabular-nums text-dim">now {fmt(current)}°</div>}
        </div>
        <button
          onClick={() => adjust(step)}
          className="size-9 border border-edge text-lg text-near active:bg-neutral-soft"
          disabled={target == null}>
          +
        </button>
      </div>
    </div>
  );
}

function ReadonlyTile({ state }: { state: HaState }) {
  const unit =
    typeof state.attributes['unit_of_measurement'] === 'string'
      ? (state.attributes['unit_of_measurement'] as string)
      : '';
  return (
    <div className="flex flex-col justify-between border border-rule bg-screen p-4">
      <DomainIcon entityId={state.entityId} />
      <div>
        <div className="truncate text-row font-medium">{friendlyName(state)}</div>
        <div className="font-display text-row-lg tracking-display tabular-nums">
          {state.state}
          {unit && <span className="ml-0.5 font-mono text-hint text-dim">{unit}</span>}
        </div>
      </div>
    </div>
  );
}

function FullError({ message }: { message: string }) {
  return (
    <div className="flex h-full w-full items-center justify-center bg-bg px-10">
      <div className="max-w-136 border border-err/40 bg-err-soft px-5 py-3 text-center font-mono text-body text-err">
        {message}
      </div>
    </div>
  );
}

function actionLabel(state: HaState, kind: 'toggle' | 'lock' | 'momentary'): string {
  if (kind === 'momentary') return 'tap to run';
  return state.state;
}

function statusLabel(status: HaStatus): string {
  if (status.kind === 'error') return status.message;
  if (status.kind === 'connecting') return 'reconnecting...';
  if (status.kind === 'authenticating') return 'authenticating...';
  return '';
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

function round(v: number, step: number): number {
  return Math.round(v / step) * step;
}

function fmt(v: number): string {
  return Number.isInteger(v) ? String(v) : v.toFixed(1);
}

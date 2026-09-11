import {
  type BridgethingClient,
  type OtaError as OtaErrorMsg,
  type OtaPhase,
  type OtaProgress,
  type WebappInfo,
} from '@bridgething/client';
import { useEffect, useMemo, useState } from 'react';

import { Settings } from './settings';

type TileEntry = {
  info: WebappInfo;
  iconUrl: string | null;
};

type OtaSnapshot = {
  attempt: number;
  progress: OtaProgress | null;
  error: OtaErrorMsg | null;
  dismissed: number | null;
};

const OTA_IDLE: OtaSnapshot = { attempt: 0, progress: null, error: null, dismissed: null };

function otaVisible(snapshot: OtaSnapshot): boolean {
  return (snapshot.progress !== null || snapshot.error !== null) && snapshot.dismissed !== snapshot.attempt;
}

export function Launcher({ client }: { client: BridgethingClient }) {
  const [tiles, setTiles] = useState<TileEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activating, setActivating] = useState<TileEntry | null>(null);
  const [ota, setOta] = useState<OtaSnapshot>(OTA_IDLE);
  const [view, setView] = useState<'apps' | 'settings'>('apps');

  useEffect(() => {
    let cancelled = false;
    const icons = new Map<string, string>();

    const iconFor = async (info: WebappInfo): Promise<string | null> => {
      if (!info.iconHash) return null;
      const key = `${info.id}:${info.iconHash}`;
      const held = icons.get(key);
      if (held) return held;
      const iconResult = await client.webapp.icon({ id: info.id });
      if (!iconResult.ok) return null;
      const bytes = new Uint8Array(iconResult.response.bytes as unknown as number[]);
      const blob = new Blob([bytes], { type: iconResult.response.mime ?? 'application/octet-stream' });
      const url = URL.createObjectURL(blob);
      icons.set(key, url);
      return url;
    };

    const reload = async () => {
      const [listResult, currentResult] = await Promise.all([client.webapp.list(), client.webapp.current()]);
      if (!listResult.ok) {
        setError('failed to list webapps');
        return;
      }
      const selfId = currentResult.ok ? currentResult.response.id : null;
      const visible = listResult.response.webapps.filter(w => w.id !== selfId);
      const entries: TileEntry[] = await Promise.all(
        visible.map(async info => ({ info, iconUrl: await iconFor(info) })),
      );
      if (cancelled) return;
      const live = new Set(visible.map(info => `${info.id}:${info.iconHash ?? ''}`));
      for (const [key, url] of icons) {
        if (live.has(key)) continue;
        URL.revokeObjectURL(url);
        icons.delete(key);
      }
      setError(null);
      setTiles(entries);
    };

    const refresh = () => {
      reload().catch(err => setError(err instanceof Error ? err.message : String(err)));
    };

    refresh();
    const offInstalled = client.webapp.onWebappInstalled(refresh);
    const offUninstalled = client.webapp.onWebappUninstalled(refresh);

    return () => {
      cancelled = true;
      offInstalled();
      offUninstalled();
      for (const url of icons.values()) URL.revokeObjectURL(url);
    };
  }, [client]);

  useEffect(() => {
    const offProgress = client.system.onOtaProgress(progress => {
      setOta(prev =>
        prev.progress && !prev.error && !isNewRun(prev.progress, progress)
          ? { ...prev, progress }
          : { attempt: prev.attempt + 1, progress, error: null, dismissed: prev.dismissed },
      );
    });
    const offError = client.system.onOtaError(error => {
      setOta(prev => ({ attempt: prev.attempt + 1, progress: null, error, dismissed: prev.dismissed }));
    });
    const offFinished = client.system.onOtaFinished(() => setOta(OTA_IDLE));
    return () => {
      offProgress();
      offError();
      offFinished();
    };
  }, [client]);

  if (view === 'settings') {
    return <Settings client={client} onClose={() => setView('apps')} />;
  }

  const onActivate = async (entry: TileEntry) => {
    if (activating) return;
    setActivating(entry);
    const r = await client.webapp.activate({ id: entry.info.id });
    if (!r.ok) setActivating(null);
  };

  const grid = tiles === null ? null : gridShape(tiles.length + 1);

  const body = error ? (
    <div className="flex flex-1 items-center justify-center">
      <div className="border border-err/40 bg-err-soft px-5 py-3 font-mono text-body text-err">{error}</div>
    </div>
  ) : tiles === null || grid === null ? (
    <div className="flex flex-1 items-center justify-center text-dim">
      <div className="font-mono text-body tracking-[0.12em] uppercase">loading apps</div>
    </div>
  ) : (
    <div className={`flex-1 ${grid.fits ? 'flex items-center justify-center' : 'overflow-y-auto'}`}>
      <div
        className="grid w-full justify-center gap-3"
        style={{ gridTemplateColumns: `repeat(${grid.cols}, ${grid.tile}px)` }}>
        {tiles.map(t => (
          <Tile key={t.info.id} entry={t} icon={grid.icon} onActivate={onActivate} disabled={activating !== null} />
        ))}
        <SettingsTile icon={grid.icon} onOpen={() => setView('settings')} disabled={activating !== null} />
      </div>
    </div>
  );

  return (
    <Shell
      status={
        otaVisible(ota) ? null : (
          <OtaChip snapshot={ota} onResume={() => setOta(prev => ({ ...prev, dismissed: null }))} />
        )
      }>
      {body}
      {activating && <ActivatingOverlay entry={activating} />}
      {otaVisible(ota) && (
        <OtaOverlay snapshot={ota} onDismiss={() => setOta(prev => ({ ...prev, dismissed: prev.attempt }))} />
      )}
    </Shell>
  );
}

function Shell({ status, children }: { status?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="relative flex h-full w-full flex-col bg-bg px-4 py-3">
      <div className="mb-3 flex h-9 items-center gap-3 border-b border-rule pb-2">
        <div className="font-mono text-eyebrow tracking-[0.25em] text-dim uppercase">apps</div>
        {status}
      </div>
      {children}
    </div>
  );
}

const MAX_COLS = 5;
const MAX_ROWS_ON_SCREEN = 2;
const GAP_PX = 12;
const GRID_WIDTH_PX = 768;
const TILE_MIN_PX = 128;
const TILE_MAX_PX = 176;

function gridShape(count: number): { cols: number; fits: boolean; tile: number; icon: number } {
  const rows = Math.max(1, Math.ceil(count / MAX_COLS));
  const cols = Math.ceil(count / rows);
  const spread = (GRID_WIDTH_PX - (cols - 1) * GAP_PX) / cols;
  const tile = Math.max(TILE_MIN_PX, Math.min(TILE_MAX_PX, Math.floor(spread)));
  return { cols, fits: rows <= MAX_ROWS_ON_SCREEN, tile, icon: Math.round(tile * 0.6) };
}

function OtaChip({ snapshot, onResume }: { snapshot: OtaSnapshot; onResume: () => void }) {
  const { progress, error } = snapshot;
  if (!progress && !error) return null;
  return (
    <button
      type="button"
      onClick={onResume}
      className="flex items-center gap-2 border border-rule px-3 py-1.5 font-mono text-eyebrow tracking-[0.08em] transition active:bg-neutral-soft">
      <span className={`size-1.5 ${error ? 'bg-err' : 'animate-pulse bg-accent'}`} />
      <span className={error ? 'text-err' : 'text-soft'}>
        {error ? 'update failed' : progress ? OTA_PHASES[progress.phase].label : null}
      </span>
    </button>
  );
}

function GearIcon({ size = 24 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
      <circle cx="12" cy="12" r="3" />
      <path
        d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

const OTA_PHASES: Record<OtaPhase, { label: string; rank: number }> = {
  streaming: { label: 'receiving update', rank: 0 },
  verifying: { label: 'verifying', rank: 1 },
  writing: { label: 'installing', rank: 2 },
  confirming: { label: 'confirming', rank: 3 },
  reboot: { label: 'rebooting', rank: 4 },
};

function isNewRun(prev: OtaProgress, next: OtaProgress): boolean {
  return next.step < prev.step || OTA_PHASES[next.phase].rank < OTA_PHASES[prev.phase].rank;
}

function OtaOverlay({ snapshot, onDismiss }: { snapshot: OtaSnapshot; onDismiss: () => void }) {
  const { progress, error } = snapshot;
  return (
    <div className="absolute inset-0 z-40 flex flex-col items-center justify-center gap-4 bg-screen">
      {error ? (
        <>
          <div className="font-mono text-body tracking-[0.2em] text-err uppercase">update failed</div>
          <div className="max-w-md border border-rule bg-bg px-4 py-3 text-center font-mono text-hint text-soft">
            {error.code}: {error.msg}
          </div>
          <button
            type="button"
            onClick={onDismiss}
            className="mt-2 border border-edge px-6 py-2.5 font-mono text-hint text-near active:bg-neutral-soft">
            dismiss
          </button>
        </>
      ) : progress ? (
        <>
          <div className="font-mono text-eyebrow tracking-[0.25em] text-dim uppercase">system update</div>
          <div className="font-display text-title font-medium tracking-display text-off-white">
            {OTA_PHASES[progress.phase].label}
          </div>
          <div className="h-1.5 w-72 border border-rule bg-bg">
            <div
              className="h-full bg-accent transition-[width] duration-200"
              style={{ width: `${Math.min(100, Math.max(0, progress.percent))}%` }}
            />
          </div>
          <div className="font-mono text-hint tabular-nums text-accent">{progress.percent}%</div>
          <button
            type="button"
            onClick={onDismiss}
            className="mt-2 border border-edge px-6 py-2.5 font-mono text-hint text-near active:bg-neutral-soft">
            hide
          </button>
          <div className="font-mono text-eyebrow text-dim">installing continues in the background</div>
        </>
      ) : null}
    </div>
  );
}

function TileShell({
  label,
  icon,
  onClick,
  disabled,
  children,
}: {
  label: string;
  icon: number;
  onClick: () => void;
  disabled: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="flex w-full flex-col items-center gap-2 border border-rule bg-screen p-3 transition-colors active:border-edge active:bg-neutral-soft disabled:opacity-60">
      <div className="flex items-center justify-center overflow-hidden" style={{ height: icon, width: icon }}>
        {children}
      </div>
      <span className="line-clamp-2 h-10 w-full text-center text-row font-medium leading-5 text-off-white">
        {label}
      </span>
    </button>
  );
}

function Tile({
  entry,
  icon,
  onActivate,
  disabled,
}: {
  entry: TileEntry;
  icon: number;
  onActivate: (entry: TileEntry) => void;
  disabled: boolean;
}) {
  const { info, iconUrl } = entry;
  const fallback = useMemo(() => fallbackStyle(info), [info]);

  return (
    <TileShell label={info.name} icon={icon} onClick={() => onActivate(entry)} disabled={disabled}>
      {iconUrl ? (
        <img src={iconUrl} alt="" className="h-full w-full object-contain" draggable={false} />
      ) : (
        <div
          className="flex h-full w-full items-center justify-center"
          style={{ background: fallback.background, color: fallback.foreground }}>
          <span className="font-display text-2xl font-medium tracking-wordmark">{fallback.letter}</span>
        </div>
      )}
    </TileShell>
  );
}

function SettingsTile({ icon, onOpen, disabled }: { icon: number; onOpen: () => void; disabled: boolean }) {
  return (
    <TileShell label="Settings" icon={icon} onClick={onOpen} disabled={disabled}>
      <div className="flex h-full w-full items-center justify-center bg-neutral-soft text-soft">
        <GearIcon size={Math.round(icon * 0.45)} />
      </div>
    </TileShell>
  );
}

function ActivatingOverlay({ entry }: { entry: TileEntry }) {
  const { info, iconUrl } = entry;
  const fallback = useMemo(() => fallbackStyle(info), [info]);
  return (
    <div className="absolute inset-0 z-50 flex flex-col items-center justify-center gap-6 bg-screen">
      <div
        className="flex h-28 w-28 items-center justify-center overflow-hidden border border-rule"
        style={iconUrl ? undefined : { background: fallback.background, color: fallback.foreground }}>
        {iconUrl ? (
          <img src={iconUrl} alt="" className="h-full w-full object-contain" draggable={false} />
        ) : (
          <span className="font-display text-4xl font-medium tracking-wordmark">{fallback.letter}</span>
        )}
      </div>
      <div className="font-display text-title font-medium tracking-display text-off-white">{info.name}</div>
      <div className="size-6 animate-spin border border-rule-strong border-t-accent" />
    </div>
  );
}

function fallbackStyle(info: WebappInfo): { letter: string; background: string; foreground: string } {
  const letter = (info.name.trim().charAt(0) || '?').toUpperCase();
  const id = String(info.id).replace(/-/g, '');
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  const hue = h % 360;
  const background = `hsl(${hue}deg 42% 13%)`;
  const foreground = `hsl(${hue}deg 62% 74%)`;
  return { letter, background, foreground };
}

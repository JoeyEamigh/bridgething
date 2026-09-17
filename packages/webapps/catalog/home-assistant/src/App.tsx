import { BridgethingClient } from '@bridgething/client';
import { daemonUrl } from '@bridgething/webapp-shared/daemon';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import Dashboard from './Dashboard';
import { controlKind, brightnessPct, momentaryCall, optimisticToggle, toggleCall } from './domains';
import { HaConnection, type HaEntities, type HaState, type HaStatus } from './ha';
import Picker from './Picker';

const SELECTION_KEY = 'selected_entities';
const TEMP_DEBOUNCE_MS = 600;

type Mode =
  | { kind: 'loading' }
  | { kind: 'needs-config'; message: string }
  | { kind: 'picker'; all: HaState[] }
  | { kind: 'dashboard' };

export default function App() {
  const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
  const connRef = useRef<HaConnection | null>(null);
  const tempTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>());

  const [mode, setMode] = useState<Mode>({ kind: 'loading' });
  const [status, setStatus] = useState<HaStatus>({ kind: 'connecting' });
  const [selection, setSelection] = useState<string[]>([]);
  const [entities, setEntities] = useState<HaEntities>({});
  const [overlay, setOverlay] = useState<Record<string, string>>({});
  const [pendingTemp, setPendingTemp] = useState<Record<string, number>>({});
  const [pendingBrightness, setPendingBrightness] = useState<Record<string, number>>({});
  const [toast, setToast] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);

  const flash = useCallback((message: string) => {
    setToast(message);
    setTimeout(() => setToast(t => (t === message ? null : t)), 3_000);
  }, []);

  const ingest = useCallback((next: HaEntities) => {
    setEntities(next);
    setOverlay(prev => {
      let changed = false;
      const out = { ...prev };
      for (const [id, value] of Object.entries(prev)) {
        if (next[id]?.state === value) {
          delete out[id];
          changed = true;
        }
      }
      return changed ? out : prev;
    });
    setPendingTemp(prev => {
      let changed = false;
      const out = { ...prev };
      for (const [id, target] of Object.entries(prev)) {
        if (numAttr(next[id]?.attributes['temperature']) === target) {
          delete out[id];
          changed = true;
        }
      }
      return changed ? out : prev;
    });
    setPendingBrightness(prev => {
      let changed = false;
      const out = { ...prev };
      for (const [id, target] of Object.entries(prev)) {
        const live = next[id];
        const livePct = live ? brightnessPct(live) : null;
        const settledOff = (!live || live.state !== 'on') && target === 0;
        if (settledOff || (livePct != null && Math.abs(livePct - target) <= 1)) {
          delete out[id];
          changed = true;
        }
      }
      return changed ? out : prev;
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    const timers = tempTimers.current;

    const boot = async () => {
      setMode({ kind: 'loading' });
      const [baseCfg, tokenCfg] = await Promise.all([
        client.config.get({ key: 'base_url' }),
        client.config.get({ key: 'token' }),
      ]);
      const baseUrl = baseCfg.ok ? baseCfg.response.value : null;
      const token = tokenCfg.ok ? tokenCfg.response.value : null;
      if (!baseUrl || !token) {
        if (!cancelled)
          setMode({
            kind: 'needs-config',
            message: 'set the Home Assistant URL and access token in the companion app.',
          });
        return;
      }

      const conn = new HaConnection({ client, baseUrl, token, onStatus: s => !cancelled && setStatus(s) });
      connRef.current = conn;
      await conn.open();
      try {
        await conn.whenReady();
      } catch {
        return;
      }
      if (cancelled) return;

      const doc = await client.doc.get({ key: SELECTION_KEY });
      const docValue = doc.ok ? doc.response.value : null;
      let ids: string[];
      if (docValue) {
        ids = splitIds(docValue);
      } else {
        const stored = await client.store.get({ key: SELECTION_KEY });
        ids = stored.ok && stored.response.value ? splitIds(stored.response.value) : [];
      }
      if (cancelled) return;

      if (ids.length === 0) {
        const all = await conn.getStates();
        if (!cancelled) setMode({ kind: 'picker', all });
      } else {
        setSelection(ids);
        await conn.subscribeEntities(ids, e => !cancelled && ingest(e));
        if (!cancelled) setMode({ kind: 'dashboard' });
      }
    };

    boot().catch(err => !cancelled && flash(err instanceof Error ? err.message : String(err)));

    const applyLive = (value: string | null) => {
      const conn = connRef.current;
      if (!conn || cancelled) return;
      const ids = value ? splitIds(value) : [];
      if (ids.length === 0) {
        conn
          .getStates()
          .then(all => !cancelled && setMode({ kind: 'picker', all }))
          .catch(e => flash(errText(e)));
        return;
      }
      setSelection(ids);
      setEntities({});
      setMode({ kind: 'dashboard' });
      conn.subscribeEntities(ids, e => !cancelled && ingest(e)).catch(e => flash(errText(e)));
    };

    const offChanged = client.config.onChanged(() => setReloadKey(k => k + 1));
    const offDoc = client.doc.onChanged(({ key, value }) => {
      if (key === SELECTION_KEY) applyLive(value);
    });
    return () => {
      cancelled = true;
      offChanged();
      offDoc();
      for (const t of timers.values()) clearTimeout(t);
      timers.clear();
      connRef.current?.close();
      connRef.current = null;
    };
  }, [client, ingest, flash, reloadKey]);

  const savePicker = useCallback(
    (ids: string[]) => {
      const conn = connRef.current;
      if (!conn) return;
      setSelection(ids);
      setEntities({});
      setMode({ kind: 'dashboard' });
      void (async () => {
        await client.store.put({ key: SELECTION_KEY, value: ids.join(',') });
        await conn.subscribeEntities(ids, ingest);
      })().catch(e => flash(errText(e)));
    },
    [client, ingest, flash],
  );

  const openPicker = useCallback(() => {
    const conn = connRef.current;
    if (!conn) return;
    conn
      .getStates()
      .then(all => setMode({ kind: 'picker', all }))
      .catch(e => flash(errText(e)));
  }, [flash]);

  const handleActivate = useCallback(
    (s: HaState) => {
      const conn = connRef.current;
      if (!conn) return;
      const kind = controlKind(s.entityId);
      if (kind === 'momentary') {
        const { domain, service } = momentaryCall(s.entityId);
        conn.callService(domain, service, {}, { entity_id: s.entityId }).catch(e => flash(errText(e)));
        return;
      }
      if (kind === 'toggle' || kind === 'lock') {
        const next = optimisticToggle(s);
        setOverlay(prev => ({ ...prev, [s.entityId]: next }));
        const { domain, service } = toggleCall(s);
        conn.callService(domain, service, {}, { entity_id: s.entityId }).catch(e => {
          setOverlay(prev => dropKey(prev, s.entityId));
          flash(errText(e));
        });
      }
    },
    [flash],
  );

  const handleSetTemp = useCallback(
    (entityId: string, target: number) => {
      const conn = connRef.current;
      if (!conn) return;
      setPendingTemp(prev => ({ ...prev, [entityId]: target }));
      const existing = tempTimers.current.get(entityId);
      if (existing) clearTimeout(existing);
      tempTimers.current.set(
        entityId,
        setTimeout(() => {
          tempTimers.current.delete(entityId);
          conn.callService('climate', 'set_temperature', { temperature: target }, { entity_id: entityId }).catch(e => {
            setPendingTemp(prev => dropKey(prev, entityId));
            flash(errText(e));
          });
        }, TEMP_DEBOUNCE_MS),
      );
    },
    [flash],
  );

  const handleSetBrightness = useCallback(
    (entityId: string, pct: number) => {
      const conn = connRef.current;
      if (!conn) return;
      setPendingBrightness(prev => ({ ...prev, [entityId]: pct }));
      setOverlay(prev => ({ ...prev, [entityId]: pct > 0 ? 'on' : 'off' }));
      const call =
        pct <= 0
          ? conn.callService('light', 'turn_off', {}, { entity_id: entityId })
          : conn.callService('light', 'turn_on', { brightness_pct: pct }, { entity_id: entityId });
      call.catch(e => {
        setPendingBrightness(prev => dropKey(prev, entityId));
        setOverlay(prev => dropKey(prev, entityId));
        flash(errText(e));
      });
    },
    [flash],
  );

  if (mode.kind === 'loading')
    return (
      <Center muted={status.kind === 'error'}>
        {status.kind === 'error' ? status.message : 'connecting to home assistant...'}
      </Center>
    );
  if (mode.kind === 'needs-config') return <Center muted>{mode.message}</Center>;
  if (mode.kind === 'picker')
    return (
      <Picker
        all={mode.all}
        initial={selection}
        onDone={savePicker}
        onCancel={selection.length ? () => setMode({ kind: 'dashboard' }) : undefined}
      />
    );

  const tiles = selection.map(id => mergeState(id, entities[id], overlay[id], pendingTemp[id], pendingBrightness[id]));
  return (
    <Dashboard
      tiles={tiles}
      status={status}
      toast={toast}
      onActivate={handleActivate}
      onSetTemp={handleSetTemp}
      onSetBrightness={handleSetBrightness}
      onOpenPicker={openPicker}
    />
  );
}

export type Tile = {
  entityId: string;
  state: HaState | null;
  pendingTemp: number | null;
  pendingBrightness: number | null;
};

function mergeState(
  id: string,
  live: HaState | undefined,
  overlayState: string | undefined,
  pending: number | undefined,
  pendingLight: number | undefined,
): Tile {
  if (!live) return { entityId: id, state: null, pendingTemp: pending ?? null, pendingBrightness: pendingLight ?? null };
  const state = overlayState ? { ...live, state: overlayState } : live;
  return { entityId: id, state, pendingTemp: pending ?? null, pendingBrightness: pendingLight ?? null };
}

function Center({ children, muted }: { children: React.ReactNode; muted?: boolean }) {
  return (
    <div className="flex h-full w-full items-center justify-center bg-bg px-10">
      <div className={`max-w-136 text-center font-mono text-row ${muted ? 'text-near' : 'text-off-white'}`}>
        {children}
      </div>
    </div>
  );
}

function splitIds(value: string): string[] {
  return value
    .split(',')
    .map(s => s.trim())
    .filter(Boolean);
}

function dropKey<T>(obj: Record<string, T>, key: string): Record<string, T> {
  if (!(key in obj)) return obj;
  const out = { ...obj };
  delete out[key];
  return out;
}

function numAttr(attr: unknown): number | null {
  return typeof attr === 'number' && Number.isFinite(attr) ? attr : null;
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

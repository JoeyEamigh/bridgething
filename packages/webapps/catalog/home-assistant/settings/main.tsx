import { closeSettings, ConfigForm, type ConfigFormHandle } from '@bridgething/client/form';
import { settings, type SettingsContext } from '@bridgething/client/settings';
import { useEffect, useMemo, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { domainOf, fetchStates, friendlyName, type HaEntity } from './ha-ws';
import { edited, fromDevice, NO_SELECTION, stored, toggled } from './selection';
import './style.css';

const SELECTION_KEY = 'selected_entities';

const PRIORITY_DOMAINS = ['light', 'switch', 'climate', 'sensor', 'media_player'];

type Conn =
  | { kind: 'idle' }
  | { kind: 'connecting' }
  | { kind: 'error'; message: string }
  | { kind: 'loaded'; entities: HaEntity[] };

function Settings() {
  const form = useRef<ConfigFormHandle>(null);
  const [ctx, setCtx] = useState<SettingsContext | null>(null);
  const [selection, setSelection] = useState(NO_SELECTION);
  const [query, setQuery] = useState('');
  const [conn, setConn] = useState<Conn>({ kind: 'idle' });
  const [status, setStatus] = useState('');

  useEffect(() => {
    const readSelection = async () => {
      try {
        const doc = await settings.doc.get(SELECTION_KEY);
        setSelection(held => fromDevice(held, doc.value));
      } catch (err) {
        setStatus(errText(err));
      }
    };
    settings.context().then(setCtx, (err: unknown) => setStatus(errText(err)));
    void readSelection();
    const offRefresh = settings.onRefresh(context => {
      setCtx(context);
      setStatus('');
      void readSelection();
    });
    const offDoc = settings.onDocChanged((key, value) => {
      if (key === SELECTION_KEY) setSelection(held => fromDevice(held, value));
    });
    return () => {
      offRefresh();
      offDoc();
    };
  }, []);

  async function connect() {
    setConn({ kind: 'connecting' });
    setStatus('');
    try {
      const values = await form.current?.flush();
      const baseUrl = values?.['base_url'] ?? '';
      const token = values?.['token'] ?? '';
      if (!baseUrl.trim() || !token.trim()) {
        setConn({ kind: 'error', message: 'enter the Home Assistant URL and a long-lived access token first.' });
        return;
      }
      const entities = await fetchStates(baseUrl, token);
      entities.sort((a, b) => friendlyName(a).localeCompare(friendlyName(b)));
      setConn({ kind: 'loaded', entities });
    } catch (err) {
      setConn({ kind: 'error', message: errText(err) });
    }
  }

  async function close() {
    try {
      await closeSettings(form.current);
    } catch (err) {
      setStatus(errText(err));
    }
  }

  async function saveSelection() {
    const ids = [...selection.picked];
    setStatus('saving selection to the device...');
    try {
      await settings.doc.set(SELECTION_KEY, ids.join(','));
      setSelection(held => stored(held, ids));
      setStatus(`saved ${ids.length} ${ids.length === 1 ? 'entity' : 'entities'} to the device`);
    } catch (err) {
      setStatus(errText(err));
    }
  }

  const groups = useMemo(() => {
    if (conn.kind !== 'loaded') return [];
    const q = query.trim().toLowerCase();
    const filtered = q
      ? conn.entities.filter(e => e.entityId.toLowerCase().includes(q) || friendlyName(e).toLowerCase().includes(q))
      : conn.entities;
    const byDomain = new Map<string, HaEntity[]>();
    for (const e of filtered) {
      const d = domainOf(e.entityId);
      const list = byDomain.get(d);
      if (list) list.push(e);
      else byDomain.set(d, [e]);
    }
    const order = [...byDomain.keys()].sort(domainOrder);
    return order.map(domain => ({ domain, entities: byDomain.get(domain)! }));
  }, [conn, query]);

  return (
    <main>
      <header>
        <h1>{ctx?.name ?? 'Home Assistant'} settings</h1>
        <p className="hint">
          {ctx
            ? `${selection.picked.size} selected${edited(selection) ? ', not saved yet' : ''}`
            : 'connecting to the companion host...'}
        </p>
      </header>

      <section className="creds">
        <ConfigForm ref={form} />
        <div className="row">
          <button type="button" onClick={connect} disabled={conn.kind === 'connecting'}>
            {conn.kind === 'connecting' ? 'connecting...' : conn.kind === 'loaded' ? 'reconnect' : 'connect'}
          </button>
          <span className="status">{status}</span>
        </div>
      </section>

      {conn.kind === 'error' && <p className="error">{conn.message}</p>}

      {conn.kind === 'loaded' && (
        <>
          <div className="search">
            <input
              type="search"
              placeholder="search entities..."
              value={query}
              onInput={e => setQuery((e.target as HTMLInputElement).value)}
            />
          </div>

          <div className="list">
            {groups.length === 0 && <p className="hint">no entities match "{query}".</p>}
            {groups.map(group => (
              <div className="group" key={group.domain}>
                <div className="group-head">{group.domain}</div>
                {group.entities.map(e => (
                  <label className="entity" key={e.entityId}>
                    <input
                      type="checkbox"
                      checked={selection.picked.has(e.entityId)}
                      onChange={() => setSelection(held => toggled(held, e.entityId))}
                    />
                    <span className="name">{friendlyName(e)}</span>
                    <span className="eid">{e.entityId}</span>
                  </label>
                ))}
              </div>
            ))}
          </div>
        </>
      )}

      <footer>
        {conn.kind === 'loaded' && (
          <button type="button" onClick={saveSelection}>
            Save selection
          </button>
        )}
        <button type="button" className="secondary" onClick={close}>
          Done
        </button>
      </footer>
    </main>
  );
}

function domainOrder(a: string, b: string): number {
  const ia = PRIORITY_DOMAINS.indexOf(a);
  const ib = PRIORITY_DOMAINS.indexOf(b);
  if (ia !== -1 || ib !== -1) return (ia === -1 ? Infinity : ia) - (ib === -1 ? Infinity : ib);
  return a.localeCompare(b);
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

createRoot(document.getElementById('root')!).render(<Settings />);

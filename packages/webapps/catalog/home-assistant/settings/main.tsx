import { settings, type SettingsContext } from '@bridgething/client/settings';
import { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { domainOf, fetchStates, friendlyName, HaAuthError, type HaEntity } from './ha-ws';
import './style.css';

const SELECTION_KEY = 'selected_entities';

const PRIORITY_DOMAINS = ['light', 'switch', 'climate', 'sensor', 'media_player'];

type Conn =
  | { kind: 'idle' }
  | { kind: 'connecting' }
  | { kind: 'error'; message: string }
  | { kind: 'loaded'; entities: HaEntity[] };

function splitIds(value: string | null): string[] {
  return (value ?? '')
    .split(',')
    .map(s => s.trim())
    .filter(Boolean);
}

function Settings() {
  const [ctx, setCtx] = useState<SettingsContext | null>(null);
  const [baseUrl, setBaseUrl] = useState('');
  const [token, setToken] = useState('');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState('');
  const [conn, setConn] = useState<Conn>({ kind: 'idle' });
  const [status, setStatus] = useState('');

  useEffect(() => {
    (async () => {
      try {
        setCtx(await settings.context());
        const entries = await settings.config.list();
        const byKey = Object.fromEntries(entries.map(e => [e.key, e.value]));
        setBaseUrl(byKey['base_url'] ?? '');
        setToken(byKey['token'] ?? '');
        const doc = await settings.doc.get(SELECTION_KEY);
        setSelected(new Set(splitIds(doc.value)));
      } catch (err) {
        setStatus(errText(err));
      }
    })();
  }, []);

  async function saveConfig() {
    setStatus('saving credentials...');
    try {
      await settings.config.set('base_url', baseUrl);
      await settings.config.set('token', token);
      setStatus('credentials saved');
    } catch (err) {
      setStatus(errText(err));
    }
  }

  async function connect() {
    if (!baseUrl.trim() || !token.trim()) {
      setConn({ kind: 'error', message: 'enter the Home Assistant URL and a long-lived access token first.' });
      return;
    }
    setConn({ kind: 'connecting' });
    setStatus('');
    await saveConfig();
    try {
      const entities = await fetchStates(baseUrl, token);
      entities.sort((a, b) => friendlyName(a).localeCompare(friendlyName(b)));
      setConn({ kind: 'loaded', entities });
    } catch (err) {
      setConn({ kind: 'error', message: err instanceof HaAuthError ? err.message : errText(err) });
    }
  }

  async function saveSelection() {
    setStatus('saving selection to the device...');
    try {
      await settings.doc.set(SELECTION_KEY, [...selected].join(','));
      setStatus(`saved ${selected.size} ${selected.size === 1 ? 'entity' : 'entities'} to the device`);
    } catch (err) {
      setStatus(errText(err));
    }
  }

  function toggle(id: string) {
    setSelected(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
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
          {ctx ? `${selected.size} selected on ${ctx.deviceId}` : 'connecting to the companion host...'}
        </p>
      </header>

      <section className="creds">
        <div className="field">
          <label htmlFor="base_url">Home Assistant URL</label>
          <input
            id="base_url"
            type="url"
            placeholder="https://your-home.ui.nabu.casa"
            value={baseUrl}
            onInput={e => setBaseUrl((e.target as HTMLInputElement).value)}
          />
        </div>
        <div className="field">
          <label htmlFor="token">Long-Lived Access Token</label>
          <input
            id="token"
            type="password"
            placeholder="paste the token from your HA profile"
            value={token}
            onInput={e => setToken((e.target as HTMLInputElement).value)}
          />
        </div>
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
                    <input type="checkbox" checked={selected.has(e.entityId)} onChange={() => toggle(e.entityId)} />
                    <span className="name">{friendlyName(e)}</span>
                    <span className="eid">{e.entityId}</span>
                  </label>
                ))}
              </div>
            ))}
          </div>

          <footer>
            <button type="button" onClick={saveSelection}>
              Save selection
            </button>
            <button type="button" className="secondary" onClick={() => settings.done()}>
              Done
            </button>
          </footer>
        </>
      )}

      {conn.kind !== 'loaded' && (
        <footer>
          <button type="button" className="secondary" onClick={() => settings.done()}>
            Done
          </button>
        </footer>
      )}
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

import {
  type BridgethingClient,
  type BridgeThingMeta,
  type BrightnessMode,
  type BrightnessState,
  type ConnectedDevice,
  type Device,
  type Diagnostics,
  type TimeInfo,
} from '@bridgething/client';
import { useEffect, useState } from 'react';

type Section = 'bluetooth' | 'display' | 'system' | 'power';

const SECTIONS: { id: Section; label: string }[] = [
  { id: 'bluetooth', label: 'bluetooth' },
  { id: 'display', label: 'display' },
  { id: 'system', label: 'system' },
  { id: 'power', label: 'power' },
];

export function Settings({ client, onClose }: { client: BridgethingClient; onClose: () => void }) {
  const [section, setSection] = useState<Section>('bluetooth');

  return (
    <div className="flex h-full w-full flex-col bg-bg">
      <header className="flex items-center gap-3 border-b border-rule px-4 pt-3 pb-2">
        <button
          type="button"
          onClick={onClose}
          aria-label="back to apps"
          className="flex size-9 items-center justify-center border border-rule text-near transition active:bg-neutral-soft">
          <BackIcon />
        </button>
        <div className="font-mono text-eyebrow tracking-[0.25em] text-dim uppercase">settings</div>
      </header>
      <div className="flex flex-1 overflow-hidden">
        <nav className="flex w-44 shrink-0 flex-col border-r border-rule py-2">
          {SECTIONS.map(s => (
            <button
              key={s.id}
              type="button"
              onClick={() => setSection(s.id)}
              className={`border-l-2 px-4 py-3 text-left font-mono text-row tracking-[0.06em] transition ${
                section === s.id
                  ? 'border-accent bg-accent-soft text-accent'
                  : 'border-transparent text-soft active:bg-neutral-soft'
              }`}>
              {s.label}
            </button>
          ))}
        </nav>
        <main className="flex-1 overflow-y-auto px-5 py-3 pb-6">
          {section === 'bluetooth' && <BluetoothPanel client={client} />}
          {section === 'display' && <DisplayPanel client={client} />}
          {section === 'system' && <SystemPanel client={client} />}
          {section === 'power' && <PowerPanel client={client} />}
        </main>
      </div>
    </div>
  );
}

function BluetoothPanel({ client }: { client: BridgethingClient }) {
  const [devices, setDevices] = useState<Device[]>([]);
  const [connectedMac, setConnectedMac] = useState<string | null>(null);
  const [alias, setAlias] = useState('');
  const [aliasPlaceholder, setAliasPlaceholder] = useState('Car Thing');
  const [aliasSaved, setAliasSaved] = useState(false);
  const [discoverable, setDiscoverable] = useState(false);
  const [busyMac, setBusyMac] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.bluetooth.list().then(r => {
      if (cancelled || !r.ok) return;
      setDevices(Object.values(r.response));
    });
    client.system.versionRequest().then(r => {
      if (cancelled || !r.ok) return;
      const tail = r.response.serialNumber.slice(-4);
      setAliasPlaceholder(tail ? `Car Thing (${tail})` : 'Car Thing');
    });
    return () => {
      cancelled = true;
    };
  }, [client]);

  useEffect(() => {
    const offDevices = client.bluetooth.onPairedDevices(map => setDevices(Object.values(map)));
    const offConnected = client.bluetooth.onConnectedDevice((d: ConnectedDevice) => setConnectedMac(d.mac));
    const offStatus = client.bluetooth.onStatus(s => {
      if (!s.connected) setConnectedMac(null);
    });
    const offInterface = client.bluetooth.onInterface(i => {
      setAliasPlaceholder(i.name);
    });
    return () => {
      offDevices();
      offConnected();
      offStatus();
      offInterface();
    };
  }, [client]);

  const saveAlias = async () => {
    const name = alias.trim();
    if (!name) return;
    await client.bluetooth.setAlias({ name });
    setAliasPlaceholder(name);
    setAlias('');
    setAliasSaved(true);
    setTimeout(() => setAliasSaved(false), 1600);
  };

  const toggleDiscoverable = async () => {
    if (discoverable) {
      await client.bluetooth.disableDiscoverable();
      setDiscoverable(false);
    } else {
      await client.bluetooth.enableDiscoverable();
      setDiscoverable(true);
    }
  };

  const reconnect = async (mac: string) => {
    setBusyMac(mac);
    await client.bluetooth.connect({ mac });
    setTimeout(() => setBusyMac(null), 1200);
  };

  const forget = async (mac: string) => {
    setBusyMac(mac);
    await client.bluetooth.forget({ mac });
    setBusyMac(null);
  };

  return (
    <div className="flex flex-col gap-5">
      <Card title="device name" hint="shown to phones when pairing">
        <div className="flex items-center gap-2">
          <input
            value={alias}
            onChange={e => setAlias(e.target.value)}
            placeholder={aliasPlaceholder}
            className="min-w-0 flex-1 border border-edge bg-bg px-3 py-2 text-row text-off-white outline-none placeholder:text-dim focus:border-accent"
          />
          <button
            type="button"
            onClick={saveAlias}
            disabled={!alias.trim()}
            className="border border-accent bg-accent px-5 py-2 font-mono text-hint text-screen transition active:opacity-80 disabled:border-rule disabled:bg-transparent disabled:text-dim">
            {aliasSaved ? 'saved' : 'save'}
          </button>
        </div>
      </Card>

      <Card title="pairing" hint="let a new phone find this device">
        <button
          type="button"
          onClick={toggleDiscoverable}
          className={`w-full border px-4 py-2.5 font-mono text-row transition ${
            discoverable ? 'border-accent bg-accent-soft text-accent' : 'border-edge text-near active:bg-neutral-soft'
          }`}>
          {discoverable ? 'discoverable' : 'make discoverable'}
        </button>
      </Card>

      <Card title="paired devices">
        {devices.length === 0 ? (
          <div className="py-2 font-mono text-hint text-dim">no paired devices.</div>
        ) : (
          <div className="flex flex-col gap-2">
            {devices.map(d => (
              <div key={d.id} className="flex items-center gap-3 border border-rule bg-bg px-3 py-2.5">
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-row font-medium text-off-white">{d.name || d.id}</span>
                    {connectedMac === d.id && (
                      <span className="border border-accent/30 bg-accent-soft px-2 py-0.5 font-mono text-eyebrow tracking-[0.12em] text-accent uppercase">
                        connected
                      </span>
                    )}
                  </div>
                  <div className="font-mono text-hint text-dim">{deviceLabel(d)}</div>
                </div>
                {connectedMac !== d.id && (
                  <button
                    type="button"
                    onClick={() => reconnect(d.id)}
                    disabled={busyMac === d.id}
                    className="border border-edge px-3 py-1.5 font-mono text-hint text-near transition active:bg-neutral-soft disabled:opacity-50">
                    reconnect
                  </button>
                )}
                <button
                  type="button"
                  onClick={() => forget(d.id)}
                  disabled={busyMac === d.id}
                  className="border border-err/40 bg-err-soft px-3 py-1.5 font-mono text-hint text-err transition active:opacity-80 disabled:opacity-50">
                  forget
                </button>
              </div>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}

function DisplayPanel({ client }: { client: BridgethingClient }) {
  const [brightness, setBrightness] = useState<BrightnessState | null>(null);
  const [ambient, setAmbient] = useState<number | null>(null);
  const [level, setLevel] = useState(1);

  useEffect(() => {
    let cancelled = false;
    client.hardware.stateGet().then(r => {
      if (cancelled || !r.ok) return;
      setBrightness(r.response.state.brightness);
      setLevel(r.response.state.brightness.level);
      setAmbient(r.response.state.ambientLevel);
    });
    return () => {
      cancelled = true;
    };
  }, [client]);

  useEffect(() => {
    const offBright = client.hardware.onBrightnessChanged(b => {
      setBrightness(b);
      setLevel(b.level);
    });
    const offAmbient = client.hardware.onAmbientLightUpdate(a => setAmbient(a.ambientLevel));
    return () => {
      offBright();
      offAmbient();
    };
  }, [client]);

  const setMode = async (mode: BrightnessMode) => {
    await client.hardware.displaySetMode({ mode });
    setBrightness(prev => (prev ? { ...prev, mode } : prev));
  };

  const commitLevel = async () => {
    await client.hardware.displaySetLevel({ level });
  };

  const mode = brightness?.mode ?? 'auto';

  return (
    <div className="flex flex-col gap-5">
      <Card title="brightness">
        <div className="flex gap-2">
          {(['auto', 'manual'] as BrightnessMode[]).map(m => (
            <button
              key={m}
              type="button"
              onClick={() => setMode(m)}
              className={`flex-1 border px-4 py-2.5 font-mono text-row transition ${
                mode === m ? 'border-accent bg-accent-soft text-accent' : 'border-rule text-soft active:bg-neutral-soft'
              }`}>
              {m}
            </button>
          ))}
        </div>
      </Card>

      {mode === 'manual' && (
        <Card title="level" hint={`${Math.round(level * 100)}%`}>
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={level}
            onChange={e => setLevel(Number(e.target.value))}
            onPointerUp={commitLevel}
            onTouchEnd={commitLevel}
            className="h-2 w-full accent-accent"
          />
        </Card>
      )}

      <Card title="ambient light" hint="from the light sensor">
        <Bar value={ambient ?? 0} max={100} label={ambient === null ? '...' : `${ambient}`} />
      </Card>

      {brightness && (
        <Card title="panel output">
          <Bar
            value={Math.round(brightness.effectiveLevel * 100)}
            max={100}
            label={`${Math.round(brightness.effectiveLevel * 100)}%`}
          />
        </Card>
      )}
    </div>
  );
}

function SystemPanel({ client }: { client: BridgethingClient }) {
  const [meta, setMeta] = useState<BridgeThingMeta | null>(null);
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [nickname, setNickname] = useState<string | null>(null);
  const [time, setTime] = useState<TimeInfo | null>(null);

  const refresh = () => {
    client.system.diagnosticsGet().then(r => r.ok && setDiag(r.response.diagnostics));
  };

  useEffect(() => {
    let cancelled = false;
    client.system.versionRequest().then(r => !cancelled && r.ok && setMeta(r.response));
    client.system.diagnosticsGet().then(r => !cancelled && r.ok && setDiag(r.response.diagnostics));
    client.system.deviceGetNickname().then(r => !cancelled && r.ok && setNickname(r.response.nickname));
    client.time.get().then(r => !cancelled && r.ok && setTime(r.response.time));
    return () => {
      cancelled = true;
    };
  }, [client]);

  return (
    <div className="flex flex-col gap-5">
      <Card title="device" hint="name is set from the companion app">
        <Rows
          rows={[
            ['name', nickname ?? meta?.modelName ?? '...'],
            ['model', meta?.modelName],
            ['serial', meta?.serialNumber],
            ['bluetooth', meta?.btMac],
          ]}
        />
      </Card>

      <Card title="software">
        <Rows
          rows={[
            ['daemon', meta?.appVersion],
            ['protocol', meta?.libbridgethingVersion],
            ['os', meta && `${meta.osName} ${meta.osVersion}`.trim()],
            ['kernel', diag?.kernelVersion],
          ]}
        />
      </Card>

      <Card title="health" action={{ label: 'refresh', onClick: refresh }}>
        <Rows
          rows={[
            ['uptime', diag && fmtUptime(diag.uptimeS)],
            ['memory', diag && `${fmtBytes(diag.memUsedBytes)} / ${fmtBytes(diag.memUsedBytes + diag.memAvailBytes)}`],
            [
              'storage',
              diag && `${fmtBytes(diag.diskUsedBytes)} / ${fmtBytes(diag.diskUsedBytes + diag.diskFreeBytes)}`,
            ],
            ['temperature', diag?.socTempC != null ? `${diag.socTempC.toFixed(1)}°C` : 'n/a'],
            ['load', diag && diag.loadAvg.map(n => n.toFixed(2)).join('  ')],
          ]}
        />
      </Card>

      <Card title="time">
        <Rows
          rows={[
            ['clock', fmtClock(time)],
            ['timezone', time?.tzIana ?? 'n/a'],
            ['locale', time?.locale ?? 'n/a'],
          ]}
        />
      </Card>
    </div>
  );
}

function PowerPanel({ client }: { client: BridgethingClient }) {
  return (
    <div className="flex flex-col gap-4">
      <ConfirmAction
        label="restart"
        hint="reboots the device"
        confirmLabel="tap again to restart"
        tone="neutral"
        onConfirm={() => client.system.reboot()}
      />
      <ConfirmAction
        label="power off"
        hint="shuts the device down"
        confirmLabel="tap again to power off"
        tone="neutral"
        onConfirm={() => client.system.powerOff()}
      />
      <ConfirmAction
        label="factory reset"
        hint="erases all settings, paired devices, and installed apps, then reboots"
        confirmLabel="tap again to erase everything"
        tone="danger"
        onConfirm={() => client.system.factoryReset()}
      />
    </div>
  );
}

function ConfirmAction({
  label,
  hint,
  confirmLabel,
  tone,
  onConfirm,
}: {
  label: string;
  hint: string;
  confirmLabel: string;
  tone: 'neutral' | 'danger';
  onConfirm: () => void;
}) {
  const [armed, setArmed] = useState(false);

  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(t);
  }, [armed]);

  const danger = tone === 'danger';

  return (
    <div className="border border-rule bg-screen p-4">
      <div className="mb-1 flex items-baseline justify-between gap-3">
        <span className={`font-mono text-eyebrow tracking-[0.18em] uppercase ${danger ? 'text-err' : 'text-dim'}`}>
          {label}
        </span>
      </div>
      <div className="mb-3 text-hint text-soft">{hint}</div>
      <button
        type="button"
        onClick={() => (armed ? onConfirm() : setArmed(true))}
        className={`w-full border px-4 py-2.5 font-mono text-row transition ${
          armed
            ? danger
              ? 'border-err bg-err text-off-white'
              : 'border-accent bg-accent text-screen'
            : danger
              ? 'border-err/40 bg-err-soft text-err'
              : 'border-edge text-near active:bg-neutral-soft'
        }`}>
        {armed ? confirmLabel : label}
      </button>
    </div>
  );
}

function Card({
  title,
  hint,
  action,
  children,
}: {
  title: string;
  hint?: string;
  action?: { label: string; onClick: () => void };
  children: React.ReactNode;
}) {
  return (
    <section className="border border-rule bg-screen p-4">
      <div className="mb-3 flex items-baseline justify-between gap-3">
        <div className="flex items-baseline gap-2">
          <h2 className="m-0 font-mono text-eyebrow tracking-[0.18em] text-dim uppercase">{title}</h2>
          {hint && <span className="text-hint text-soft">{hint}</span>}
        </div>
        {action && (
          <button
            type="button"
            onClick={action.onClick}
            className="font-mono text-hint text-accent transition active:opacity-70">
            {action.label}
          </button>
        )}
      </div>
      {children}
    </section>
  );
}

function Rows({ rows }: { rows: [string, string | null | undefined][] }) {
  return (
    <div className="flex flex-col gap-1.5">
      {rows.map(([k, v]) => (
        <div key={k} className="flex items-baseline justify-between gap-4">
          <span className="font-mono text-hint text-dim">{k}</span>
          <span className="truncate text-right font-mono text-hint text-near">{v ?? '...'}</span>
        </div>
      ))}
    </div>
  );
}

function Bar({ value, max, label }: { value: number; max: number; label: string }) {
  const pct = Math.min(100, Math.max(0, (value / max) * 100));
  return (
    <div className="flex items-center gap-3">
      <div className="h-1.5 flex-1 border border-rule bg-bg">
        <div className="h-full bg-accent transition-[width] duration-200" style={{ width: `${pct}%` }} />
      </div>
      <span className="w-12 text-right font-mono text-hint tabular-nums text-soft">{label}</span>
    </div>
  );
}

function BackIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
      <path d="M15 18l-6-6 6-6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function deviceLabel(d: Device): string {
  const type = d.type === 'unknown' ? '' : d.type;
  return [type, d.default ? 'default' : ''].filter(Boolean).join(' • ') || d.id;
}

function fmtBytes(n: number): string {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
  if (n >= 1024 ** 2) return `${Math.round(n / 1024 ** 2)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} KB`;
  return `${n} B`;
}

function fmtUptime(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h ${m}m`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

function fmtClock(time: TimeInfo | null): string {
  if (!time?.wallClockUnixS) return 'n/a';
  const d = new Date(time.wallClockUnixS * 1000);
  try {
    return d.toLocaleString(time.locale ?? undefined, {
      timeZone: time.tzIana ?? undefined,
      hour: '2-digit',
      minute: '2-digit',
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    });
  } catch {
    return d.toISOString();
  }
}

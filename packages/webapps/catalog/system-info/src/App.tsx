import { BridgethingClient } from '@bridgething/client';
import { daemonUrl } from '@bridgething/webapp-shared/daemon';
import { useEffect, useMemo, useState } from 'react';
import { formatBytes, formatUptime, loadPercent, parseStats, percentOf, type Stats } from './stats';

type Link = 'unknown' | 'absent' | 'present';

export default function App() {
  const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
  const [link, setLink] = useState<Link>('unknown');
  const [stats, setStats] = useState<Stats | null>(null);

  useEffect(() => {
    const apply = (available: boolean) => {
      setLink(available ? 'present' : 'absent');
      if (available) void client.forward.json({ type: 'refresh' });
    };
    const offStats = client.forward.onJson(message => {
      const next = parseStats(message);
      if (next) setStats(next);
    });
    const offCaps = client.capabilities.subscribePartial({
      snapshot: msg => apply(msg.capabilities.available.forward),
      update: msg => apply(msg.capabilities.available.forward),
    });
    void client.capabilities.get().then(reply => {
      if (reply.ok) apply(reply.response.capabilities.available.forward);
    });
    return () => {
      offStats();
      offCaps();
    };
  }, [client]);

  return (
    <div className="flex h-full w-full flex-col bg-bg px-10 py-8 text-off-white">
      {link !== 'present' ? (
        <Waiting
          title={link === 'unknown' ? 'connecting' : 'waiting for the desktop app'}
          detail={
            link === 'unknown'
              ? null
              : 'install this app from the bridgething desktop app on the computer you want to watch. it shows up here the moment that extension is running.'
          }
        />
      ) : stats ? (
        <Dashboard stats={stats} />
      ) : (
        <Waiting title="linked" detail="waiting for the first sample from the desktop app." />
      )}
    </div>
  );
}

function Waiting({ title, detail }: { title: string; detail: string | null }) {
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-3 text-center">
      <div className="font-display text-4xl tracking-display text-off-white">{title}</div>
      {detail ? <div className="max-w-xl font-mono text-row text-near">{detail}</div> : null}
    </div>
  );
}

function Dashboard({ stats }: { stats: Stats }) {
  const { host, cores, load, memory, uptimeSeconds, addresses } = stats;
  const cpu = loadPercent(load[0], cores);
  const ram = percentOf(memory.used, memory.total);
  const swap = memory.swapTotal > 0 ? percentOf(memory.swapTotal - memory.swapFree, memory.swapTotal) : null;
  const cached = memory.cached === null ? '' : ` · ${formatBytes(memory.cached)} cached`;

  return (
    <>
      <div className="flex items-end justify-between border-b border-rule pb-4">
        <div className="min-w-0">
          <div className="truncate font-display text-5xl font-medium tracking-wordmark text-off-white">{host.name}</div>
          <div className="mt-1 font-mono text-hint tracking-[0.08em] text-dim uppercase">
            {host.os} {host.arch} · {host.release} · {cores} cores
          </div>
        </div>
        <div className="shrink-0 text-right">
          <div className="font-mono text-eyebrow tracking-[0.18em] text-dim uppercase">uptime</div>
          <div className="font-mono text-title tabular-nums text-near">{formatUptime(uptimeSeconds)}</div>
        </div>
      </div>

      <div className="grid flex-1 grid-cols-3 gap-4 py-5">
        <Tile
          label="cpu load"
          percent={cpu}
          detail={`${load[0].toFixed(2)} now · ${load[1].toFixed(2)} 5m · ${load[2].toFixed(2)} 15m`}
        />
        <Tile
          label="memory"
          percent={ram}
          detail={`${formatBytes(memory.used)} of ${formatBytes(memory.total)}${cached}`}
        />
        <Tile
          label="swap"
          percent={swap}
          detail={
            swap === null
              ? 'none configured'
              : `${formatBytes(memory.swapTotal - memory.swapFree)} of ${formatBytes(memory.swapTotal)}`
          }
        />
      </div>

      <div className="flex gap-6 border-t border-rule pt-4 font-mono text-hint tabular-nums text-near">
        {addresses.length === 0 ? (
          <span className="text-dim">no network address</span>
        ) : (
          addresses.map(nic => (
            <span key={`${nic.name}-${nic.address}`}>
              <span className="text-dim">{nic.name} </span>
              {nic.address}
            </span>
          ))
        )}
      </div>
    </>
  );
}

function Tile({ label, percent, detail }: { label: string; percent: number | null; detail: string }) {
  const tone = percent === null ? 'bg-dim' : percent >= 85 ? 'bg-warn' : 'bg-accent';
  return (
    <div className="flex flex-col justify-between border border-rule bg-screen p-5">
      <div className="font-mono text-eyebrow tracking-[0.18em] text-dim uppercase">{label}</div>
      <div className="font-display text-6xl font-medium tracking-wordmark tabular-nums text-off-white">
        {percent === null ? '--' : `${percent}%`}
      </div>
      <div>
        <div className="h-2 w-full bg-neutral-soft">
          <div className={`h-full ${tone}`} style={{ width: `${percent ?? 0}%` }} />
        </div>
        <div className="mt-2 font-mono text-hint tabular-nums text-near">{detail}</div>
      </div>
    </div>
  );
}

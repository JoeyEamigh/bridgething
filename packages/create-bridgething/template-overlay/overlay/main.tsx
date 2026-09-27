import type { BluetoothPin, Notification, PeerSnapshotMap, PhoneCall, VolumeChanged } from '@bridgething/client';
import { BridgethingClient } from '@bridgething/client';
import { render } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import css from './style.css?inline';

type OverlaySurfaces = {
  notifications: boolean;
  call: boolean;
  pairing: boolean;
  connection: boolean;
  volume: boolean;
};

type OverlayConfig = { origin: string; url: string; surfaces: OverlaySurfaces };

declare global {
  interface Window {
    __bridgethingOverlay?: OverlayConfig;
    __bridgethingOverlayMounted?: boolean;
  }
}

const TOAST_TTL_MS = 5_000;
const VOLUME_TTL_MS = 1_500;
const CONNECTION_SHOW_DELAY_MS = 3_000;

function useLatest<T>(
  subscribe: (emit: (value: T | null) => void) => () => void,
  ttl?: number,
  deps: unknown[] = [],
): T | null {
  const [value, setValue] = useState<T | null>(null);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = subscribe(next => {
      clearTimeout(timer);
      setValue(next);
      if (next !== null && ttl !== undefined) timer = setTimeout(() => setValue(null), ttl);
    });
    return () => {
      clearTimeout(timer);
      off();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return value;
}

const CHIP = 'absolute left-1/2 -translate-x-1/2 rounded-full bg-chip px-4 py-2 text-sm text-white';

function ConnectionBanner({ client }: { client: BridgethingClient }) {
  const [away, setAway] = useState(false);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = client.peer.onSnapshot((snapshot: PeerSnapshotMap) => {
      const peers = Object.values(snapshot);
      const paired = peers.some(p => p.paired);
      const useful = peers.some(p => p.iap2 === 'identified' || p.companion.type === 'connected');
      if (paired && !useful) {
        timer ??= setTimeout(() => setAway(true), CONNECTION_SHOW_DELAY_MS);
      } else {
        clearTimeout(timer);
        timer = undefined;
        setAway(false);
      }
    });
    return () => {
      clearTimeout(timer);
      off();
    };
  }, [client]);

  if (!away) return null;
  return <div class={`${CHIP} top-3`}>phone disconnected</div>;
}

function CallBanner({ client, onDismissible }: { client: BridgethingClient; onDismissible: Dismissible }) {
  const [call, setCall] = useState<PhoneCall | null>(null);

  useEffect(() => {
    const offs = [
      client.phone.onCallStarted(setCall),
      client.phone.onCallUpdated(next => setCall(prev => (prev === null || prev.callId === next.callId ? next : prev))),
      client.phone.onCallEnded(ended => setCall(prev => (prev?.callId === ended.callId ? null : prev))),
    ];
    return () => offs.forEach(off => off());
  }, [client]);

  useEffect(() => (call ? onDismissible(() => setCall(null)) : undefined), [call, onDismissible]);

  if (!call) return null;
  return (
    <div class={`${CHIP} top-3`}>
      {call.displayName || call.remoteId || 'unknown caller'} - {call.status}
    </div>
  );
}

function Toasts({ client }: { client: BridgethingClient }) {
  const [live, setLive] = useState<Notification[]>([]);

  useEffect(() => {
    const timers = new Map<string, ReturnType<typeof setTimeout>>();
    const drop = (id: string) => {
      clearTimeout(timers.get(id));
      timers.delete(id);
      setLive(prev => prev.filter(n => n.id !== id));
    };
    const post = (n: Notification) => {
      if (n.flags.silent) return;
      clearTimeout(timers.get(n.id));
      timers.set(
        n.id,
        setTimeout(() => drop(n.id), TOAST_TTL_MS),
      );
      setLive(prev => [...prev.filter(other => other.id !== n.id), n]);
    };
    const offs = [
      client.notifications.onPosted(post),
      client.notifications.onUpdated(post),
      client.notifications.onRemoved(removed => drop(removed.id)),
    ];
    return () => {
      timers.forEach(clearTimeout);
      offs.forEach(off => off());
    };
  }, [client]);

  return (
    <div class="absolute top-0 right-0 flex flex-col items-end gap-3 p-3">
      {live.map(n => (
        <div key={n.id} class="w-65 rounded-xl bg-card px-3 py-2.5 text-white">
          <div class="text-[11px] opacity-60">{n.app.displayName ?? n.app.bundleId}</div>
          <div class="text-sm font-semibold">{n.title ?? ''}</div>
          {(n.message ?? n.subtitle) ? <div class="text-xs opacity-80">{n.message ?? n.subtitle}</div> : null}
        </div>
      ))}
    </div>
  );
}

function PairingModal({ client, onDismissible }: { client: BridgethingClient; onDismissible: Dismissible }) {
  const [pin, setPin] = useState<BluetoothPin | null>(null);

  useEffect(() => {
    const offs = [client.bluetooth.onPin(setPin), client.bluetooth.onPairingResult(() => setPin(null))];
    return () => offs.forEach(off => off());
  }, [client]);

  useEffect(() => (pin ? onDismissible(() => setPin(null)) : undefined), [pin, onDismissible]);

  if (!pin) return null;
  return (
    <div class="pointer-events-auto absolute inset-0 grid place-items-center bg-scrim text-center text-white">
      <div>
        <div class="text-[13px] opacity-70">enter this pin on your phone</div>
        <div class="my-3 text-5xl font-bold tracking-[0.1em]">{pin.pin}</div>
        <div class="text-xs opacity-60">{pin.name || pin.mac}</div>
      </div>
    </div>
  );
}

function VolumeChip({ client }: { client: BridgethingClient }) {
  const volume = useLatest<VolumeChanged>(emit => client.audio.onVolumeChanged(emit), VOLUME_TTL_MS, [client]);
  if (!volume) return null;
  const percent = Math.round(Math.min(1, Math.max(0, volume.level)) * 100);
  return <div class={`${CHIP} bottom-6`}>{volume.muted ? 'muted' : `${percent}%`}</div>;
}

type Dismissible = (hide: () => void) => () => void;

function Overlay({ cfg, client }: { cfg: OverlayConfig; client: BridgethingClient }) {
  const [stack] = useState<Array<() => void>>([]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || stack.length === 0) return;
      event.stopPropagation();
      event.preventDefault();
      stack.pop()?.();
    };
    document.addEventListener('keydown', onKey, { capture: true });
    return () => document.removeEventListener('keydown', onKey, { capture: true });
  }, [stack]);

  const dismissible: Dismissible = hide => {
    stack.push(hide);
    return () => {
      const at = stack.indexOf(hide);
      if (at >= 0) stack.splice(at, 1);
    };
  };

  return (
    <>
      <style>{css}</style>
      <div class="absolute inset-0 font-sans">
        {cfg.surfaces.connection && <ConnectionBanner client={client} />}
        {cfg.surfaces.call && <CallBanner client={client} onDismissible={dismissible} />}
        {cfg.surfaces.notifications && <Toasts client={client} />}
        {cfg.surfaces.volume && <VolumeChip client={client} />}
        {cfg.surfaces.pairing && <PairingModal client={client} onDismissible={dismissible} />}
      </div>
    </>
  );
}

function boot() {
  const cfg = window.__bridgethingOverlay;
  if (!cfg || window.__bridgethingOverlayMounted) return;
  window.__bridgethingOverlayMounted = true;

  const mount = () => {
    const host = document.createElement('div');
    host.style.cssText = 'position:fixed;inset:0;z-index:2147483647;pointer-events:none';
    const shadow = host.attachShadow({ mode: 'closed' });
    document.body.appendChild(host);
    render(<Overlay cfg={cfg} client={new BridgethingClient({ url: cfg.url })} />, shadow);
  };

  if (document.body) mount();
  else document.addEventListener('DOMContentLoaded', mount, { once: true });
}

boot();

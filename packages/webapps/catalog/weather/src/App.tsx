import { BridgethingClient, type NetFetchReply } from '@bridgething/client';
import { daemonUrl } from '@bridgething/webapp-shared/daemon';
import { useEffect, useMemo, useState } from 'react';

type Coords = { lat: number; lon: number; label: string };

type Forecast = {
  current: { temp: number; code: number; wind: number; humidity: number };
  unit: string;
  days: { date: string; code: number; hi: number; lo: number }[];
};

type Phase = { kind: 'loading' } | { kind: 'ready'; data: Forecast } | { kind: 'error'; message: string };

// burn-in protection: drift the whole content a few pixels every minute
// (mirrors the Glassy Overlay ambient dashboard)
const BURNIN_STEPS = [
  { x: 0, y: 0 },
  { x: 5, y: 3 },
  { x: -4, y: 5 },
  { x: -5, y: -3 },
  { x: 4, y: -5 },
];
const BURNIN_SHIFT_MS = 60_000;

export default function App() {
  const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
  const [phase, setPhase] = useState<Phase>({ kind: 'loading' });
  const [shift, setShift] = useState(BURNIN_STEPS[0]);

  useEffect(() => {
    let i = 0;
    const t = window.setInterval(() => {
      i = (i + 1) % BURNIN_STEPS.length;
      setShift(BURNIN_STEPS[i]);
    }, BURNIN_SHIFT_MS);
    return () => window.clearInterval(t);
  }, []);

  useEffect(() => {
    let cancelled = false;

    const load = async () => {
      if (!cancelled) setPhase({ kind: 'loading' });
      try {
        const [unitsCfg, locCfg] = await Promise.all([
          client.config.get({ key: 'units' }),
          client.config.get({ key: 'location' }),
        ]);
        const units = (unitsCfg.ok && unitsCfg.response.value) || 'metric';
        const locStr = locCfg.ok ? locCfg.response.value : null;

        const coords = parseLatLon(locStr) ?? (await geoCoords(client));
        if (!coords) {
          if (!cancelled)
            setPhase({
              kind: 'error',
              message: 'no location - please set in companion app or allow gps',
            });
          return;
        }

        const data = await fetchForecast(client, coords, units);
        if (!cancelled) setPhase({ kind: 'ready', data });
      } catch (err) {
        if (!cancelled) setPhase({ kind: 'error', message: err instanceof Error ? err.message : String(err) });
      }
    };

    load();
    const intervalId = window.setInterval(load, 15 * 60 * 1000);
    const offChanged = client.config.onChanged(() => load());
    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
      offChanged();
    };
  }, [client]);

  return (
    <div className="flex h-full w-full flex-col bg-bg px-10 py-8 text-off-white">
      <div
        className="flex h-full w-full flex-col"
        style={{
          transform: `translate(${shift.x}px, ${shift.y}px)`,
          transition: 'transform 2.5s ease-in-out',
        }}
      >
        {phase.kind === 'loading' && <Centered>loading weather...</Centered>}
        {phase.kind === 'error' && <Centered tone="muted">{phase.message}</Centered>}
        {phase.kind === 'ready' && <ForecastView data={phase.data} />}
      </div>
    </div>
  );
}

function ForecastView({ data }: { data: Forecast }) {
  const { current, days, unit } = data;
  const c = wmo(current.code);
  return (
    <>
      <div className="flex flex-1 items-center gap-10">
        <div className="text-[7rem] leading-none">{c.icon}</div>
        <div className="flex flex-col gap-1">
          <div className="font-display text-7xl font-medium tracking-wordmark tabular-nums text-off-white">
            {Math.round(current.temp)}
            {unit}
          </div>
          <div className="font-mono text-title tracking-[0.06em] text-near lowercase">{c.label}</div>
          <div className="mt-2 font-mono text-hint tracking-[0.08em] text-dim uppercase">
            humidity {current.humidity}% &nbsp;/&nbsp; wind {Math.round(current.wind)}
          </div>
        </div>
      </div>
      <div className="flex gap-3 border-t border-rule pt-4">
        {days.slice(0, 5).map(d => {
          const dc = wmo(d.code);
          return (
            <div key={d.date} className="flex flex-1 flex-col items-center gap-1 border border-rule bg-screen py-3">
              <div className="font-mono text-eyebrow tracking-[0.18em] text-dim uppercase">{weekday(d.date)}</div>
              <div className="text-3xl">{dc.icon}</div>
              <div className="font-mono text-row tabular-nums">
                <span className="text-off-white">{Math.round(d.hi)}</span>
                <span className="text-dim"> / {Math.round(d.lo)}</span>
              </div>
            </div>
          );
        })}
      </div>
    </>
  );
}

function Centered({ children, tone }: { children: React.ReactNode; tone?: 'muted' }) {
  return (
    <div className="flex h-full w-full items-center justify-center">
      <div className={`max-w-lg text-center font-mono text-row ${tone === 'muted' ? 'text-near' : 'text-off-white'}`}>
        {children}
      </div>
    </div>
  );
}

function parseLatLon(s: string | null): Coords | null {
  if (!s) return null;
  const m = s.split(',').map(p => Number(p.trim()));
  if (m.length !== 2 || !Number.isFinite(m[0]) || !Number.isFinite(m[1])) return null;
  return { lat: m[0], lon: m[1], label: s.trim() };
}

async function geoCoords(client: BridgethingClient): Promise<Coords | null> {
  const r = await client.geo.getOnce({ accuracy: 'coarse' });
  if (!r.ok) return null;
  const { lat, lon } = r.response.position;
  return { lat, lon, label: 'current location' };
}

async function fetchForecast(client: BridgethingClient, coords: Coords, units: string): Promise<Forecast> {
  const imperial = units === 'imperial';
  const params = new URLSearchParams({
    latitude: String(coords.lat),
    longitude: String(coords.lon),
    current: 'temperature_2m,relative_humidity_2m,weather_code,wind_speed_10m',
    daily: 'weather_code,temperature_2m_max,temperature_2m_min',
    timezone: 'auto',
    temperature_unit: imperial ? 'fahrenheit' : 'celsius',
    wind_speed_unit: imperial ? 'mph' : 'kmh',
    forecast_days: '6',
  });
  const url = `https://api.open-meteo.com/v1/forecast?${params.toString()}`;

  const res = await client.net.fetch({
    request: { url, method: 'GET', headers: [], body: null, timeoutMs: 12_000, redirect: 'follow' },
  });
  if (!res.ok) throw new Error(netErrorMessage(res));

  const reply = res.response as NetFetchReply;
  if (reply.response.status >= 400) throw new Error(`weather api returned ${reply.response.status}`);
  const text = new TextDecoder().decode(new Uint8Array(reply.response.body as unknown as number[]));
  const json = JSON.parse(text);

  return {
    unit: imperial ? '°F' : '°C',
    current: {
      temp: json.current.temperature_2m,
      code: json.current.weather_code,
      wind: json.current.wind_speed_10m,
      humidity: json.current.relative_humidity_2m,
    },
    days: (json.daily.time as string[]).map((date, i) => ({
      date,
      code: json.daily.weather_code[i],
      hi: json.daily.temperature_2m_max[i],
      lo: json.daily.temperature_2m_min[i],
    })),
  };
}

function netErrorMessage(res: { kind: 'domain' | 'protocol'; error: unknown }): string {
  if (res.kind === 'domain') return 'no network - please connect your phone';
  return 'network request failed.';
}

function weekday(isoDate: string): string {
  const d = new Date(`${isoDate}T00:00:00`);
  return d.toLocaleDateString(undefined, { weekday: 'short' });
}

function wmo(code: number): { label: string; icon: string } {
  if (code === 0) return { label: 'clear', icon: '☀️' };
  if (code <= 2) return { label: 'partly cloudy', icon: '⛅' };
  if (code === 3) return { label: 'overcast', icon: '☁️' };
  if (code <= 48) return { label: 'fog', icon: '🌫️' };
  if (code <= 57) return { label: 'drizzle', icon: '🌦️' };
  if (code <= 67) return { label: 'rain', icon: '🌧️' };
  if (code <= 77) return { label: 'snow', icon: '🌨️' };
  if (code <= 82) return { label: 'showers', icon: '🌧️' };
  if (code <= 86) return { label: 'snow showers', icon: '🌨️' };
  return { label: 'thunderstorm', icon: '⛈️' };
}

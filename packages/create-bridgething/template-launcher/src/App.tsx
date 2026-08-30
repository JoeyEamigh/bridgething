import { BridgethingClient, type WebappInfo } from '@bridgething/client';
import { useEffect, useMemo, useState } from 'react';
import { daemonUrl } from './daemon';

type Entry = {
  info: WebappInfo;
  iconUrl: string | null;
};

export default function App() {
  const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activating, setActivating] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const revoke: string[] = [];

    const load = async () => {
      const [list, current] = await Promise.all([client.webapp.list(), client.webapp.current()]);
      if (!list.ok) {
        setError('failed to list webapps');
        return;
      }
      const selfId = current.ok ? current.response.id : null;
      const visible = list.response.webapps.filter(w => w.id !== selfId);
      const loaded = await Promise.all(
        visible.map(async (info): Promise<Entry> => {
          if (!info.iconHash) return { info, iconUrl: null };
          const icon = await client.webapp.icon({ id: info.id });
          if (!icon.ok) return { info, iconUrl: null };
          const bytes = new Uint8Array(icon.response.bytes as unknown as number[]);
          const url = URL.createObjectURL(
            new Blob([bytes], { type: icon.response.mime ?? 'application/octet-stream' }),
          );
          revoke.push(url);
          return { info, iconUrl: url };
        }),
      );
      if (!cancelled) setEntries(loaded);
    };

    load().catch(err => setError(err instanceof Error ? err.message : String(err)));

    const offInstalled = client.webapp.onWebappInstalled(() => void load());
    const offUninstalled = client.webapp.onWebappUninstalled(() => void load());

    return () => {
      cancelled = true;
      offInstalled();
      offUninstalled();
      for (const url of revoke) URL.revokeObjectURL(url);
    };
  }, [client]);

  const activate = async (id: string) => {
    if (activating) return;
    setActivating(id);
    const result = await client.webapp.activate({ id });
    if (!result.ok) setActivating(null);
  };

  if (error) return <Centered>{error}</Centered>;
  if (!entries) return <Centered>loading</Centered>;
  if (entries.length === 0) return <Centered>no apps installed</Centered>;

  return (
    <div className="h-full w-full overflow-y-auto p-6">
      <div className="grid grid-cols-4 gap-4">
        {entries.map(({ info, iconUrl }) => (
          <button
            key={info.id}
            type="button"
            disabled={activating !== null}
            onClick={() => activate(info.id)}
            className="flex flex-col items-center gap-2 p-3 disabled:opacity-50">
            <div className="grid h-20 w-20 place-items-center border">
              {iconUrl ? (
                <img src={iconUrl} alt="" className="h-full w-full object-contain" draggable={false} />
              ) : (
                <span className="text-2xl">{(info.name.trim().charAt(0) || '?').toUpperCase()}</span>
              )}
            </div>
            <span className="text-center text-sm">{info.name}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return <div className="grid h-full w-full place-items-center text-sm">{children}</div>;
}

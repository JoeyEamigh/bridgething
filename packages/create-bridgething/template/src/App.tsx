import { BridgethingClient, type ConnectionState, type PlayerState } from '@bridgething/client';
import { useEffect, useMemo, useState } from 'react';
import { daemonUrl } from './daemon';

export default function App() {
  const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
  const [conn, setConn] = useState<ConnectionState>(client.connectionState);
  const [state, setState] = useState<PlayerState | null>(null);
  const [artUrl, setArtUrl] = useState<string | null>(null);

  useEffect(() => {
    const offConn = client.on(event => {
      if (event.type === 'open' || event.type === 'close' || event.type === 'connecting') {
        setConn(client.connectionState);
      }
    });
    const offSnapshot = client.player.onSnapshot(reply => setState(reply.state));
    client.player.stateGet().then(r => r.ok && setState(r.response.state));
    return () => {
      offConn();
      offSnapshot();
    };
  }, [client]);

  const track = state?.track ?? null;
  const artworkId = track?.artworkId ?? null;

  useEffect(() => {
    if (!artworkId) {
      setArtUrl(null);
      return;
    }
    let revoked = false;
    let blobUrl: string | null = null;
    (async () => {
      const result = await client.asset.get({ id: artworkId, requestId: crypto.randomUUID() });
      if (revoked) return;
      if (result.ok) {
        const bytes = new Uint8Array(result.response.bytes as unknown as number[]);
        blobUrl = URL.createObjectURL(new Blob([bytes], { type: result.response.mime ?? 'image/jpeg' }));
        setArtUrl(blobUrl);
      }
    })();
    return () => {
      revoked = true;
      if (blobUrl) URL.revokeObjectURL(blobUrl);
    };
  }, [client, artworkId]);

  const playing = state?.playback.state === 'playing';

  return (
    <div className="flex h-full w-full items-center justify-center gap-12 bg-bg p-12 text-off-white">
      {artUrl ? (
        <img src={artUrl} alt="" className="h-full max-h-96 w-auto border border-rule" />
      ) : (
        <div className="grid h-96 w-96 place-items-center border border-rule bg-screen font-mono text-body text-dim">
          {conn === 'open' ? 'no track' : conn}
        </div>
      )}
      <div className="flex flex-col gap-3">
        <div className="font-mono text-eyebrow tracking-[0.25em] text-dim uppercase">{conn}</div>
        {track ? (
          <>
            <div className="font-display text-3xl font-medium leading-tight tracking-display">
              {track.title ?? 'unknown'}
            </div>
            <div className="text-xl text-soft">{track.artist ?? ''}</div>
            <div className="font-mono text-hint text-dim">{track.album ?? ''}</div>
            <div className="mt-6 flex gap-4">
              <button
                className="border border-edge px-6 py-3 font-mono text-row text-near transition active:bg-neutral-soft"
                onClick={() => client.player.skipPrev({ allowSeeking: true })}>
                ◀◀
              </button>
              <button
                className="border border-accent bg-accent px-6 py-3 font-mono text-row text-screen transition active:opacity-80"
                onClick={() => (playing ? client.player.pause() : client.player.resume())}>
                {playing ? '❚❚' : '▶'}
              </button>
              <button
                className="border border-edge px-6 py-3 font-mono text-row text-near transition active:bg-neutral-soft"
                onClick={() => client.player.skipNext()}>
                ▶▶
              </button>
            </div>
          </>
        ) : (
          <div className="text-title text-soft">connect a phone to see now playing</div>
        )}
      </div>
    </div>
  );
}

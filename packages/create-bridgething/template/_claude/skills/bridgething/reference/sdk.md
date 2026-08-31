# The webapp SDK (`@bridgething/client`)

## Read the types

The shipped declarations carry a doc comment on every method, event, and field.

- `node_modules/@bridgething/client/dist/dispatch.generated.d.ts` has every
  `client.<surface>` method and event with its argument and return types.
- `node_modules/@bridgething/lib/dist/bindings/client.d.ts` has the request and
  reply payloads. `shared.d.ts` has the data types.

```bash
grep -n 'class PlayerSurface' -A40 node_modules/@bridgething/client/dist/dispatch.generated.d.ts
grep -n 'export type PlayerState ' -A12 node_modules/@bridgething/lib/dist/bindings/shared.d.ts
```

## Connect

```ts
import { BridgethingClient } from '@bridgething/client';

import { daemonUrl } from './daemon';

const client = new BridgethingClient({ url: daemonUrl() });
```

Construct once and reuse, a `useMemo` in React. Watch the link with
`client.on(e => ...)`, where `e.type` is `open`, `close`, `connecting`, or
`message`, and read `client.connectionState`, one of
`'connecting' | 'open' | 'closing' | 'closed'`.

## The three call shapes

Events. `onXxx` returns an unsubscribe function to call in cleanup.
`subscribe({...})` takes several at once.

```ts
const off = client.player.onSnapshot(reply => setState(reply.state));
// ...later
off();
```

Requests. Check `.ok` on every result.

```ts
const res = await client.player.stateGet();
if (res.ok) console.log(res.response.state);
else console.warn(res.kind, res.error); // kind: 'domain' | 'protocol'
```

Commands. The returned `Promise<void>` resolves when the daemon has taken the
message, not when the phone finished acting.

```ts
await client.player.skipNext();
```

## Cookbook

`client.player`:

```ts
client.player.onSnapshot(r => setState(r.state)); // full PlayerState on every material change
client.player.stateGet().then(r => r.ok && setState(r.response.state)); // prime on mount

// PlayerState: { track: MediaItem | null, playback: Playback, queue, options, context }
// track.title / track.artist / track.album / track.artworkId / track.durationMs
// playback.state: 'stopped' | 'paused' | 'playing'; playback.positionMs; playback.shuffle

client.player.play({ uri: 'spotify:track:...' });
client.player.pause();
client.player.resume();
client.player.skipNext();
client.player.skipPrev({ allowSeeking: true }); // true restarts the track if it is progressed
client.player.seekTo({ positionMs: 30_000 });
client.player.setShuffle({ on: true });
client.player.setRepeat({ mode: 'all' }); // 'off' | 'all' | 'one'
```

`onSnapshot` fires on material changes such as a track change, play, pause, and
seek. For a smooth progress bar, extrapolate the playhead between snapshots from
`playback.state` and `playback.positionMs`.

`client.asset`. Player state carries opaque asset ids:

```ts
const res = await client.asset.get({ id: track.artworkId, requestId: crypto.randomUUID() });
if (res.ok) {
  const bytes = new Uint8Array(res.response.bytes as unknown as number[]);
  const url = URL.createObjectURL(new Blob([bytes], { type: res.response.mime ?? 'image/jpeg' }));
  // set <img src={url}>, and URL.revokeObjectURL(url) on cleanup
}
```

`client.store`, per-webapp key and value that survives restarts:

```ts
await client.store.put({ key: 'theme', value: 'dark' });
const r = await client.store.get({ key: 'theme' }); // r.ok && r.response.value
await client.store.delete({ key: 'theme' });
```

`client.config`, the values declared in `manifest.json`. The companion app writes
them, the webapp reads them:

```ts
client.config.onChanged(c => applySetting(c.key, c.value));
const r = await client.config.get({ key: 'units' });
const all = await client.config.list();
```

The settings page under `settings/` reads and writes them through
`@bridgething/client/settings`, a `postMessage` bridge. It runs on the phone with
real internet, but loads from a `file://` origin and the webview enforces CORS on
fetch and XHR. WebSocket APIs work. Plain HTTP APIs work when the server sends
permissive CORS headers, since requests arrive with `Origin: null`. For a
CORS-strict HTTP-only service, fetch through `client.net` instead.

Write it in react like the rest of the app. `react` and `react-dom/client`
imports, `className`, `htmlFor`, react's event types. The build runs it on
preact through `@preact/preset-vite`, which is most of what keeps the page
small, so keep the react-flavoured source and leave the preset alone. One
tsconfig covers `src`, `settings` and `extension`, so `bun run typecheck`
checks all three.

Keep the page small. It builds to one self-contained HTML file, and the device
refuses to install a bundle whose declared settings page is over 1 MiB, failing
the whole install with an invalid-manifest error. The build warns past 200 KiB
and again past 500 KiB. A heavy dependency in `settings/` is the usual way
there.

`client.net` tunnels HTTP, websockets, and a SOCKS proxy through the phone. The
SOCKS proxy needs `"net.proxy"` in the manifest's `permissions`. Read the
`NetSurface` block in the `.d.ts` for the request shapes.

`client.library` is Spotify-backed: `browse`, `search`, `recommendations`,
`favoritesList`, and `favoritesContains` are requests; `favoritesToggle` and
`favoritesSet` are commands; `onFavoriteChanged` is an event.

## Every surface

Read a surface's class in `dispatch.generated.d.ts` for its exact methods.

| Surface | What it does | Notable methods |
| --- | --- | --- |
| `player` | now-playing and transport | onSnapshot, stateGet, play, pause, resume, skipNext, skipPrev, seekTo, setShuffle, setRepeat, queueGet |
| `asset` | fetch blobs by opaque id | get, preload, onReady, onCleared |
| `store` | per-webapp key and value | get, put, delete |
| `config` | user settings from the manifest | get, list, onChanged |
| `capabilities` | which companion features are live | get, onSnapshot |
| `library` | Spotify browse, search, favorites | browse, search, recommendations, favoritesList, favoritesContains, favoritesToggle, favoritesSet, onFavoriteChanged |
| `audio` | volume, mute, TTS, earcons | volumeUp, volumeDown, setVolume, muteToggle, tts, earcon |
| `notifications` | phone notification actions | invokePositive, invokeNegative, onPosted, onRemoved, onUpdated |
| `phone` | call control | stateGet, accept, end, initiate, mute, dtmf, onCallStarted, onCallUpdated, onCallEnded |
| `geo` | location watches, sourced from the phone | watch, unwatch, getOnce, onPosition |
| `net` | HTTP, WS, and SOCKS through the phone | fetch, wsOpen, wsSend, wsClose, streamOpen, streamCancel |
| `hardware` | backlight and ambient light sensor | displaySetMode, displaySetLevel, stateGet, onAmbientLightUpdate, onBrightnessChanged |
| `bluetooth` | adapter alias, bonds, discoverable | list, connect, forget, setAlias, enableDiscoverable |
| `system` | version, logs, power, diagnostics | versionRequest, logsTail, logsSubscribe, reboot, powerOff, factoryReset, onVersion, onLogEntry, onOtaProgress |
| `time` | wall clock | get, onSnapshot, onChanged |
| `voice` | mic and push-to-talk, capability-gated | pushToTalk, cancel, muteMic, stateGet |
| `webapp` | list and activate installed webapps | list, current, activate, icon |
| `forward` | passthrough to the desktop-side extension | text, json, binary, onText, onJson, onBinary |
| `doc` | per-webapp documents the companion also writes | get, list, set, delete, onChanged |
| `lyrics` | timed lyrics for the current track | get |
| `peer` | every known peer and its link state | onSnapshot |

## Gotchas

- Check `.ok` on every request result. The daemon answers with a domain or
  protocol error when it cannot serve the call, such as `player.play` with a uri
  no companion claims.
- Write the app so it works with no phone connected. `player`, `library`, and
  `net` all depend on the companion app.
- Treat asset ids as opaque. Pass them to `client.asset.get` as they arrive.
- Read `client.capabilities` before using `voice` and `phone`.

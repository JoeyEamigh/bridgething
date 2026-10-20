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
client.player.play({ uri: 'https://stream.example/live.mp3' }); // any http(s) media url plays on the phone
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

An `http`/`https` uri is a raw media stream (internet radio, a podcast episode,
an HLS playlist) played by the phone's native player. Its snapshot carries the
stream's live metadata: ICY titles land in `track.title`, station artwork in
`track.artworkId`, and `playback.setElapsedTimeAvailable` is true only for
finite media that can seek. Live streams have no duration. A stream that fails
arrives on `player.onErrorEvent` as `playFailed`, and the stream stops when the
last Car Thing disconnects.

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

## The settings page: `@bridgething/client/settings`

The page under `settings/` runs in the companion: the phone app, the desktop
app, or the device page at bridgething.com. It reads and writes the app's
config and docs through `@bridgething/client/settings`, a `postMessage` bridge.

```ts
import { settings } from '@bridgething/client/settings';

const { webappId, name, version, deviceId } = await settings.context();
await settings.config.set('units', 'metric'); // also fields, list, delete
await settings.doc.set('layout', JSON.stringify(layout)); // also get, list, delete
const off = settings.onDocChanged((key, value) => sync(key, value));
settings.done(); // closes the settings screen
```

The page has real internet but renders from a null origin, so the webview's
own `fetch` and XHR are blocked by CORS against any server without permissive
headers. WebSockets work from the page directly. For HTTP,
`settings.fetch(url, init)` has the host make the request natively and hands
back a real `Response`: 1 MiB each way, buffered whole, with an optional
`timeoutMs`. It rejects with a `SettingsFetchError` whose `kind` is `network`,
`timeout`, or `invalid_url`. `settings.installFetch()` swaps the global `fetch`
for third-party SDKs and returns a function that restores it.
`settings.auth.authorize(url)` opens an OAuth provider in the real browser and
resolves with the callback URL; register
`https://bridgething.com/oauth/callback` as the redirect URI, do PKCE in the
page, and exchange the code over `settings.fetch`. It rejects with an
`AuthorizeError` whose `kind` is `cancelled`, `busy` (one authorize at a time),
or `unsupported`. The device page has neither: there `settings.fetch` rejects
with a plain `Error` and `authorize` with `unsupported`.

The companion keeps the page loaded after the user leaves it and shows the
same document the next time they open the app's settings. Three rules follow:

- `settings.onRefresh(context => ...)` fires each time the page comes back on
  screen, and whenever the app's name, version, or config fields change. Reread
  anything the page read from the device, such as a doc it cached.
- `settings.done()` works only while the page is on screen. A `done()` from a
  page the user already left does nothing.
- A page that navigates or reloads itself comes back as a fresh document.
  Calls the old one had in flight are never answered.
- A page that crashes the webview before it finishes loading, or twice in a
  row while on screen, shows a failure with a retry instead of loading again.

When the phone's settings screen hides, the phone app blurs the focused field.

## The config form: `@bridgething/client/form`

`<ConfigForm/>` renders every `config` field from the manifest and writes each
one to the device as the user commits it. An app with `config` and no
`settings` page still gets a settings screen: the companion shows a default
page that is just `<ConfigForm/>` and a Done button. A custom page embeds the
form and adds its own controls around it, so never hand-roll inputs for
declared fields.

```tsx
import {
  ConfigForm,
  ConfigFormError,
  closeSettings,
  type ConfigFormHandle,
} from '@bridgething/client/form';

const form = useRef<ConfigFormHandle>(null);

async function connect() {
  try {
    const values = await form.current!.flush();
    await signIn(values.base_url, values.token);
  } catch (err) {
    if (!(err instanceof ConfigFormError)) throw err;
    setStatus(err.message);
  }
}

async function close() {
  try {
    await closeSettings(form.current);
  } catch (err) {
    setStatus(err instanceof Error ? err.message : String(err));
  }
}

<ConfigForm ref={form} />;
```

A toggle or a choice commits on tap. A text or number field commits when it
loses focus, or on Enter. A touch webview does not always take focus off a
field when a button is tapped, so any button that acts on the values calls
`flush()` first. A Done button calls `closeSettings(form.current)`, which
flushes and then closes the page unless an edit was refused, rejecting with
that error so the page can show it. `flush()` writes every field still being
typed, waits for the device, and resolves with the values the device holds.
It rejects with a `ConfigFormError`:

- `kind: 'refused'`: a typed value broke its field's rules, or the device
  refused the write. `fields` maps each config key to its message and `message`
  is `<label>: <reason>`. The form shows the same message under the field.
- `kind: 'unavailable'`: the form never loaded. `message` is the load failure,
  and the form shows its own retry.
- `kind: 'replaced'`: the host pointed the page at another app while `flush()`
  waited. The edits still land on the app they were made for.

`onChange` reports written values only, never text still being typed. It fires
the moment a field commits, and again with the device's value if the write is
refused. Take values for an action from `flush()`, not from `onChange` state.
The form rereads its fields and values on every refresh.

The form styles off `--bt-*` CSS variables (`--bt-color-bg`, `--bt-color-fg`,
`--bt-color-accent`, `--bt-font-body`, `--bt-text-row`, and the rest of the
names in `@bridgething/client/theme`).

The host app's theme arrives on its own. `settings.context()` carries
`theme: { scheme, colors, text, fonts }`, and importing
`@bridgething/client/settings` paints it as those `--bt-*` variables plus
`--bt-scheme` (`light` or `dark`), repainting when the phone or desktop flips
scheme. Use the variables in your own CSS and the page matches the app for
free; `color-scheme: var(--bt-scheme)` on `html` gets native controls right. The
painted values have zero specificity, so any `:root` rule of yours wins: set
the variables there to opt out. `settings.onThemeChanged(theme => ...)` hears
flips when a canvas or chart needs the raw tokens. An older companion sends no
theme and the variables fall back to the defaults, following
`prefers-color-scheme`.

Write the page in react like the rest of the app. `react` and `react-dom/client`
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

## Files the user picks: `client.blob`

A webapp can show files the user hands it from the companion, like a GIF or
video background. The settings page picks and uploads them; the webapp only
reads them.

```tsx
import type { BlobEntry } from '@bridgething/lib/client';

const [bg, setBg] = useState<BlobEntry | null>(null);

useEffect(() => {
  const load = async () => {
    const r = await client.blob.list();
    setBg(
      r.ok
        ? (r.response.entries.find(e => e.key === 'background') ?? null)
        : null,
    );
  };
  load();
  return client.blob.onChanged(c => c.key === 'background' && load());
}, [client]);

if (!bg) return null;
if (bg.mime.startsWith('video/'))
  return <video src={bg.url} autoPlay loop muted playsInline />;
return <img src={bg.url} />;
```

`url` is `/.bridgething/blob/<key>?v=<sha256>`, served by the daemon from the
page's own origin with the stored mime, an ETag, and Range support. Use it
as-is. The `?v=` changes whenever the content does, so caching never shows a
stale file. Only the active webapp is served, and `onChanged` reaches only the
active webapp, never an overlay.

On the settings page, `pick` opens the phone's photo picker or the desktop's
file dialog and hands back a handle, never the bytes. `blob.put` streams the
file to the device under a key:

```ts
import { BlobError, settings } from '@bridgething/client/settings';

const picked = await settings.pick({ kind: 'video' }); // null when the user backs out
if (picked) {
  thumb.src = picked.preview ?? ''; // small jpeg data url; a video's is its first frame
  try {
    await settings.blob.put('background', picked.handle, {
      onProgress: (sent, total) => (bar.value = sent / total),
    });
  } catch (err) {
    if (err instanceof BlobError && err.kind === 'too_large') showTooBig();
  }
}
```

`kind` is `'image'`, `'video'`, or `'any'`. The companion makes picks fit the
device before upload, and there are no options beyond `cover`:

- Animated GIF, APNG, and WebP pass through byte for byte. Crop and placement
  are yours to do in the webapp.
- Stills larger than the `cover` box scale down until they just cover it,
  keeping their format. HEIC becomes JPEG.
- Video always becomes H.264 mp4, no audio, at most 30 fps, scaled to cover the
  box without upscaling.
- `any` is untouched.
- On the device page at bridgething.com, the browser's own file picker is used
  and nothing is converted or previewed, since a browser has no transcoder.

`cover: [w, h]` is the one override. It defaults to `[800, 800]` for stills,
which covers the screen in either orientation with zoom headroom, and
`[800, 480]` for video. Each edge is a whole number from 1 to 4294967295;
anything else rejects. The result is
`{ handle, mime, size, width?, height?, durationMs?, preview? }`. One picker is
open at a time: a `pick` while another is still open rejects with `busy`.

Bytes the page makes itself, like a canvas export, go through a handle too:

```ts
const png = await new Promise<Blob>(done =>
  canvas.toBlob(b => done(b!), 'image/png'),
);
const handle = await settings.blob.create('image/png');
await settings.blob.append(handle, png); // Blob, ArrayBuffer, or a typed array
await settings.blob.put('cropped', handle);
```

`settings.blob.append` returns the handle's size so far.
`settings.blob.list()` returns `{ key, sha256, mime, size }` for every key, and
`settings.blob.delete(key)` removes one.

`put` resolves once the device has the file, however long the link takes: it
streams on the background lane so now-playing stays smooth, which means a
large video over a car's Bluetooth link takes minutes. `onProgress` reports the
bytes the device has confirmed a few times a second and ends on the total, so
show it. A `put` that picks up an interrupted upload starts at the bytes the
device already has, not at 0. Putting bytes the device already holds finishes
at once, and a second `put` of the same key replaces one still in flight.

A handle lives in the companion, not on the device:

- A `put` the device stores uses the handle up. To store the same bytes under
  a second key, pick or create again.
- A failed `put` keeps the handle, so the upload can be retried with it.
- A `put` keeps its handle alive for as long as it streams.
- A handle nothing is uploading expires after 30 minutes unused.
- The companion holds at most 128 MiB of handles. A pick or append past that
  rejects with `busy` until a `put` lands or a handle expires. Nothing already
  held is dropped.
- A handle that expired or was used up rejects with `unknown_handle`. Pick
  again.

`pick` and every `settings.blob` call reject with a `BlobError`. Its `kind`:

- `unsupported`: the companion or the Car Thing predates these calls, or the
  host cannot do the work.
- `disconnected`: no Car Thing is linked, or the link dropped mid-upload.
- `too_large`: the file is over 32 MiB.
- `quota_exceeded`: the put would take the app past 64 MiB.
- `storage_full`: the Car Thing has no room left.
- `not_media`: the picked file is not the kind asked for.
- `unknown_handle`: the handle is not live. It was never made, it expired, or
  a stored `put` used it up.
- `busy`: a picker is already open, or held handles fill the companion's whole
  handle budget.
- `no_gesture`: the pick did not start from a tap or click, so the browser
  opened no picker. Call `pick` from the tap handler itself.
- `failed`: anything else. Read `message`.

The footguns:

- 32 MiB per file and 64 MiB per webapp of distinct content. The device refuses
  past either and never evicts to make room, so delete what you replace.
- A key is 1 to 256 bytes with no control characters. Do not use `.` or `..`:
  a browser folds them out of the url, so the file never loads.
- A GIF is decoded on the device's CPU every frame, on a Cortex-A53 that also
  runs everything else. A big or long GIF starves the now-playing UI. Prefer
  video for animated backgrounds: it decodes in hardware. Pick with
  `kind: 'video'`, or tell users to keep GIFs small.

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
| `system` | version, logs, power, diagnostics, launcher gesture | versionRequest, logsTail, logsSubscribe, reboot, powerOff, factoryReset, launcherGestureGet, launcherGestureSet, onVersion, onLogEntry, onOtaProgress, onLauncherGestureChanged |
| `time` | wall clock | get, onSnapshot, onChanged |
| `voice` | mic and push-to-talk, capability-gated | pushToTalk, cancel, muteMic, stateGet |
| `webapp` | list and activate installed webapps | list, current, activate, icon |
| `forward` | passthrough to the desktop-side extension | text, json, binary, onText, onJson, onBinary |
| `doc` | per-webapp documents the companion also writes | get, list, set, delete, onChanged |
| `blob` | files the user picked in the companion, served by url | list, onChanged |
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

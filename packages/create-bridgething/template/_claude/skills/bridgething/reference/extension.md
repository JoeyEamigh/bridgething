# Extensions (`@bridgething/extension`)

An extension is the desktop-side half of a webapp: a Deno process the bridgething
desktop app spawns, one per installed and enabled app. It has whatever host
access the manifest asks for, and reaches every connected Car Thing through the
daemon's forward surface.

## Shape of an extension

```ts
import { asJson, defineExtension, json } from '@bridgething/extension';

let timer: ReturnType<typeof setInterval> | undefined;

defineExtension({
  start(ctx) {
    ctx.on('device', event => ctx.log.info(event.type, event.device.name));
    ctx.on('message', (device, message) =>
      device.send(json({ echo: asJson(message) })),
    );
    timer = setInterval(() => ctx.broadcast(json({ tick: Date.now() })), 5_000);
  },
  stop() {
    clearInterval(timer);
  },
});
```

`defineExtension` runs `start` after the host's `hello`, sends `ready` when the
returned promise settles, and on `stop`, or when the host closes stdin, runs
`stop` and exits. The extension runs device or no device.

- Stdout is the protocol. Log with `ctx.log.*` and keep `console.error` on stderr
  for scratch debugging.
- Register listeners synchronously at the top of `start`. The host replays every
  live device immediately after it calls `start`, so a listener added after an
  `await` misses them.

## The `ctx` contract

Read `node_modules/@bridgething/extension/dist/*.d.ts` for the exact signatures.

Identity:

- `ctx.api` is the host protocol revision, `1`.
- `ctx.webapp` is `{ id, name, version }` from `manifest.json`.
- `ctx.dataDir` is a directory this extension owns. The kv store lives there.

Devices:

- `ctx.devices` is the connected devices, recomputed on read.
- `ctx.device(id)` returns a handle for any id, connected or not. It never
  throws.
- `ctx.on('device', event => ...)` streams `event.type` of `connected`,
  `disconnected`, or `active`, with `event.device` as the handle.
- A handle carries `id`, `name`, `active`, `connected`, `config`, and `send`, and
  survives a disconnect with `connected` false.

Messages:

- `ctx.device(id).send(message)` and `ctx.broadcast(message)`. Broadcast reaches
  every connected device where this webapp is active.
- `ctx.on('message', (device, message) => ...)` takes forwards coming back.
- Both directions are fire and forget. Build your own correlation for
  request and reply.
- The daemon delivers a forward to the active webapp, so check `device.active`. A
  send to a backgrounded app is dropped silently.

Config:

- `ctx.config(device)` is a snapshot of that device's settings, keyed by
  `manifest.json` config key. Values are strings.
- `ctx.on('config', (device, key, value) => ...)` fires when a settings page
  writes, from any companion. `value` is `null` when the user cleared the key,
  and the key leaves the snapshot. An empty string is a real empty value.

Storage, auth, logging:

- `ctx.kv.get(key)`, `.set(key, value)`, `.delete(key)`, and `.list()` are a
  persistent JSON store scoped to this extension. `get` resolves `undefined` for
  a missing key.
- `ctx.auth.authorize(url)` opens the system browser and resolves with the full
  callback URL, query string included. It rejects when the user backs out.
- `ctx.log.debug/info/warn/error(...args)` writes into the host's log: the vite
  terminal in development, the desktop app's log once installed.
- `kv` and `auth` failures reject with an `ExtensionError` carrying a `kind` of
  `host-error`, `disconnected`, or `write-failed`.

Persist eagerly as state changes. The host drops your stdin the moment it sends
`stop`, so a `ctx.kv` or `ctx.auth` call made from `stop` rejects with
`kind: 'disconnected'`. Use `stop` to release timers, sockets, and children.

## Forward messages

A forward is `{ encoding, data }`, the same type the webapp SDK uses:

| encoding | reading                             | writing                                |
| -------- | ----------------------------------- | -------------------------------------- |
| `text`   | `asText(message)` -> `string`       | `send('hi')` or `send(text('hi'))`     |
| `json`   | `asJson<T>(message)` -> `T`         | `send(json(value))`                    |
| `binary` | `asBinary(message)` -> `Uint8Array` | `send(bytes)` or `send(binary(bytes))` |

The narrowing helpers return `undefined` when the encoding does not match, so
`asJson(message) ?? fallback` is the normal shape. Wrap a plain object in
`json(...)` before sending, because `send` cannot tell one apart from a
`ForwardMessage`. Binary always arrives as a `Uint8Array`.

## Manifest block and permissions

```json
"extension": {
  "entry": "extension/desktop.mjs",
  "permissions": ["all"],
  "api": 1
}
```

- `entry` is the path inside the built bundle. It must exist, or the daemon
  refuses the install.
- `api` is the host protocol revision. `1` is frozen.
- `permissions` are Deno permission descriptors, handed to the runtime as flags.

Descriptor grammar, one string per entry: `all`, `net`, `net:<host[:port]>`,
`read`, `read:<path>`, `write`, `write:<path>`, `run`, `run:<binary>`, `env`,
`env:<VAR>`, `sys`, `sys:<kind>`, `ffi`, `ffi:<path>`. A path may start with `~`.

Narrow `["all"]` before you publish. The dev loop runs the extension with exactly
the declared flags, so it fails on your machine the moment you use something you
did not declare. The store card lists the permissions and the desktop app makes
the user confirm them at install. A version declaring `extension` must publish
from a public GitHub repo.

## The dev loop

```bash
bun run dev            # page in your browser + extension under deno, both on the connected Car Thing
bun run dev:device     # the same, with the page on the Car Thing's own screen
bun run build          # webapp + settings page + dist/extension/desktop.mjs
bun run share          # zips dist/, extension included
```

Both dev commands:

1. Make this app the active webapp on the Car Thing, installing it once if the
   device has never seen it, so forwards route.
2. Bundle `extension/main.ts` with esbuild and run it under Deno with the
   manifest's permissions, from the gitignored `.dev-extension/`, which also
   holds the kv store. Every save rebuilds and restarts it. A build error prints
   and leaves the previous build running.
3. Link to the daemon over USB as an extension host, so the extension gets
   `device.connected`, `device.active`, `config.changed`, and every forward the
   page sends. With the Car Thing unplugged it links up when one appears.

`ctx.log.*` and stderr both print in the vite terminal. A crash restarts with
backoff, or immediately on the next save; a tight loop shows as `crashed` in the
desktop app's row. Ctrl-C stops the extension and, under `dev:device`, hands the
screen back. Disable this app's extension in the desktop app while you develop,
or the page hears both copies.

Under `bun run dev`, `ctx.auth.authorize(url)` ends on bridgething.com's callback
page. Copy that page's address and paste it at
`http://localhost:5173/__extension/authorize` to deliver it to the extension. The
desktop app captures the callback itself once the app is installed there.

The runtime is the `deno` npm package, pinned to the version the desktop app
downloads and fetched into `node_modules` on first run. `BRIDGETHING_DENO=/path`
overrides it.

## Shipping

`bun run push` installs the webapp on a Car Thing, and the extension stays on the
desktop. Once someone installs the app through the desktop app, that app runs the
extension from the zip. Phones have no extension host, and the desktop app can be
closed while the device is on a phone, so make the webapp useful with nothing
attached.

`npm:`, `jsr:`, and `node:` specifiers work in the bundle, as do `Deno.Command`
and unix sockets, subject to the permissions you declared. Deno resolves them at
runtime only when it is not standing in a project: a `package.json` or
`deno.json` anywhere above the bundle switches it to node_modules resolution and
it refuses `npm:` with _"Could not find a matching package"_. The dev loop and
the desktop app run the bundle from a data directory with
`DENO_NO_PACKAGE_JSON=1`. Set that variable when you run the bundle by hand.

# **PROJECT_NAME**

A bridgething webapp scaffolded with `create-bridgething`. It runs full-screen in
the chromium kiosk on a Spotify Car Thing and talks to the on-device daemon
through [`@bridgething/client`](https://github.com/JoeyEamigh/bridgething).

Stack: React 19, Vite, Tailwind v4, TypeScript strict.

Open this folder with your coding agent. It reads `CLAUDE.md` or `AGENTS.md` and
the `/bridgething` skill.

## Develop

```bash
bun install
bun run dev
```

Vite serves at http://localhost:5173/ with hot reload, against the daemon on the
Car Thing plugged in over USB. Set `SUPERBIRD_HOST` to target another device, or
`BRIDGETHING_DAEMON_URL` to reach a daemon elsewhere.

```bash
bun run dev:device
```

The same server, shown on the Car Thing's own screen, hot reload included.
Ctrl-C hands the screen back to the installed build.

When the project ships an extension (`extension/main.ts`), both commands bundle
it on every save and run it under Deno with the permissions the manifest
declares.

## Push, share, update

```bash
bun run push     # build, copy to the Car Thing, switch the kiosk to it
bun run build
bun run share    # write <name>-<version>.zip from dist/
bun run update   # bring the device to the latest bridgething release
```

`push` targets `bridgething.local` over USB; pass a host or IP address for
another device. Anyone with a bridgething Car Thing installs the zip from the
companion app. For one device out of several,
`bun run update -- --host ws://bridgething-<serial>.local:8892/`.

## Layout

- `src/App.tsx` subscribes to `client.player.onSnapshot`, fetches artwork through
  `client.asset.get`, and draws transport controls.
- `src/daemon.ts` returns the daemon URL.
- `public/manifest.json` carries the webapp id, version, config schema, and
  permissions.
- `settings/` is the settings page the companion app renders, built to
  `dist/settings.html`.
- `index.html` and `vite.config.ts` fix the 800x480 viewport and the build target.

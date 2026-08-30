# Building this bridgething webapp

A bridgething webapp is one page running full-screen in the chromium kiosk on a
Spotify Car Thing. It reaches the on-device daemon through `@bridgething/client`.

## The device

The screen is 800x480, landscape, and never resizes. The kiosk shows one webapp,
so build in-app views rather than tabs or windows. Add an on-screen keyboard if
the app needs text entry.

Listen with a `keydown` handler and a `wheel` handler on `window`.

| Control      | Event                                 |
| ------------ | ------------------------------------- |
| Preset 1-4   | `keydown` key `"1"` `"2"` `"3"` `"4"` |
| Mode         | `keydown` key `"m"`                   |
| Back         | `keydown` key `"Escape"`              |
| Rotary wheel | `wheel` with horizontal `deltaX`      |
| Touch        | pointer and touch events              |

Make horizontal wheel scroll move through the main list. Five fast presses of
Mode returns to the launcher.

## The client

```ts
import { BridgethingClient } from '@bridgething/client';
import { useMemo } from 'react';

import { daemonUrl } from './daemon';

const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
```

Construct it once and reuse it. It connects and reconnects on its own. Call
`daemonUrl()` instead of a literal `ws://` address.

Now-playing and library data come from the phone's Spotify, so render a
placeholder when no phone is connected. Fetch artwork with `client.asset.get`
using the opaque id on the track.

Every surface: `player asset config store doc capabilities library audio
notifications phone peer geo net hardware bluetooth system time voice lyrics
webapp forward`. Each method is an event, a request, or a command.

Methods, types, and examples: `.claude/skills/bridgething/reference/sdk.md`.

## `public/manifest.json`

- `id` identifies this webapp on the device. Keep it.
- `version` tells builds apart. Raise it before sharing an update.
- `config` declares the settings the companion app edits and `client.config`
  reads.
- `permissions` grants `geo` and `net.proxy`.
- `art.heroPx` and `art.thumbPx` are the sizes artwork arrives at.
- `settings` names the page built from `settings/`, capped at 1 MiB. It talks to
  the companion app through `@bridgething/client/settings`.

## Workflow

- `bun run dev` serves at `http://localhost:5173/` against the connected Car
  Thing.
- `bun run dev:device` shows that server on the device's own screen.
- `bun run build` writes `dist/`.
- `bun run push` builds and installs onto the connected device.
- `bun run share` zips `dist/`.
- `bun run update` brings the device to the latest bridgething release.

Running and driving the app: `.claude/skills/bridgething/reference/develop.md`.
Push, zip, update: `.claude/skills/bridgething/reference/ship.md`.

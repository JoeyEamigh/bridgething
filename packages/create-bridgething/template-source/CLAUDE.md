# Building bridgething webapps in this repo

This repo is a bun workspace of webapps for the Spotify Car Thing, plus the pipeline that publishes them as a `catalog.v1` source on GitHub Pages. Each `apps/<slug>` is one webapp: a single page running full-screen in the chromium kiosk on the device, reaching the on-device daemon through `@bridgething/client`.

## The device

The screen is 800x480, landscape, and never resizes. The kiosk shows one webapp, so build in-app views rather than tabs or windows. Add an on-screen keyboard if the app needs text entry.

Listen with a `keydown` handler and a `wheel` handler on `window`.

| Control      | Event                                 |
| ------------ | ------------------------------------- |
| Preset 1-4   | `keydown` key `"1"` `"2"` `"3"` `"4"` |
| Mode         | `keydown` key `"m"`                   |
| Back         | `keydown` key `"Escape"`              |
| Rotary wheel | `wheel` with horizontal `deltaX`      |
| Touch        | pointer and touch events              |

Make horizontal wheel scroll move through the main list. Five fast presses of Mode returns to the launcher.

## The client

```ts
import { BridgethingClient } from '@bridgething/client';
import { useMemo } from 'react';

import { daemonUrl } from './daemon';

const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
```

Construct it once and reuse it. It connects and reconnects on its own. Call `daemonUrl()` instead of a literal `ws://` address.

Now-playing and library data come from the phone's Spotify, so render a placeholder when no phone is connected. Fetch artwork with `client.asset.get` using the opaque id on the track.

Every surface: `player asset config store doc capabilities library audio notifications phone peer geo net hardware bluetooth system time voice lyrics webapp forward`. Each method is an event, a request, or a command.

Methods, types, and examples: `.claude/skills/bridgething/reference/sdk.md`. That skill lives once at the repo root and every app symlinks it, so it is one copy for the whole workspace. `bun run skills` refreshes it from the published `create-bridgething`; `bun run check` says when it is behind.

## Working on one app

```sh
bun run dev                            # the only app, against a connected Car Thing
bun run dev <slug>                     # name it when there is more than one
bun run --cwd apps/<slug> dev          # always works
bun run --cwd apps/<slug> dev:device   # show that server on the device's own screen
bun run --cwd apps/<slug> push         # build and install onto the connected device
bun run --cwd apps/<slug> build        # writes dist/
bun run --cwd apps/<slug> typecheck    # checks src, settings and extension together
bun run --cwd apps/<slug> share        # zips dist/ to hand to someone directly
bun run --cwd apps/<slug> update       # brings the device to the latest bridgething release
```

An app with a variant or an extension carries its own `apps/<slug>/CLAUDE.md` with the parts specific to it.

Running and driving the app: `.claude/skills/bridgething/reference/develop.md`. Push, zip, update: `.claude/skills/bridgething/reference/ship.md`. The desktop-side Deno extension and its permission model, for an app with an `extension/` directory: `.claude/skills/bridgething/reference/extension.md`.

## The three files that describe an app

**`apps/<slug>/public/manifest.json`** is the app's identity as far as the device and the catalog are concerned.

- `id` identifies this webapp on the device. It is a uuidv7 generated once at scaffold time. **Never change it**: the device keys upgrade-in-place and the app's key-value namespace on it, so a new uuid orphans everyone's installed copy and its data.
- `version` is what the store publishes against. Move it with `bun run bump`, never by hand.
- `description` is the store tagline and must not be empty.
- `config` declares the settings the companion app edits and `client.config` reads.
- `permissions` grants `geo` and `net.proxy`.
- `art.heroPx` and `art.thumbPx` are the sizes artwork arrives at.
- `settings` names the page built from `settings/`, capped at 1 MiB. It talks to the companion app through `@bridgething/client/settings`.
- `extension` declares the desktop-side Deno process and the host permissions it gets.

**`apps/<slug>/catalog.json`** is the store listing and nothing else: `author`, `homepage`, `source`, an `icon` override, `screenshots`, and `min_libbridgething_version` (the oldest daemon the app works against). Never restate anything the manifest already says. An app that ships an extension must carry its `github.com` repo url in `source`, because the store shows that link next to the host permissions it is asking for.

**`apps/<slug>/CHANGELOG.md`** carries a `## <version>` section per release, which becomes that version's changelog in the catalog.

## Adding and shipping

```sh
bun run new <slug> [--extension | --launcher | --overlay]
bun run shot <slug> [--replace | --name <label>]
bun run bump <slug> <major | minor | patch | x.y.z> [-m "note"]
bun run check
```

`new` scaffolds into `apps/<slug>`. Never add an app by hand: the scaffold generates the uuid, writes the store listing, wires the dev server to a connected device, and lands the tsconfig and scripts CI expects.

`shot` captures the kiosk over CDP into `apps/<slug>/screenshots/`. The image fronts chromium's debugging port on `bridgething.local:9222`, so it needs nothing but the device plugged in. Up to six, filename order, first is the store card.

`bump` writes `public/manifest.json` and `package.json` together and opens the changelog section. Editing one without the other fails the build.

`check` is the whole gate and is exactly what CI runs: typecheck, build, bundle, generate the catalog, validate it against `catalog.v1` and the cross-reference invariants. Run it before claiming anything works.

Pushing to main publishes. `bun run publish --dry-run` assembles the exact bytes that would be pushed, into `site/`, without pushing.

## Publishing rules that are not negotiable

- **A published version is immutable.** `published.json` on the `gh-pages` branch records the sha256 clients verify against. Changing an app means a new version, always. `check` fails a pull request that changed an app without bumping it.
- **Bundles are reproducible.** Timestamps are flattened before zipping so the same source gives the same digest. Do not add anything nondeterministic to a build.
- **`site/` is generated and gitignored.** Nothing about a release belongs on main.
- **CORS is a requirement.** A source must serve its catalog and its downloads with `Access-Control-Allow-Origin: *` or browser clients cannot read it. GitHub Pages does; most self-hosting does not.
- **`base_url` in `source.json` is where this catalog is served.** If you forked this repo, point it at your own pages site and give every app a fresh uuid, or you publish app ids that belong to someone else. `check` refuses when it disagrees with the origin remote.

## Comments

Terse, lowercase, self-contained, non-obvious WHY only, 120 columns. No historical cross-references, no pointers at other files, no emdashes. Most code here needs none.

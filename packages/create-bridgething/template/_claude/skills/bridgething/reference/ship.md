# Installing and sharing the app

## Install onto a connected Car Thing

```bash
bun run push            # targets bridgething.local (the device over USB)
bun run push <address>  # or an explicit host or IP
```

`push` builds `dist/` itself, copies it to the device, then switches the kiosk to
your app. With several devices on the network, set
`SUPERBIRD_HOST=bridgething-<serial>.local`.

Flags: `--skip-build` copies whatever is already in `dist/`, and `--no-switch`
copies without activating. To iterate on hardware, use `bun run dev:device` from
the develop reference instead.

When `manifest.json` declares `role: launcher` or an `overlay`, `push` also
claims the matching slot. A launcher is switched to; an overlay is not, so pass
`--switch` to open its own page. `--no-slot` pushes without claiming a slot.
`--release` gives the slots back to the built-in hub and overlay.

## Make a shareable zip

```bash
bun run build   # share reads dist/, so produce it first
bun run share   # writes <name>-<version>.zip from dist/
```

Anyone with a bridgething Car Thing installs that zip from the companion app.

Raise `version` in `public/manifest.json` before you share an update. Keep `id`.

## Keep the device on the latest bridgething

```bash
bun run update   # updates the connected Car Thing's daemon and image
```

Flags go after `--`:

```bash
bun run update -- --host ws://bridgething-<serial>.local:8892/  # pick one of several devices
bun run update -- --channel <name>                              # track another channel
bun run update -- --version <composite-version>                 # install a specific release
```

---
name: bridgething
description: Build, run, and ship a bridgething webapp for the Spotify Car Thing. Use for @bridgething/client calls (now-playing, transport, artwork, storage, user settings, networking, every surface), running the dev server and driving the 800x480 screen and its physical controls, pushing the built app to a device or packaging a shareable zip, and writing the desktop-side Deno extension a webapp can ship.
---

# Building a bridgething webapp

- Calling the daemon, from now-playing to networking, plus every surface the SDK
  ships: [reference/sdk.md](reference/sdk.md)
- Running the app, screenshotting the 800x480 screen, and pressing the physical
  controls locally and over CDP on the device:
  [reference/develop.md](reference/develop.md)
- Installing onto a connected Car Thing, packaging a zip, and updating the
  device: [reference/ship.md](reference/ship.md)
- The desktop-side Deno extension, its `ctx` contract, and its permission model,
  when the project has an `extension/` directory:
  [reference/extension.md](reference/extension.md)

CLAUDE.md holds the device shape, the control-to-event mapping, and the manifest
fields.

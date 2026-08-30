## This project is a launcher

`manifest.json` declares `"role": "launcher"`. The daemon keeps this bundle out
of `client.webapp.list`, so the grid you draw excludes itself. Holding the
device's launcher slot makes it the home screen: the boot default, the target of
the Mode gesture, and where the Back button returns to.

`bun run push` claims the slot and switches to it. `bun run push --release` hands
it back to the built-in hub, the recovery path when a build wedges the device.

### What a home screen covers

A launcher implements as much or as little as you want. The built-in hub covers:

- the app grid (`client.webapp.list`, `.icon`, `.activate`, `.current`, plus
  `onWebappInstalled` and `onWebappUninstalled`)
- bluetooth bonds, adapter alias, discoverable
- display brightness
- system info and health
- power: restart, shut down, factory reset
- OTA progress (`client.system.onOtaProgress` and `.onOtaError`)

The starter here draws the grid. Everything the hub uses is on the public client
SDK.

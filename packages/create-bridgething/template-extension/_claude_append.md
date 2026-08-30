## This project ships a native extension

`manifest.json` declares an `extension` block, so this bundle has two halves.

- `src/` is the webapp, running in the device's chromium kiosk.
- `extension/` is a Deno process on the desktop. The bridgething desktop app
  spawns it while the app is installed and enabled, with or without a Car Thing
  connected.

The two halves talk over the daemon's forward surface: `client.forward` in the
webapp, `device.send` and `ctx.on('message', ...)` in the extension. The daemon
delivers a forward while this webapp is the active one on that device.

Host access belongs in the extension, an ordinary Deno program with `npm:`,
`jsr:`, and `node:` available. Keep `src/` a view over what it sends. The webapp
also runs with nothing attached, so check `capabilities.available.forward` and
give the user something useful when it is false.

`manifest.json` starts with `"permissions": ["all"]`. Narrow it to the Deno
permissions you use before you publish.

The `ctx` contract, the permission grammar, the dev loop, and the gotchas:
`.claude/skills/bridgething/reference/extension.md`.

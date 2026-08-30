# @bridgething/updater

Bring a [bridgething](https://github.com/JoeyEamigh/bridgething) Spotify Car
Thing to the latest release on its channel.

```sh
bunx @bridgething/updater
```

The updater connects over the daemon's network gateway, reads the release
manifest, and installs the daemon binary, plus the system image when the image
half of the version changed. It refuses a yanked or deprecated release.

| flag                 | default                         | meaning                      |
| -------------------- | ------------------------------- | ---------------------------- |
| `--root <url>`       | `https://ota.bridgething.com`   | Manifest and artifact root   |
| `--channel <name>`   | the channel the device reports  | Channel to track             |
| `--host <ws-url>`    | `ws://bridgething.local:8892/`  | Daemon network gateway       |
| `--cache-dir <path>` | a directory under the OS tmpdir | Artifact download cache      |
| `--version <ver>`    | the channel's `latest`          | Composite version to install |

The network gateway has no authentication. Run the updater over the USB link or
a trusted LAN. With several devices, point `--host` at one:

```sh
bunx @bridgething/updater --host ws://bridgething-<serial>.local:8892/
```

The same update logic is available from Node through
[`@bridgething/core-node`](https://www.npmjs.com/package/@bridgething/core-node)
and from a web page through
[`@bridgething/browser`](https://www.npmjs.com/package/@bridgething/browser).

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

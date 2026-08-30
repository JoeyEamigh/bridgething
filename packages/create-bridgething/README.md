# create-bridgething

Scaffold a [bridgething](https://github.com/JoeyEamigh/bridgething) webapp for
the Spotify Car Thing: React, Vite, Tailwind v4, and
[`@bridgething/client`](https://www.npmjs.com/package/@bridgething/client).

```sh
bun create bridgething my-app
```

| flag           | what you get                                  |
| -------------- | --------------------------------------------- |
| `--launcher`   | A replacement home screen                     |
| `--overlay`    | System UI drawn over every webapp             |
| `--extension`  | A desktop-side Deno process beside the webapp |
| `--no-install` | Skip `bun install`                            |
| `--no-git`     | Skip `git init`                               |

`--extension` combines with `--launcher`, `--overlay`, or a plain webapp.

The project ships `bun run dev`, `build`, `push` (install on a connected Car
Thing), `share` (zip for the store), and `update` (bring the device to the
latest release).

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

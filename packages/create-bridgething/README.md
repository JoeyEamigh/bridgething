# create-bridgething

Scaffold a [bridgething](https://github.com/JoeyEamigh/bridgething) app source
for the Spotify Car Thing: a GitHub repo of webapps (React, Vite, Tailwind v4,
[`@bridgething/client`](https://www.npmjs.com/package/@bridgething/client)) that
publishes a store catalog to GitHub Pages.

```sh
bun create bridgething my-apps
```

Run it again inside the source to add another app.

| flag           | what you get                                  |
| -------------- | --------------------------------------------- |
| `--launcher`   | A replacement home screen                     |
| `--overlay`    | System UI drawn over every webapp             |
| `--extension`  | A desktop-side Deno process beside the webapp |
| `--no-install` | Skip `bun install`                            |
| `--no-git`     | Skip `git init`                               |

`--extension` combines with `--launcher`, `--overlay`, or a plain webapp.

The source ships `bun run dev`, `push` (install on a connected Car Thing),
`share` (zip to hand to someone directly), `bump`, `check`, and `submit` (list
the source in the bridgething store). Pushing to main publishes the catalog.

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

# @bridgething/client

The SDK a [bridgething](https://github.com/JoeyEamigh/bridgething) webapp uses
to talk to the daemon on a Spotify Car Thing.

```sh
bun add @bridgething/client
```

```ts
import { BridgethingClient } from '@bridgething/client';

const client = new BridgethingClient();
client.player.onSnapshot(msg => render(msg.state));
client.player.skipNext();
```

The client connects on construction and reconnects on its own. Every method
has hover docs.

`@bridgething/client/settings` is the API for a webapp's settings page, which
runs in the companion app or the desktop app:

```ts
import { settings } from '@bridgething/client/settings';

await settings.config.set('city', 'Detroit');
```

Scaffold a webapp with the client preinstalled:

```sh
bun create bridgething my-app
```

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

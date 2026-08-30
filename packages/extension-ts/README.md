# @bridgething/extension

The client a [bridgething](https://github.com/JoeyEamigh/bridgething) extension
uses to talk to its host. An extension is a Deno process the bridgething
desktop app runs beside a webapp.

```sh
bun add @bridgething/extension
```

```ts
import { asJson, defineExtension, json } from '@bridgething/extension';

defineExtension({
  start(ctx) {
    ctx.on('device', e => ctx.log.info(e.type, e.device.name));
    ctx.on('message', (device, message) =>
      device.send(json({ echo: asJson(message) })),
    );
  },
});
```

Register listeners synchronously in `start`; the host replays connected devices
as soon as `start` runs. The host protocol owns stdin and stdout, so
`console.log` corrupts it. Log with `ctx.log`.

Scaffold a webapp with an extension:

```sh
bun create bridgething my-app --extension
```

- Docs: <https://bridgething.com/docs/extensions>
- Source: <https://github.com/JoeyEamigh/bridgething>

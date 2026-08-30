# @bridgething/browser

Drive a [bridgething](https://github.com/JoeyEamigh/bridgething) Spotify Car
Thing from a web page: install and switch webapps, push updates, rename the
device.

```sh
bun add @bridgething/browser
```

```ts
import { Device } from '@bridgething/browser';

// the daemon's network gateway, over usb or the local network
const device = await Device.overNetwork('bridgething.local');

// a paired bluetooth peer, over Web Serial
const device = await Device.overSerial();
```

`overSerial` resolves null when the user dismisses the port chooser. Web Serial
is Chromium-only; check `serialAvailable()` before offering it.

```ts
const webapps = await device.webapps();
await device.switchWebapp(webapps[0].id);
await device.installWebapp(bytes, 'https://apps.example.com/weather.zip');
await device.setNickname('the dashboard');

const phase = await device.push('daemon', binary);
if (phase.kind === 'failed') console.error(phase.reason);
```

`installWebapp` records the second argument as the install's source.
`nextEvent` reads the progress feed during a push. `fetchManifest`,
`compositeVersion`, and `otaArtifactUrls` read the release manifest the device
reads.

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

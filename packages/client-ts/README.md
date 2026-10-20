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

`settings.pick` and the `settings.blob` verbs reject with a `BlobError`. Its `kind` is one of:

| kind             | meaning                                                                     |
| ---------------- | --------------------------------------------------------------------------- |
| `unsupported`    | the host or the Car Thing has no such verb, or cannot do the work           |
| `disconnected`   | no Car Thing is linked, or the link dropped mid-upload                      |
| `too_large`      | the bytes are over the per-blob cap                                         |
| `quota_exceeded` | the put would take the app over its storage quota                           |
| `storage_full`   | the Car Thing has no room left                                              |
| `not_media`      | the picked file is not the kind the page asked for                          |
| `unknown_handle` | the handle is not live: never made, released, or left idle too long         |
| `busy`           | a picker is already open, or held handles fill the whole handle budget      |
| `no_gesture`     | the pick did not start from a tap or click, so the browser opened no picker |
| `failed`         | anything else; read `message`                                               |

A `pick` cover edge is a whole number from 1 to 4294967295.

`settings.onRefresh(context => ...)` fires when the host shows the page again or the app behind it was updated. Reload anything the page read from the device.

`@bridgething/client/form` is `<ConfigForm/>`, a react component (preact/compat works) that renders the manifest's `config` fields over that API. It styles off `--bt-*` CSS variables from `@bridgething/client/theme`. The settings SDK paints the companion's theme and repaints on a light/dark flip; you can override with your own `:root` rules.

```tsx
import {
  ConfigForm,
  ConfigFormError,
  type ConfigFormHandle,
} from '@bridgething/client/form';

const form = useRef<ConfigFormHandle>(null);

async function connect() {
  try {
    const values = await form.current!.flush();
    await signIn(values.base_url, values.token);
  } catch (err) {
    if (err instanceof ConfigFormError) setStatus(err.message);
  }
}

<ConfigForm ref={form} onChange={values => setValues(values)} />;
```

A text field writes to the device when it loses focus or on Enter. A touch webview does not always take focus off a field when a button is tapped, so a button that acts on the values calls `flush()` first. `flush()` writes every field still being typed, waits for the device to answer, and resolves with the values the device holds. It rejects with a `ConfigFormError` when a typed value breaks its field's rules or the device refuses a write (`kind: 'refused'`, with a message per field key in `fields`), when the form never loaded (`'unavailable'`), or when the host moved the page to another app while it waited (`'replaced'`).

A Done button calls `closeSettings(form.current)`. It flushes, then closes the page unless an edit was refused, and rejects with that `ConfigFormError` so the page can show it.

When the host moves the page to another app, every edit already made, and every field still being typed, is written to the app it was made for, and `onChange` stays quiet about the old app. The form for the next app says so if the device refuses one. It loads at once, unless it is the same app the old form is still writing to; then it waits for those writes so it shows what landed. `settings.config.pin()` returns config verbs that keep talking to the app the page holds now in the same way. Call `release()` on them once their last call settles, so the host can drop that app. `settings.onContextChanged((context, previous) => ...)` hands over the app the page showed before the move, or `null` when the page never learned it.

`onChange` reports written values only, never text still being typed. It fires as soon as a field commits, and again with the device's value if the write is refused. The form reloads its fields and values on `onRefresh` and offers a retry when a load fails.

`@bridgething/client/settings-protocol` is the bridge's wire contract for hosts that render custom settings pages. A host calls `openSettingsSession` once per document it loads, hands that document's messages to `receive`, and calls `close` when it replaces the document. Each retarget opens a new epoch. The session keeps serving an earlier epoch for writes the page is still finishing until the page sends `release` for it, and refuses anything on a released epoch with `UNKNOWN_SETTINGS_EPOCH`.

`@bridgething/client/testing` drives a built settings page in a [happy-dom](https://github.com/capricorn86/happy-dom) `Window` against a fake host, and does not itself depend on happy-dom: `settingsPage(window, { target, bind })` opens the session, `load(html)` runs the page, and `enter`, `press`, `text` and `until` drive it.

Scaffold a webapp with the client preinstalled:

```sh
bun create bridgething my-app
```

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

# Running and driving the app

## Start the dev server

```bash
bun run dev
```

Vite serves at `http://localhost:5173/` with hot reload. The page reaches the
daemon on the Car Thing plugged in over USB through the dev server's
`/__bridgething` proxy, so now-playing, config, and geo are the real thing rather
than fixtures. `SUPERBIRD_HOST=bridgething-<serial>.local` targets another
device, and `BRIDGETHING_DAEMON_URL=ws://host:8891` targets a daemon anywhere
else.

## Show it on the Car Thing with hot reload

```bash
bun run dev:device
```

On start it installs the app once if the daemon has never seen it, makes it the
active webapp so config, permissions, and forwards are yours, then points the
kiosk at `http://<your usb address>:5173/`. Every save from then on hot-reloads
on the real display. Ctrl-C hands the screen back to the installed build.

## The extension half

When `public/manifest.json` declares `extension`, both dev commands also bundle
`extension/main.ts`, run it under Deno with the manifest's permissions, and link
it to the same Car Thing. Every save rebuilds and restarts it, and `ctx.log.*`
prints in the vite terminal. The contract is in
[reference/extension.md](extension.md).

## Drive it with Playwright

To screenshot and drive the app from a script, add Playwright:

```bash
bun add -d playwright
bunx playwright install chromium
```

```ts
import { chromium } from 'playwright';

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 800, height: 480 } });
await page.goto('http://localhost:5173/');

await page.screenshot({ path: 'screen.png' }); // look at the result

// physical controls -> the exact events the device sends:
await page.keyboard.press('1'); // preset 1  (also '2' '3' '4')
await page.keyboard.press('m'); // Mode button
await page.keyboard.press('Escape'); // Back button
await page.mouse.wheel(120, 0); // rotary wheel: HORIZONTAL scroll (deltaX)
// touch: page.tap('selector')  or  page.mouse.click(x, y)
```

Loop: edit code, let vite hot-reload, screenshot, adjust. Keep the viewport at
`{ width: 800, height: 480 }` so what you see matches the device.

## Your own browser and DevTools

Open `http://localhost:5173/`, then set a custom device of 800 x 480 in DevTools'
device toolbar. Type `1` through `4`, `m`, and `Escape`, and hold shift while
scrolling for the rotary's horizontal wheel.

## Drive the real device over CDP

CDP input goes into the page, so the launcher gesture and browser-nav side
effects stay out of the way.

Get the app on the kiosk first with `bun run dev:device` or `bun run push`.
Chromium listens on loopback, but the image proxies it, so CDP answers on
`bridgething.local:9222` over USB:

```bash
curl -s http://bridgething.local:9222/json/version >/dev/null && echo cdp-up
```

Use the `webSocketDebuggerUrl` exactly as returned. It comes back as an IP
rather than the hostname you asked for, and that address is the correct one.

Playwright's `connectOverCDP` never finishes the websocket upgrade against the
device's embedded chromium, so drive it with a raw CDP websocket. Connect to the
`page` target from `/json`, not the browser-level endpoint.

```ts
// drive.ts  ->  run with: bun drive.ts
const targets = (await fetch('http://bridgething.local:9222/json').then(r =>
  r.json(),
)) as any[];
const page = targets.find(t => t.type === 'page');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise<void>((res, rej) => {
  ws.onopen = () => res();
  setTimeout(() => rej(new Error('ws open timeout')), 8000);
});

let id = 0;
const pending = new Map<number, (v: any) => void>();
ws.onmessage = ev => {
  const m = JSON.parse(ev.data as string);
  if (m.id && pending.has(m.id))
    (pending.get(m.id)!(m.result), pending.delete(m.id));
};
const send = (method: string, params: any = {}) =>
  new Promise<any>(
    res => (
      pending.set(++id, res),
      ws.send(JSON.stringify({ id, method, params }))
    ),
  );

// screenshot
const { data } = await send('Page.captureScreenshot', { format: 'png' });
await Bun.write('device.png', Buffer.from(data, 'base64'));

// read what is on screen, to assert a change happened
const text = await send('Runtime.evaluate', {
  expression: 'document.body.innerText',
  returnByValue: true,
});
console.log(text.result.value);

// press a button: keyDown + keyUp. key/code/text must match a real key.
const key = (type: string, k: string, code: string, vk: number) =>
  send('Input.dispatchKeyEvent', {
    type,
    key: k,
    code,
    text: k.length === 1 ? k : '',
    windowsVirtualKeyCode: vk,
  });
await key('keyDown', '3', 'Digit3', 51); // preset 3
await key('keyUp', '3', 'Digit3', 51);

// rotary wheel = horizontal scroll:
await send('Input.dispatchMouseEvent', {
  type: 'mouseWheel',
  x: 400,
  y: 240,
  deltaX: 120,
  deltaY: 0,
});

process.exit(0);
```

Iterate in the local loop, then confirm real now-playing, real artwork, and the
actual screen on the device.

## Input reference

| Control      | Browser event                         | CDP `key`/`code`/vk        |
| ------------ | ------------------------------------- | -------------------------- |
| Preset 1-4   | `keydown` key `"1"` `"2"` `"3"` `"4"` | `Digit1`-`Digit4` / 49-52  |
| Mode (M)     | `keydown` key `"m"`                   | `KeyM` / 77                |
| Back         | `keydown` key `"Escape"`              | `Escape` / 27              |
| Rotary wheel | `wheel` with `deltaX` (horizontal)    | mouseWheel `deltaX`        |
| Touch        | pointer / touch events                | `Input.dispatchTouchEvent` |

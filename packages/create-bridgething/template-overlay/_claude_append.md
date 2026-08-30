## This project is a system overlay

`manifest.json` declares `"overlay": "overlay.js"`. While this bundle holds the
device's overlay slot, the daemon injects that file into every webapp's document
as it loads. The page in `src/` renders when someone launches this bundle from
the hub.

`bun run push` claims the overlay slot and reloads the kiosk, so the overlay
appears over the webapp that is showing. `bun run push --release` hands the slot
back to the built-in overlay, the recovery path when a build misbehaves.

### The contract

The daemon prepends one global before your bundle:

```js
window.__bridgethingOverlay = { origin: 'http://127.0.0.1:8891', surfaces: {...} };
```

`surfaces` is the active webapp's declared profile, a boolean each for
`notifications`, `call`, `pairing`, `connection`, `volume`, and `voice`. Render
only the surfaces set to true; an app that draws its own volume indicator
declares `volume: false`. When every surface is false the daemon injects nothing.

`origin` is the kiosk origin. Compare `location.origin` against it before
mounting. Everything else comes from `@bridgething/client` over the local
websocket.

### Output constraints

`overlay.js` must be one self-contained file under 512 KiB. The daemon injects it
as a script string into another app's document, so it carries its own code and
styles and reaches nothing at runtime. `vite.overlay.config.ts` builds a single
inlined iife; keep that shape.

Style `overlay/main.tsx` with tailwind classes like the rest of the project.
`overlay/style.css` is imported with vite's `?inline`, so the compiled css mounts
into the shadow root as a string. Tailwind scans only `overlay/main.tsx`
(`source(none)`), so add an `@source` line for every file you split across.

### Keep these four from the starter

1. The origin guard before mounting.
2. The `__bridgethingOverlayMounted` guard, so a second injection is a no-op.
3. The closed shadow root, which keeps your styles and the host app's apart.
4. Escape-only key handling on the capture phase, active only while something is
   showing.

You run inside every webapp's page. A crash, a fullscreen paint, or a broad key
handler takes down all of them.

# bridgething agent guide

Rules for editing this repo. The lib/core split is the one that breaks silently.

## Layout

- `crates/` is the cargo workspace. Daemon side: `lib` (wire types), `core` (the daemon), `iap2`, `mfi`, `mfi-proxy`, `dsp`, `wakeword`. Companion side: `sdk-runtime`, `io`, `gateway-rs`, `delivery/{core,napi,wasm}`, `companion`, with `spotify` and `nlu` linked in. Consumers: `client-rs`, `host-gateway`. The workspace also holds `tools/codegen` and `desktop/src-tauri`.
- `packages/` is the bun workspace. `webapps/builtin/*` ship with the daemon; `webapps/catalog/*` publish to the store. `companion/{swift,kotlin}` and `asr` are mobile shells over the shared Rust core and sit outside bun.
- `mobile/` is the React Native app. `desktop/` is the Tauri app; `desktop/src-tauri` links `crates/companion` and holds all desktop state. `site/` is bridgething.com.
- Rust crates are kebab-case (`bridgething-mfi`). TS packages are scoped (`@bridgething/lib`).

## `crates/lib` holds the wire types only

Everything serialized across the local WebSocket or the Bluetooth link lives here, with the codec and framing in `crates/lib/src/protocol/`. Its dependencies are `serde`, `ts-rs`, `uuid`, `serde_with`, `derive_more`, and the `protocol` feature deps. Runtime types (`tokio::sync`, handlers, managers, hardware, non-protocol errors) belong in `crates/core`. If a type is useless to a third party speaking the protocol, it is a core type.

## Wrap, don't duplicate

A wire type has one home, `crates/lib`. Every other crate imports it. When core needs runtime-only variants beside a wire enum, wrap the wire enum in a core enum and keep the payloads as lib types (`RecvMsgData` in `crates/core/src/handler/client/msg.rs`). A new field goes in lib and propagates outward. `crates/client-rs` and `packages/browser` re-export lib types.

## Stock translation lives in core

- `crates/core/src/stock/` holds the raw JSON shapes of the stock Spotify webapp. `crates/core/src/handler/client/stock.rs` translates them. They stay in core so the generated bindings stay clean.
- `crates/lib/src/stock/` holds the SDK-facing types a modern webapp uses to invoke legacy operations through the `LegacyStock` command.

## `crates/lib/src/shared/`

For types used in both directions of a protocol and by more than one protocol (`Track`, `Album`, `Device`, `WebappInfo`). A type used in one direction or one protocol stays in that module.

## Codegen

`just codegen` regenerates `crates/lib/ts/bindings/`, `crates/companion/ts/companion.ts`, `crates/*-rs/src/surface.generated.rs`, and `crates/lib/docs/surfaces.json` from `crates/lib/src/`. Run it after any change to a lib type. `just companion-bindings` regenerates the swift and kotlin bindings after any change to the FFI surface in `crates/companion`. Fix generated output at its source: the Rust type, or a transform in `tools/codegen/`.

## Concurrency in core

The daemon is actor-style. `RfcommGateway` in `crates/core/src/bluetooth/rfcomm/mod.rs` is the template for a new subsystem.

- One task owns its state and mutates it inside its `select!` loop. Other tasks send commands over a bounded mpsc (`channel(16)`); the bound is the backpressure. `Arc<RwLock<_>>` is for read-mostly snapshots such as `AppState`, never for a protocol hot path.
- `init() -> Self` constructs (opens sockets, registers profiles). `spawn(self) -> JoinHandle<()>` starts the loop. Store the handle in a `_handle` field so it drops with its owner.
- Byte-stream protocols implement `tokio_util::codec::Decoder`: return `Ok(None)` until a frame is complete, advance one byte to resync. Split with `Framed::split()`.
- In-flight payloads are `bytes::Bytes`. Build in a `BytesMut` and freeze. The device has 512 MB shared with chromium, so clone by refcount, never by heap.
- Errors are `thiserror` enums per subsystem, joined with `#[from]` so `?` crosses layers.
- Tracing: state transitions at `debug!`, frames at `trace!`, handshakes and connection-up at `info!`, error paths at `warn!` or `error!`.

## Commands

- `just dev-daemon` runs the daemon on the host with state under `.dev/` and leaves the Bluetooth adapter alone. `cargo run -p bridgething` reconfigures the host adapter; use it only when you need the radio.
- `cargo build -p bridgething --features superbird --no-default-features` is the on-device build. `just cross-build` runs it in the aarch64 image; `just push` installs the result on the connected device.
- `cargo test -p libbridgething` runs the unit and golden tests. `just goldens` regenerates the fixtures in `crates/lib/tests/`.
- `just test-all` runs every suite; `just test-{rust,kotlin,swift,ts}` runs one. `FORCE=1` bypasses the gradle and turbo caches.

## Style

- Comments in core are for gotchas only. A gotcha is a sign the code needs a refactor.
- Delete dead code. `#[allow(dead_code)]` is banned.
- No em dashes or en dashes.

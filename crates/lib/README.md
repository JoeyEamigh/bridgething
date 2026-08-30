# @bridgething/lib

TypeScript types for the [bridgething](https://github.com/JoeyEamigh/bridgething)
wire protocol, generated from the Rust crate that defines them.

```sh
bun add @bridgething/lib
```

| export                     | what it holds                                   |
| -------------------------- | ----------------------------------------------- |
| `@bridgething/lib`         | Shared types, envelope types, protocol constants |
| `@bridgething/lib/client`  | Messages between a webapp and the daemon        |
| `@bridgething/lib/gateway` | Messages between the daemon and the phone       |
| `@bridgething/lib/shared`  | `Track`, `Album`, and other cross-protocol types |
| `@bridgething/lib/stock`   | Preset types for the stock Spotify app          |
| `@bridgething/lib/wire`    | `MsgMeta` and `WireError`                       |
| `@bridgething/lib/uuid`    | UUID encoding helpers                           |
| `@bridgething/lib/logger`  | The `Logger` the SDKs write through             |

To write a webapp, use
[`@bridgething/client`](https://www.npmjs.com/package/@bridgething/client). To
speak the byte protocol from TypeScript, use
[`@bridgething/browser`](https://www.npmjs.com/package/@bridgething/browser).

- Docs: <https://bridgething.com/docs>
- Source: <https://github.com/JoeyEamigh/bridgething>

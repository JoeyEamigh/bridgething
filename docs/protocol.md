# bridgething wire protocol

A gateway is the phone-side or host-side peer that talks to the daemon over Bluetooth or the network
gateway WebSocket. A client is a webapp that talks to the daemon over the local WebSocket. Generated
TypeScript types for every surface ship in `@bridgething/lib`.

## The two surfaces

| surface | message pair                                | endpoint                                                  | encoding      | framing                  |
| ------- | ------------------------------------------- | --------------------------------------------------------- | ------------- | ------------------------ |
| gateway | `BridgeToGatewayMsg` / `GatewayToBridgeMsg` | RFCOMM, iAP2 EA channel, or network gateway WS (port 8892) | msgpack       | 16-byte frame header     |
| client  | `BridgeToClientMsg` / `ClientToBridgeMsg`   | local WebSocket on port 8891                               | JSON, msgpack | one message per WS frame |

## Frame layout

The client surface uses WebSocket framing and carries no header.

```text
offset  size  field        value
------  ----  -----------  -----
0       2     magic        0xdead, big-endian
2       1     version      2
3       1     compression  0 = none, 1 = gzip
4       1     encoding     0 = msgpack, 1 = json
5       1     priority     0 = normal, 1 = bulk, 2 = background
6       2     reserved     zero
8       8     length       big-endian u64, payload bytes that follow
16      N     payload      one envelope
```

`length` counts the payload after compression.

On RFCOMM and the iAP2 EA channel the frames form a byte stream: one read can hold several frames,
and one frame can span several reads. On the network gateway WebSocket each binary message must hold
whole frames. The daemon closes the connection when a message ends part way through a frame.

The decoder recovers from a bad magic, an unknown version, and a length over the 16 MiB cap by
scanning forward to the next magic.

### Compression

The sender gzips a normal-lane payload once it passes 16256 bytes, and keeps the gzipped form only
when it is smaller. Handle both compression values on receive.

### Encoding

The daemon writes msgpack. Its decoder also accepts JSON. msgpack payloads use named maps, so
decoders match on the field name.

### Priority

| value | lane         | use for                                                        |
| ----- | ------------ | -------------------------------------------------------------- |
| 0     | `normal`     | control, state, and transport messages                         |
| 1     | `bulk`       | large payloads a user is waiting on, such as requested artwork |
| 2     | `background` | transfers with nothing on screen waiting, such as an OTA image |

The writer drains `normal` first, then `bulk`, then `background`. Break a large payload into many
small typed messages so higher lanes interleave between them. An unrecognized byte decodes as
`normal`.

## Envelope

```ts
{
  id: Uuid,      // fresh on every message you send
  meta: MsgMeta, // command | event | request | response
  data: <surface tagged enum>,
}
```

A UUID is 16 raw bytes in msgpack and a hyphenated string in JSON. The TypeScript bindings type it as
`string`.

The outer `type` picks the surface, the inner `event` picks the variant, both camelCase.

```jsonc
{
  "id": "01924f2b-1f7e-7c1a-9f3a-6b2d9e5a4c18",
  "meta": { "kind": "command" },
  "data": {
    "type": "player",
    "data": {
      "event": "play",
      "data": { "uri": "spotify:track:abc", "context": null }
    }
  }
}
```

### Message kinds

| `meta.kind` | sender means       | receiver must                                               |
| ----------- | ------------------ | ----------------------------------------------------------- |
| `command`   | do this            | act on it                                                   |
| `event`     | this happened      | observe it                                                  |
| `request`   | answer this        | send one `response` whose `requestId` is the request's `id` |
| `response`  | here is the answer | match it to a pending request                               |

The variant inside `data` fixes the legal kind.

## Requests and correlation

```jsonc
{
  "id": "<uuid A>",
  "meta": { "kind": "request" },
  "data": {
    "type": "library",
    "data": { "event": "browse", "data": { "nodeId": null, "limit": 14, "offset": 0 } }
  }
}
```

The responder replies with a fresh `id` and echoes the request's id under `meta.data.requestId`:

```jsonc
{
  "id": "<uuid B>",
  "meta": { "kind": "response", "data": { "requestId": "<uuid A>" } },
  "data": {
    "type": "library",
    "data": { "event": "browseReply", "data": { "result": { "entries": [] } } }
  }
}
```

Answer every request. The caller times out otherwise.

When a gateway sends a request variant the daemon cannot decode, the daemon replies with an
`unsupported` `WireError` keyed to the request id, and the pending request resolves immediately.

## Errors

`WireError` arrives as its own surface: `data.type` is `error` and `data.data` is the `WireError`.

```ts
type WireError =
  | { type: 'unsupported' }                            // the receiver does not know this variant
  | { type: 'unimplemented' }                          // the variant is known, the backend is not wired
  | { type: 'malformed'; data: { reason: string } }    // the payload failed to decode or validate
  | { type: 'handlerFailed'; data: { reason: string } }; // the handler hit an internal error
```

A request that can fail predictably also carries a domain error on its own reply variant.
`library.browse` answers with `errorReply`, holding a `LibraryError` (`notFound`, `notSupported`,
`unauthorized`, `noGateway`). The SDKs return both:

```ts
type TypedRequestResult<R, E> =
  | { ok: true; response: R }
  | { ok: false; kind: 'domain'; error: E }
  | { ok: false; kind: 'protocol'; error: WireError };
```

## Transports

### RFCOMM

Bluetooth Classic, one paired peer per connection.

| field          | value                                  |
| -------------- | -------------------------------------- |
| service UUID   | `dead0000-854d-408e-81f0-fb6147f918fd` |
| RFCOMM channel | 1                                      |
| device class   | `0x7c0000`                             |

### iAP2 external accessory channel

The daemon opens the channel after the iAP2 link comes up and runs the same frames over it.

| field          | value                     |
| -------------- | ------------------------- |
| EA protocol id | 1                         |
| protocol name  | `com.bridgething.gateway` |

### Network gateway WebSocket

| field        | value                                              |
| ------------ | -------------------------------------------------- |
| port         | 8892                                               |
| frames       | WebSocket binary messages, each holding whole frames |
| message cap  | 1 MiB                                              |
| peer address | 6 bytes, `0xfe 0xfe` then a big-endian u32 counter |

### Local client WebSocket

Send a text message holding JSON, or a binary message holding msgpack. The daemon replies in
whichever encoding the webapp sent last. `MsgMeta` and `WireError` match the gateway surface, and the
`data` types come from the client surface.

### Stock Spotify WebSocket

The stock Spotify webapp connects on port 8890 and speaks string method names and untyped JSON. The
daemon translates those onto the modern surfaces. See `stock-webapp-gateway-contract.md`.

## Limits

| limit                       | value   |
| --------------------------- | ------- |
| daemon request timeout      | 60 s    |
| SDK request timeout         | 30 s    |
| network gateway message cap | 1 MiB   |
| frame payload cap           | 16 MiB  |
| daemon fragment size        | 4 KiB   |
| in-memory transfer cap      | 256 KiB |

Ship a large payload through the transfer surface. The typed message carries a `TransferBody`,
holding either inline bytes or a `TransferRef`. Stream bytes follow as `transfer.fragment` events on
the lane you choose, in ascending offset order with no gaps.

## Versioning

Ship version 2 frames. The decoder skips any other version byte.

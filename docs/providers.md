# Adding a music provider to the companion

A provider is a Rust type in `crates/companion/src/provider/` that owns one service: its catalog, its
favorites, its lyrics, and what happens when a webapp plays one of its uris. Spotify and Apple Music
drive a third-party app on the phone. Subsonic plays the audio itself through the companion's stream
playback engine, which is the shape a Jellyfin, Plex, Funkwhale, or podcast provider takes: pure HTTP
against the service, no native code.

`provider/subsonic/` is the worked example. Read it alongside this page.

## What you write

1. **A client** for the service's HTTP API (`subsonic/client.rs`). Requests go through the
   `HttpExecutor` the provider is built with, never a direct `reqwest`. That executor is the phone's
   own transport, so the request survives backgrounding and honors the platform's network policy.
2. **A provider** implementing `Provider` and `PlayerTransport` from `provider/mod.rs`
   (`subsonic/mod.rs`).
   - `name()` is the id the apps and the daemon see. `uri_schemes()` declares the scheme you own; the
     daemon routes `player.play` by scheme, so pick one nobody else claims (`subsonic:`).
   - `music_provider()` returns a `MusicProvider` variant. Add one to
     `crates/lib/src/shared/capabilities.rs` and run `just codegen`.
   - Browse, search, recommendations, and favorites map the service's objects onto `LibraryItem`,
     `BrowseFolder`, and friends from `libbridgething`. Node ids are your own strings; container uris
     double as node ids so a webapp can descend into an album it found in search.
   - Artwork is an asset id, never a url. Build one `ImageAssetCodec` with your own namespace
     (`subsonic/img/`), mint ids from it, and serve them from `asset()` through an `ArtCache`. Keep
     credentials out of the id: Subsonic ids carry a cover id and the provider turns it into an
     authenticated url only when the bytes are fetched.
3. **Playback through `StreamPlayback`** (`provider/playback/`). Build one with your stream backend,
   http transport, art cache, source id, and an `ArtResolver` that turns your artwork sources into
   fetchable urls. Then:
   - `play(entries, start, context)` takes a list of `QueueEntry` (a stream url plus a
     `Presentation`: uri, title, artist, album, artwork source, duration, liked). The engine plays the
     entry at `start`, publishes the presentation on the wire under your source id, walks the list on
     `skip_next`, `skip_prev`, `skip_to_index`, advances when the phone reports the track ended, and
     sends the upcoming list as the queue.
   - `enqueue(entries, position)` inserts into the running list or starts one.
   - Whatever the stream reports about itself (embedded tags, ICY titles, artwork bytes) wins over
     the presentation, so a radio station shows its live title and a catalog track shows its tags.
   - Call `attach` and `detach` from your provider's `attach` and `detach`, and `set_art_edges`
     from `set_art_profile`. Playback belongs to the phone: a Car Thing leaving does not stop it,
     the lock screen and the media notification do.
4. **A catalog entry** implementing `CatalogEntry` in `provider/catalog.rs`. It says how the
   provider signs in (`SignInMethod::Handshake` when the provider drives its own flow, `ServerLogin`
   when the user types a server, username, and password), stores and clears the credentials in the
   host's `SecretStore`, and builds the provider. Chain it into `ProviderCatalog::new` in
   `session/mod.rs`, gated on whatever host backends it needs. Subsonic is gated on the stream
   backend, so a host that cannot play audio never offers it.

## What you do not write

- No settings screen. The apps render every catalog entry from `ProviderInfo`; a `ServerLogin`
  entry gets the server, username, and password form on iOS, Android, and the desktop, and
  `complete_provider_auth` delivers the credentials to your entry.
- No now-playing plumbing. `StreamPlayback` submits snapshots, queues, and the lock-screen
  presentation to the hub; the hub arbitrates between providers and talks to the device.
- No native playback. The stream backend on each platform already plays a url and reports status,
  timing, and metadata back.

## Tests

`crates/companion/tests/subsonic.rs` drives the provider against a loopback stub of the service
(`tests/rig/subsonic.rs`) through a fake stream backend and the real hub, and checks what reaches
the wire. `tests/subsonic_live.rs` runs the same shape against a real server when
`BRIDGETHING_SUBSONIC_LIVE_URL`, `BRIDGETHING_SUBSONIC_LIVE_USERNAME`, and
`BRIDGETHING_SUBSONIC_LIVE_PASSWORD` are set. Write both for a new provider: the stub proves the
mapping, the live lane proves the API shapes.

After changing the FFI surface (a new `CompanionSession` method, a record, an enum) run
`just companion-bindings`; after changing a lib wire type run `just codegen`.

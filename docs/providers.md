# Adding a music provider to the companion

A provider is a Rust type in `crates/companion/src/provider/` that owns one service: its catalog, its
favorites, its lyrics, and what happens when a webapp plays one of its uris. Spotify and Apple Music
drive a third-party app on the phone. Subsonic and Plex play the audio themselves through the companion's
stream playback engine, which is the shape a Jellyfin, Funkwhale, or podcast provider takes: pure HTTP
against the service, no native code.

`provider/subsonic/` is the worked example for a server the user types in. `provider/plex/` is the one for a
service with its own sign-in and several servers behind one account. Read one alongside this page.

## What you write

1. **A client** for the service's HTTP API (`subsonic/client.rs`). Requests go through the
   `HttpExecutor` the provider is built with, never a direct `reqwest`. That executor is the phone's
   own transport, so the request survives backgrounding and honors the platform's network policy.
2. **A provider** implementing `Provider` and `PlayerTransport` from `provider/mod.rs`
   (`subsonic/mod.rs`).
   - `name()` is the id the apps and the daemon see. `uri_schemes()` declares the scheme you own; the
     daemon routes `player.play` by scheme, so pick one nobody else claims (`subsonic:`).
   - `music_provider()` returns a `MusicProvider` variant. Add one to
     `crates/lib/src/shared/capabilities.rs` above `None` (which stays last so daemons read any provider
     they do not know as `None`), give it the first daemon version that ships it in
     `crates/companion/src/session/downlevel.rs`, and run `just codegen`. The companion always updates
     before the daemon, since it is what delivers daemon updates, so a daemon older than your provider is
     told `None` instead of a value it cannot decode.
   - Browse, search, recommendations, and favorites map the service's objects onto `LibraryItem`,
     `BrowseFolder`, and friends from `libbridgething`. Node ids are your own strings; container uris
     double as node ids so a webapp can descend into an album it found in search.
   - Artwork is an asset id, never a url. Build one `ImageAssetCodec` with your own namespace
     (`subsonic/img/`), mint ids from it, and serve them from `asset()` through the session's shared
     `ArtCache`, which your constructor takes and which keeps art on the phone's disk across
     restarts. Pass your `HttpExecutor` as the `ArtOrigin` that fetches a miss. Keep credentials out
     of the id: Subsonic ids carry a cover id and the provider turns it into an authenticated url
     only when the bytes are fetched.
   - The staple root (playlists, albums, artists, recently played, recently added, random) and the
     paging helpers live in `provider/library.rs`. Call `staple_root` with your node lookup instead of
     building the folders yourself.
   - Library answers are cached for you: the dispatcher keeps every browse, search,
     recommendations, favorites list, and context resolve on disk under your provider name, serves
     it when the phone is offline or slow, and warms the art ids it lists.
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
   - Pass a `PlayReporter` to report listening back to the service, or `None`. The engine calls it with
     `Started` once a track actually plays, `Progress` (playing, paused, stopped, with position and
     duration) on every state change and every ten seconds, and `Played` once per track when half of it
     or four minutes has played, or it ended. Subsonic scrobbles on `Started` and `Played`; Plex sends
     its timeline on every `Progress` and lets the server count the play. Live streams never report.
   - Call `attach` and `detach` from your provider's `attach` and `detach`, and `set_art_edges`
     from `set_art_profile`. Playback belongs to the phone: a Car Thing leaving does not stop it,
     the lock screen and the media notification do.
4. **A catalog entry** implementing `CatalogEntry` in `provider/catalog.rs`. It says how the
   provider signs in (`SignInMethod::Handshake` when the provider drives its own flow, `ServerLogin`
   when the user types a server, username, and password), stores and clears the credentials in the
   host's `SecretStore`, and builds the provider. A `Handshake` provider runs its flow from `attach`
   and reports it through the auth observer: `Pending` with `verification_url_complete` set to the page
   the apps open for the user, then `Authenticated` once it holds a token (Plex's PIN flow is the
   example). Chain it into `ProviderCatalog::new` in
   `session/mod.rs`, gated on whatever host backends it needs. Subsonic is gated on the stream
   backend, so a host that cannot play audio never offers it.
5. **Voice** through `voice_resolver::Catalog` (`subsonic/voice.rs`). The shared resolver in
   `crates/voice-resolver` turns "play the first album by X" into one of your uris; you only answer its
   questions: search, artist releases and top songs, container tracks, what is playing, the user's
   library (liked songs, saved albums, followed artists, playlists with ownership), recents, and a
   station for a seed. `kind_of` reads your uri grammar. The defaults cover what a service lacks:
   no popularity signal keeps relevance order, no new-release feed falls back to search, no personal
   mixes skips them. Return the resolver from `voice_resolver()` once signed in:
   `CatalogResolver::new(core)`. "Liked songs" must be a uri your `play` accepts, even when the
   service has no such playlist (Subsonic mints `subsonic:starred:songs`).

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
the wire, including the voice picks through `voice_resolver()`. `tests/subsonic_live.rs` runs the same shape against a real server when
`BRIDGETHING_SUBSONIC_LIVE_URL`, `BRIDGETHING_SUBSONIC_LIVE_USERNAME`, and
`BRIDGETHING_SUBSONIC_LIVE_PASSWORD` are set. `tests/plex.rs` and `tests/plex_live.rs`
are the same pair for Plex; the live lane takes an account token in `BRIDGETHING_PLEX_LIVE_TOKEN` and finds
the servers itself. Write both for a new provider: the stub proves the mapping, the live lane proves the
API shapes.

After changing the FFI surface (a new `CompanionSession` method, a record, an enum) run
`just companion-bindings`; after changing a lib wire type run `just codegen`.

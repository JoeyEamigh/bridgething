# Gateway contract for the stock Spotify webapp

The stock Spotify webapp is the fixed Chromium page that ships in the original Car Thing firmware. It
speaks string method names and untyped JSON. The daemon translates those calls onto the modern wire
surfaces, so a gateway populates it through the same surfaces it already implements.

Implement four things:

1. Answer `library.browse` requests with a `BrowseResult`.
2. Answer `library.resolveContext` requests with a name and an artwork id.
3. Answer `asset.request` requests with the image bytes.
4. Push `player.snapshot` and `player.queueChanged` events.

## What the daemon translates

| stock call                               | modern surface              | what your gateway receives                           |
| ---------------------------------------- | --------------------------- | ---------------------------------------------------- |
| `com.spotify.superbird.get_home`         | `library.browse`            | `nodeId: null`, `sections: 10`                       |
| `com.spotify.get_children_of_item`       | `library.browse`            | `nodeId` set to the parent id                        |
| `com.spotify.superbird.get_podcast`      | `library.browse`            | `nodeId` set to the show URI                         |
| graphql `shelf`                          | `library.browse`            | `nodeId: null`, `sections: 10`, default limit 14     |
| graphql `section`                        | `library.browse`            | `nodeId` set to the section id, default limit 20     |
| `com.spotify.play_uri`                   | `player.play`               | `{ uri, context: null }`                             |
| `com.spotify.queue_spotify_uri`          | `player.queue`              | `{ uri, position: 'append' }`                        |
| `com.spotify.set_saved`                  | `library.favoritesSet`      | `{ item: { uri, kind: 'track' }, liked }`            |
| `com.spotify.get_saved`                  | `library.favoritesContains` | `{ uris: [uri] }`                                    |
| `com.spotify.set_podcast_playback_speed` | `player.setSpeed`           | `{ speed }` as a float multiplier                    |
| `com.spotify.get_image`                  | `asset.request`             | the artwork id with its edge segment set to 248      |
| `com.spotify.get_thumbnail_image`        | `asset.request`             | the artwork id with its edge segment set to 96       |
| `com.spotify.superbird.earcon`           | `audio.earcon`              | `{ name: 'confirmation' \| 'listening' \| 'error' }` |
| `com.spotify.superbird.tts.speak`        | `audio.earcon`              | `{ name: 'spotify-stock:<file>' }`                   |

## library.browse

```ts
type LibraryBrowseRequest = {
  nodeId: string | null; // null is the home root
  limit: number; // the daemon caps this at 100
  offset: number;
  sections: number | null; // how many home shelves the caller wants
  preview: number | null; // how many preview children per shelf
};

type BrowseReply = { result: BrowseResult };

type BrowseResult = {
  entries: BrowseEntry[];
  total: number | null; // null when the count is indeterminate
  hasMore: boolean; // the end-of-data signal callers paginate against
};

type BrowseEntry = { type: 'folder'; data: BrowseFolder } | { type: 'item'; data: LibraryItem };

type BrowseFolder = {
  nodeId: string; // your namespace, opaque to the daemon
  title: string;
  subtitle: string | null;
  artworkId: string | null;
  total: number | null;
  previewChildren: BrowseEntry[] | null;
};

type LibraryItem =
  | { type: 'track'; data: Track }
  | { type: 'album'; data: Album }
  | { type: 'playlist'; data: Playlist }
  | { type: 'podcastEpisode'; data: PodcastEpisode }
  | { type: 'show'; data: Show }
  | { type: 'artist'; data: Artist }
  | { type: 'station'; data: Station };
```

### The home request

`nodeId` is `null` and `offset` is `0`. Return folders with `previewChildren` populated. Each folder
becomes one home shelf and its `previewChildren` become that shelf's cards. The daemon drops any item
entry at the root. Set each folder's `total` to the number of items behind that shelf.

```ts
{
  type: 'folder',
  data: {
    nodeId: 'home:recently-played',
    title: 'Recently played',
    artworkId: null,
    total: 50,
    previewChildren: [
      { type: 'folder', data: { nodeId: 'spotify:playlist:abc', title: 'Discover Weekly', subtitle: 'Spotify', artworkId: 'gw/img/300/dw' } },
    ],
  },
}
```

### The section request

`nodeId` is a value you returned earlier, either a synthetic shelf id or a Spotify URI. Return the
entries under that node.

The webapp paginates by raising `offset` until `hasMore` is false, so set `hasMore` correctly on
every page. Set `total` when you can get the count cheaply. When `total` is null the daemon
synthesizes a count from `offset`, the entry count, and `hasMore`.

| entry            | plays on tap | drills down |
| ---------------- | ------------ | ----------- |
| `folder`         | no           | yes         |
| `track`          | yes          | no          |
| `podcastEpisode` | yes          | no          |
| `station`        | yes          | no          |
| `album`          | yes          | yes         |
| `playlist`       | yes          | yes         |
| `show`           | no           | yes         |
| `artist`         | no           | yes         |

### The podcast request

Opening a show sends a browse request with the show URI as `nodeId`. Return the show's episodes as
`podcastEpisode` items.

## Node ids

The stock webapp echoes back the ids you send, so your gateway owns the namespace.

- Give anything playable its real `spotify:<kind>:<base62>` URI as its `id` or `uri`. The webapp
  sends that URI straight back on `player.play`.
- Give shelves and pseudo-folders a synthetic id under a prefix you route on, such as `home:` or
  `gw:`. The daemon passes these through untouched.
- Name the recently-played folder `recently-played`, the shared convention across gateways.

## Player state

Push `player.snapshot` with a full `PlayerState` whenever your side changes, and
`player.queueChanged` with a `QueueSnapshot` when the queue changes. The daemon caches the merged
result and serves the stock webapp's reads from it.

The daemon advances the playhead from its own clock while `playback.state` is `playing`, using
`positionMs` and `track.durationMs` from your last snapshot. Push a snapshot when the position jumps
after a seek or a track change.

| field                             | what it drives                                               |
| --------------------------------- | ------------------------------------------------------------ |
| `track.title`                     | the track line                                               |
| `track.artist`, `track.artistUri` | the artist line and its link                                 |
| `track.album`, `track.albumUri`   | the album line and its link                                  |
| `track.artworkId`                 | the artwork the card fetches                                 |
| `track.durationMs`                | the seek bar length                                          |
| `track.liked`                     | the heart button                                             |
| `track.uri`                       | the track identity, falling back to `track.persistentId`     |
| `playback.state`                  | the play button. Any value other than `playing` shows paused |
| `playback.positionMs`             | the seek bar position                                        |
| `playback.shuffle`                | the shuffle toggle                                           |
| `playback.repeat`                 | the repeat toggle                                            |
| `context.uri`                     | the playing-from line and preset highlighting                |
| `context.name`                    | the playing-from title                                       |
| `options.speed`                   | the podcast speed control, as a multiplier such as 1.5       |

Set `options.speed` to a finite value above zero. The daemon substitutes 1.0 for anything else. Set
`context.uri` on every snapshot. The daemon falls back to `track.uri` when it is empty, which points
the playing-from line at the track itself.

`player.queueChanged` carries `{ order, items }`. Each `QueueItem` needs a `uri`, which is how the
webapp addresses a skip-to-index. Set `queued` to `true` on items the user queued by hand.

## Artwork

The daemon pulls artwork bytes on demand and caches them, so publish an id and serve it when asked.

The daemon sends `asset.request { id, requestId }`. Answer with one of:

- `asset.got { id, mime, body: { type: 'inline', data: <bytes> } }` for a small image.
- `asset.got { id, mime, body: { type: 'stream', data: { id, totalSize, sha256 } } }` for a larger
  one, followed by `transfer.fragment` events on the bulk lane. Set the `TransferRef` `id` to the
  request's `requestId`, send fragments in ascending offset order with no gaps, and keep the total
  under 256 KiB.
- `asset.notFound { id }` when you have no bytes for that id.

Give an artwork id the shape `<namespace>/img/<edge>/<rest>`, for example `gw/img/300/abc123`. The
daemon rewrites the edge segment to `248` for a hero image and `96` for a thumbnail before it asks
you, so serve the requested size. An id in any other shape reaches you unchanged.

Namespace your ids. The daemon owns `iap2/art/` for artwork it pulls off an iPhone over iAP2, and
`builtin/img/` for images that ship with the daemon.

## Presets

The user fills a preset slot by holding a physical preset button. Pressing a filled button plays the
slot's stored context URI, which reaches your gateway as `player.play`.

When the user saves a slot, the daemon sends `library.resolveContext { uri }` and stores what you
return as the preset's name and artwork:

```ts
type ContextResolveReply = {
  name: string | null;
  artworkId: string | null;
  subtitle: string | null;
};
```

A webapp can read or seed a slot through `client.store` at the keys `presets:1` through `presets:4`
in the nil-UUID scope. The stored value is JSON with snake_case keys:

```ts
type StockPreset = {
  context_uri: string;
  image_url: string | null;
  slot_index: number; // 1 to 4
  name: string | null;
  description: string | null;
};
```

## Smoke test

1. Answer a `nodeId === null` browse with one folder holding one preview track. Home shows one shelf
   with one card.
2. Answer the `asset.request` for that card's artwork id. The card shows your image.
3. Handle `player.play`. Tap the card, then push a `player.snapshot` with `track`, `context`, and
   `playback.state` set to `playing`. The now-playing card fills in.
4. Leave `playback.state` on `playing` and set `track.durationMs`. The seek bar advances on its own.

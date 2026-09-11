use std::{
  collections::{BTreeSet, HashMap},
  io::{Read, Write},
  net::{TcpListener, TcpStream},
  sync::{Arc, Mutex},
  thread,
};

use md5::{Digest, Md5};
use serde_json::{Value, json};

pub const USERNAME: &str = "demo";
pub const PASSWORD: &str = "demo-pass";
pub const COVER_BYTES: &[u8] = b"cover-jpeg";
pub const AUDIO_BYTES: &[u8] = b"mp3-frames-go-here";

#[derive(Clone)]
pub struct Song {
  pub id: &'static str,
  pub title: &'static str,
  pub album: &'static str,
  pub track: u32,
  pub duration: u32,
}

#[derive(Clone)]
pub struct Album {
  pub id: &'static str,
  pub name: &'static str,
  pub cover: &'static str,
  pub songs: Vec<Song>,
}

#[derive(Clone)]
pub struct Artist {
  pub id: &'static str,
  pub name: &'static str,
  pub albums: Vec<&'static str>,
}

#[derive(Clone)]
pub struct Playlist {
  pub id: &'static str,
  pub name: &'static str,
  pub owner: &'static str,
  pub songs: Vec<&'static str>,
}

pub struct Library {
  pub artists: Vec<Artist>,
  pub albums: Vec<Album>,
  pub playlists: Vec<Playlist>,
  pub lyrics: HashMap<&'static str, Vec<(u32, &'static str)>>,
}

pub fn library() -> Library {
  let mut lyrics = HashMap::new();
  lyrics.insert("s1", vec![(0, "first line"), (4_200, "second line")]);
  Library {
    artists: vec![
      Artist {
        id: "ar1",
        name: "Pornophonique",
        albums: vec!["al1", "al2"],
      },
      Artist {
        id: "ar2",
        name: "Maya Filipic",
        albums: vec!["al3"],
      },
    ],
    albums: vec![
      Album {
        id: "al1",
        name: "8-bit lagerfeuer",
        cover: "al-1",
        songs: vec![
          Song {
            id: "s1",
            title: "Sad Robot",
            album: "8-bit lagerfeuer",
            track: 1,
            duration: 212,
          },
          Song {
            id: "s2",
            title: "Space Invaders",
            album: "8-bit lagerfeuer",
            track: 2,
            duration: 198,
          },
          Song {
            id: "s3",
            title: "I Want To Be A Machine",
            album: "8-bit lagerfeuer",
            track: 3,
            duration: 240,
          },
        ],
      },
      Album {
        id: "al2",
        name: "Second Album",
        cover: "al-2",
        songs: vec![
          Song {
            id: "s4",
            title: "Game Over",
            album: "Second Album",
            track: 1,
            duration: 180,
          },
          Song {
            id: "s5",
            title: "Lemmings In Love",
            album: "Second Album",
            track: 2,
            duration: 201,
          },
        ],
      },
      Album {
        id: "al3",
        name: "Between two worlds",
        cover: "al-3",
        songs: vec![Song {
          id: "s6",
          title: "Stories from Emona I",
          album: "Between two worlds",
          track: 1,
          duration: 265,
        }],
      },
    ],
    playlists: vec![Playlist {
      id: "pl1",
      name: "Road trip",
      owner: "demo",
      songs: vec!["s2", "s6"],
    }],
    lyrics,
  }
}

#[derive(Default)]
struct State {
  requests: Vec<String>,
  starred: BTreeSet<String>,
}

pub struct SubsonicStub {
  pub url: String,
  state: Arc<Mutex<State>>,
}

impl SubsonicStub {
  pub fn serve() -> Self {
    Self::serve_with(library(), PASSWORD)
  }

  pub fn serve_with(library: Library, password: &'static str) -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port for the subsonic stub");
    let port = listener.local_addr().expect("a bound address").port();
    let state = Arc::new(Mutex::new(State::default()));
    let library = Arc::new(library);
    let shared = Arc::clone(&state);
    thread::spawn(move || {
      for accepted in listener.incoming() {
        let Ok(socket) = accepted else { continue };
        let library = Arc::clone(&library);
        let shared = Arc::clone(&shared);
        thread::spawn(move || answer(socket, &library, password, &shared));
      }
    });
    SubsonicStub {
      url: format!("http://127.0.0.1:{port}"),
      state,
    }
  }

  pub fn requests(&self) -> Vec<String> {
    self.state.lock().unwrap().requests.clone()
  }

  pub fn endpoints(&self) -> Vec<String> {
    self
      .requests()
      .iter()
      .filter_map(|line| {
        line
          .split('?')
          .next()
          .map(|path| path.trim_start_matches("/rest/").to_owned())
      })
      .collect()
  }

  pub fn starred(&self) -> Vec<String> {
    self.state.lock().unwrap().starred.iter().cloned().collect()
  }
}

fn answer(mut socket: TcpStream, library: &Library, password: &str, state: &Arc<Mutex<State>>) {
  let mut head = Vec::new();
  let mut byte = [0u8; 1];
  while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
    match socket.read(&mut byte) {
      Ok(1) => head.push(byte[0]),
      _ => return,
    }
  }
  let text = String::from_utf8_lossy(&head);
  let Some(target) = text.lines().next().and_then(|line| line.split(' ').nth(1)) else {
    return;
  };
  state.lock().unwrap().requests.push(target.to_owned());
  let parsed = url::Url::parse(&format!("http://stub{target}")).expect("a parseable request target");
  let query: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
  let endpoint = parsed.path().trim_start_matches("/rest/").to_owned();

  let authorized = query.get("u").map(String::as_str) == Some(USERNAME)
    && query.get("t").zip(query.get("s")).is_some_and(|(token, salt)| {
      let digest: String = Md5::digest(format!("{password}{salt}").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
      *token == digest
    });
  if !authorized {
    respond_json(&mut socket, &failed(40, "Wrong username or password"));
    return;
  }

  match endpoint.as_str() {
    "stream" => respond_bytes(&mut socket, "audio/mpeg", AUDIO_BYTES),
    "getCoverArt" => respond_bytes(
      &mut socket,
      "image/jpeg",
      format!(
        "{}:{}",
        String::from_utf8_lossy(COVER_BYTES),
        query.get("id").cloned().unwrap_or_default()
      )
      .as_bytes(),
    ),
    _ => {
      let body = route(&endpoint, &query, library, state);
      respond_json(&mut socket, &body);
    }
  }
}

fn ok(payload: Value) -> Value {
  let mut reply = json!({ "status": "ok", "version": "1.16.1", "type": "stub", "openSubsonic": true });
  if let (Value::Object(reply), Value::Object(payload)) = (&mut reply, payload) {
    reply.extend(payload);
  }
  json!({ "subsonic-response": reply })
}

fn failed(code: u32, message: &str) -> Value {
  json!({ "subsonic-response": { "status": "failed", "version": "1.16.1", "error": { "code": code, "message": message } } })
}

fn song_json(song: &Song, album: &Album, artist: &Artist, starred: &BTreeSet<String>) -> Value {
  let mut value = json!({
    "id": song.id,
    "title": song.title,
    "album": song.album,
    "albumId": album.id,
    "artist": artist.name,
    "artistId": artist.id,
    "track": song.track,
    "duration": song.duration,
    "coverArt": album.cover,
    "isDir": false,
  });
  if starred.contains(&format!("song:{}", song.id)) {
    value["starred"] = json!("2026-09-10T00:00:00Z");
  }
  value
}

fn album_json(album: &Album, artist: &Artist, starred: &BTreeSet<String>) -> Value {
  let mut value = json!({
    "id": album.id,
    "name": album.name,
    "artist": artist.name,
    "artistId": artist.id,
    "coverArt": album.cover,
    "songCount": album.songs.len(),
  });
  if starred.contains(&format!("album:{}", album.id)) {
    value["starred"] = json!("2026-09-10T00:00:00Z");
  }
  value
}

fn artist_json(artist: &Artist, starred: &BTreeSet<String>) -> Value {
  let mut value = json!({ "id": artist.id, "name": artist.name, "albumCount": artist.albums.len() });
  if starred.contains(&format!("artist:{}", artist.id)) {
    value["starred"] = json!("2026-09-10T00:00:00Z");
  }
  value
}

fn artist_of<'a>(library: &'a Library, album: &Album) -> &'a Artist {
  library
    .artists
    .iter()
    .find(|artist| artist.albums.contains(&album.id))
    .expect("every fixture album has an artist")
}

fn album_of<'a>(library: &'a Library, song_id: &str) -> Option<(&'a Album, &'a Song)> {
  library.albums.iter().find_map(|album| {
    album
      .songs
      .iter()
      .find(|song| song.id == song_id)
      .map(|song| (album, song))
  })
}

fn route(endpoint: &str, query: &HashMap<String, String>, library: &Library, state: &Arc<Mutex<State>>) -> Value {
  let id = query.get("id").cloned().unwrap_or_default();
  let starred = state.lock().unwrap().starred.clone();
  let starred = &starred;
  let size = query
    .get("size")
    .and_then(|size| size.parse::<usize>().ok())
    .unwrap_or(10);
  let offset = query
    .get("offset")
    .and_then(|offset| offset.parse::<usize>().ok())
    .unwrap_or(0);
  match endpoint {
    "ping" => ok(json!({})),
    "getArtists" => ok(json!({ "artists": { "index": [{
      "name": "A-Z",
      "artist": library.artists.iter().map(|artist| artist_json(artist, starred)).collect::<Vec<_>>(),
    }] } })),
    "getArtist" => match library.artists.iter().find(|artist| artist.id == id) {
      Some(artist) => {
        let mut value = artist_json(artist, starred);
        value["album"] = Value::Array(
          library
            .albums
            .iter()
            .filter(|album| artist.albums.contains(&album.id))
            .map(|album| album_json(album, artist, starred))
            .collect(),
        );
        ok(json!({ "artist": value }))
      }
      None => failed(70, "Artist not found"),
    },
    "getAlbum" => match library.albums.iter().find(|album| album.id == id) {
      Some(album) => {
        let artist = artist_of(library, album);
        let mut value = album_json(album, artist, starred);
        value["song"] = Value::Array(
          album
            .songs
            .iter()
            .map(|song| song_json(song, album, artist, starred))
            .collect(),
        );
        ok(json!({ "album": value }))
      }
      None => failed(70, "Album not found"),
    },
    "getAlbumList2" => {
      let kind = query.get("type").map(String::as_str).unwrap_or("alphabeticalByName");
      let mut albums: Vec<&Album> = library.albums.iter().collect();
      match kind {
        "alphabeticalByName" => albums.sort_by_key(|album| album.name.to_lowercase()),
        "recent" | "newest" => albums.reverse(),
        _ => {}
      }
      let page: Vec<Value> = albums
        .iter()
        .skip(offset)
        .take(size)
        .map(|album| album_json(album, artist_of(library, album), starred))
        .collect();
      ok(json!({ "albumList2": { "album": page } }))
    }
    "getPlaylists" => ok(
      json!({ "playlists": { "playlist": library.playlists.iter().map(|playlist| json!({
      "id": playlist.id, "name": playlist.name, "owner": playlist.owner, "songCount": playlist.songs.len(),
    })).collect::<Vec<_>>() } }),
    ),
    "getPlaylist" => match library.playlists.iter().find(|playlist| playlist.id == id) {
      Some(playlist) => ok(json!({ "playlist": {
        "id": playlist.id, "name": playlist.name, "owner": playlist.owner, "songCount": playlist.songs.len(),
        "entry": playlist.songs.iter().filter_map(|song_id| album_of(library, song_id)).map(|(album, song)| {
          song_json(song, album, artist_of(library, album), starred)
        }).collect::<Vec<_>>(),
      } })),
      None => failed(70, "Playlist not found"),
    },
    "getSong" => match album_of(library, &id) {
      Some((album, song)) => ok(json!({ "song": song_json(song, album, artist_of(library, album), starred) })),
      None => failed(70, "Song not found"),
    },
    "getRandomSongs" => {
      let songs: Vec<Value> = library
        .albums
        .iter()
        .flat_map(|album| {
          album
            .songs
            .iter()
            .map(move |song| song_json(song, album, artist_of(library, album), starred))
        })
        .take(size)
        .collect();
      ok(json!({ "randomSongs": { "song": songs } }))
    }
    "search3" => {
      let needle = query.get("query").cloned().unwrap_or_default().to_lowercase();
      let count = |name: &str| {
        query
          .get(name)
          .and_then(|count| count.parse::<usize>().ok())
          .unwrap_or(0)
      };
      let songs: Vec<Value> = library
        .albums
        .iter()
        .flat_map(|album| {
          album
            .songs
            .iter()
            .filter(|song| song.title.to_lowercase().contains(&needle))
            .map(move |song| song_json(song, album, artist_of(library, album), starred))
        })
        .take(count("songCount"))
        .collect();
      let albums: Vec<Value> = library
        .albums
        .iter()
        .filter(|album| album.name.to_lowercase().contains(&needle))
        .map(|album| album_json(album, artist_of(library, album), starred))
        .take(count("albumCount"))
        .collect();
      let artists: Vec<Value> = library
        .artists
        .iter()
        .filter(|artist| artist.name.to_lowercase().contains(&needle))
        .map(|artist| artist_json(artist, starred))
        .take(count("artistCount"))
        .collect();
      ok(json!({ "searchResult3": { "song": songs, "album": albums, "artist": artists } }))
    }
    "getStarred2" => {
      let songs: Vec<Value> = library
        .albums
        .iter()
        .flat_map(|album| {
          album
            .songs
            .iter()
            .filter(|song| starred.contains(&format!("song:{}", song.id)))
            .map(move |song| song_json(song, album, artist_of(library, album), starred))
        })
        .collect();
      let albums: Vec<Value> = library
        .albums
        .iter()
        .filter(|album| starred.contains(&format!("album:{}", album.id)))
        .map(|album| album_json(album, artist_of(library, album), starred))
        .collect();
      let artists: Vec<Value> = library
        .artists
        .iter()
        .filter(|artist| starred.contains(&format!("artist:{}", artist.id)))
        .map(|artist| artist_json(artist, starred))
        .collect();
      ok(json!({ "starred2": { "song": songs, "album": albums, "artist": artists } }))
    }
    "star" | "unstar" => {
      let key = if let Some(album) = query.get("albumId") {
        format!("album:{album}")
      } else if let Some(artist) = query.get("artistId") {
        format!("artist:{artist}")
      } else {
        format!("song:{id}")
      };
      let mut state = state.lock().unwrap();
      if endpoint == "star" {
        state.starred.insert(key);
      } else {
        state.starred.remove(&key);
      }
      ok(json!({}))
    }
    "getLyrics" => {
      let title = query.get("title").cloned().unwrap_or_default();
      let text = library
        .albums
        .iter()
        .flat_map(|album| album.songs.iter())
        .find(|song| song.title == title)
        .and_then(|song| library.lyrics.get(song.id))
        .map(|lines| lines.iter().map(|(_, text)| *text).collect::<Vec<_>>().join("\n"));
      match text {
        Some(text) => ok(json!({ "lyrics": { "artist": query.get("artist"), "title": title, "value": text } })),
        None => ok(json!({ "lyrics": {} })),
      }
    }
    "getLyricsBySongId" => match library.lyrics.get(id.as_str()) {
      Some(lines) => ok(json!({ "lyricsList": { "structuredLyrics": [{
        "lang": "xxx", "synced": true, "offset": 0,
        "line": lines.iter().map(|(start, text)| json!({ "start": start, "value": text })).collect::<Vec<_>>(),
      }] } })),
      None => ok(json!({ "lyricsList": { "structuredLyrics": [] } })),
    },
    _ => failed(0, &format!("the stub does not serve {endpoint}")),
  }
}

fn respond_json(socket: &mut TcpStream, body: &Value) {
  respond_bytes(socket, "application/json", body.to_string().as_bytes());
}

fn respond_bytes(socket: &mut TcpStream, content_type: &str, body: &[u8]) {
  let head = format!(
    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
    body.len()
  );
  let _ = socket.write_all(head.as_bytes());
  let _ = socket.write_all(body);
  let _ = socket.flush();
}

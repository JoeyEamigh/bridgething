use std::{
  collections::HashMap,
  io::{Read, Write},
  net::{TcpListener, TcpStream},
  sync::{Arc, Mutex},
  thread,
};

use serde_json::{Value, json};

pub const CLIENT_ID: &str = "rig-client";
pub const ACCOUNT_TOKEN: &str = "account-token";
pub const PIN_ID: u64 = 4242;
pub const PIN_CODE: &str = "RIGPIN42";
pub const TV: &str = "plex.tv";
pub const MACHINE_A: &str = "machine-a";
pub const MACHINE_B: &str = "machine-b";
pub const MACHINE_C: &str = "machine-c";
pub const TOKEN_A: &str = "token-a";
pub const TOKEN_B: &str = "token-b";
pub const TOKEN_C: &str = "token-c";
pub const AUDIO_BYTES: &[u8] = b"audio-frames-go-here";
pub const SYNCED_LYRICS: &str = "[00:00.00]first line\n[00:04.20]second line";
pub const PLAIN_LYRICS: &str = "plain words\nmore words";
pub const UNTIMED_LRC: &str = "words without any timing";
pub const EMONA_LYRICS: &str = "emona words\nsecond verse";
const STAMP: i64 = 1_700_000_000;

#[derive(Clone, Copy)]
pub struct Script {
  pub pin_after: u32,
  pub revoked: bool,
}

impl Default for Script {
  fn default() -> Self {
    Script {
      pin_after: 2,
      revoked: false,
    }
  }
}

#[derive(Debug, Clone)]
pub struct Seen {
  pub host: &'static str,
  pub method: String,
  pub path: String,
  pub query: HashMap<String, String>,
  pub token: Option<String>,
  pub accept: Option<String>,
}

#[derive(Clone, Default)]
struct Item {
  key: &'static str,
  kind: &'static str,
  title: &'static str,
  guid: Option<&'static str>,
  title_sort: Option<&'static str>,
  parent: Option<&'static str>,
  index: Option<u32>,
  duration: Option<u32>,
  codec: Option<&'static str>,
  added_at: Option<i64>,
  viewed_at: Option<i64>,
  view_count: Option<u32>,
  rating_count: Option<u64>,
  rated_at: Option<i64>,
  lyrics: Vec<(&'static str, &'static str)>,
  similar: Vec<&'static str>,
  nearest: Vec<&'static str>,
  members: Vec<&'static str>,
}

fn artist(key: &'static str, title: &'static str, guid: &'static str) -> Item {
  Item {
    key,
    kind: "artist",
    title,
    guid: Some(guid),
    ..Item::default()
  }
}

fn album(key: &'static str, parent: &'static str, title: &'static str, guid: &'static str, added: i64) -> Item {
  Item {
    key,
    kind: "album",
    title,
    guid: Some(guid),
    parent: Some(parent),
    added_at: Some(STAMP + added),
    ..Item::default()
  }
}

fn track(
  key: &'static str,
  parent: &'static str,
  title: &'static str,
  index: u32,
  codec: &'static str,
  seconds: u32,
  rating_count: u64,
) -> Item {
  Item {
    key,
    kind: "track",
    title,
    parent: Some(parent),
    index: Some(index),
    duration: Some(seconds * 1000),
    codec: Some(codec),
    rating_count: Some(rating_count),
    ..Item::default()
  }
}

fn playlist(key: &'static str, title: &'static str, members: Vec<&'static str>) -> Item {
  Item {
    key,
    kind: "playlist",
    title,
    members,
    ..Item::default()
  }
}

impl Item {
  fn guid(self, guid: &'static str) -> Self {
    Item {
      guid: Some(guid),
      ..self
    }
  }

  fn sorted_as(self, title_sort: &'static str) -> Self {
    Item {
      title_sort: Some(title_sort),
      ..self
    }
  }

  fn viewed(self, at: i64, count: u32) -> Self {
    Item {
      viewed_at: Some(STAMP + at),
      view_count: Some(count),
      ..self
    }
  }

  fn rated(self, at: i64) -> Self {
    Item {
      rated_at: Some(STAMP + at),
      ..self
    }
  }

  fn similar(self, similar: Vec<&'static str>) -> Self {
    Item { similar, ..self }
  }

  fn with_lyrics(self, lyrics: Vec<(&'static str, &'static str)>) -> Self {
    Item { lyrics, ..self }
  }

  fn near(self, nearest: Vec<&'static str>) -> Self {
    Item { nearest, ..self }
  }

  fn thumb(&self) -> String {
    format!("/library/metadata/{}/thumb/{STAMP}", self.key)
  }
}

struct Server {
  machine: &'static str,
  name: &'static str,
  token: &'static str,
  owned: bool,
  section: &'static str,
  items: Vec<Item>,
  streams: HashMap<&'static str, &'static str>,
}

fn tower() -> Server {
  Server {
    machine: MACHINE_A,
    name: "Tower",
    token: TOKEN_A,
    owned: true,
    section: "1",
    items: vec![
      artist("11", "Pornophonique", "plex://artist/porno").similar(vec!["12"]),
      artist("12", "Maya Filipic", "local://12"),
      album("101", "11", "8-bit lagerfeuer", "plex://album/8bit", 100).viewed(100, 1),
      album("102", "11", "Second Album", "plex://album/second", 300),
      album("104", "11", "Second Album (FLAC)", "plex://album/second", 310),
      album("103", "12", "The Emona Sessions", "local://103", 500)
        .sorted_as("Emona Sessions")
        .viewed(200, 1),
      track("1001", "101", "Sad Robot", 1, "mp3", 212, 40)
        .guid("plex://track/robot")
        .with_lyrics(vec![("lrc", "/library/streams/9001")])
        .near(vec!["1004", "1001", "1005"]),
      track("1002", "101", "Space Invaders", 2, "flac", 198, 20)
        .with_lyrics(vec![("lrc", "/library/streams/9002"), ("txt", "/library/streams/9003")]),
      track("1003", "101", "I Want To Be A Machine", 3, "wmav2", 240, 10),
      track("1004", "102", "Game Over", 1, "mp3", 180, 30)
        .guid("plex://track/gameover")
        .rated(200_100),
      track("1005", "103", "Stories from Emona", 1, "mp3", 265, 5)
        .with_lyrics(vec![("lrc", "/library/streams/9004"), ("txt", "/library/streams/9005")]),
      playlist("501", "Road trip", vec!["1002", "1005"]),
    ],
    streams: HashMap::from([
      ("/library/streams/9001", SYNCED_LYRICS),
      ("/library/streams/9003", PLAIN_LYRICS),
      ("/library/streams/9004", UNTIMED_LRC),
      ("/library/streams/9005", EMONA_LYRICS),
    ]),
  }
}

fn attic() -> Server {
  Server {
    machine: MACHINE_B,
    name: "Attic",
    token: TOKEN_B,
    owned: true,
    section: "3",
    items: vec![
      artist("21", "Pornophonique", "plex://artist/porno").similar(vec!["22"]),
      artist("22", "Chipzel", "plex://artist/chipzel"),
      artist("23", "Maya Filipic", "local://23"),
      album("201", "21", "8-bit lagerfeuer", "plex://album/8bit", 50).viewed(500, 2),
      album("202", "21", "Live at the Lagerfeuer", "plex://album/live", 400).viewed(300, 1),
      album("203", "23", "The Emona Sessions", "local://203", 40).sorted_as("Emona Sessions"),
      album("204", "21", "Second Album", "plex://album/second", 30),
      album("205", "22", "Spectra", "plex://album/spectra", 20),
      track("2001", "201", "Sad Robot", 1, "mp3", 212, 1).guid("plex://track/robot"),
      track("2002", "202", "Sad Robot (Live)", 1, "mp3", 200, 10),
      track("2003", "202", "Sweet Ivy", 2, "flac", 190, 50).rated(200_200),
      track("2005", "202", "Lagerfeuer Outro", 3, "mp3", 150, 30),
      track("2008", "204", "Game Over", 1, "mp3", 180, 2)
        .guid("plex://track/gameover")
        .rated(50),
      track("2006", "205", "Focus", 1, "mp3", 180, 5).guid("plex://track/focus"),
      track("2007", "205", "Only Human", 2, "mp3", 200, 8).guid("plex://track/human"),
      playlist("601", "Night drive", vec!["2006"]),
    ],
    streams: HashMap::new(),
  }
}

fn friend() -> Server {
  Server {
    machine: MACHINE_C,
    name: "Friend",
    token: TOKEN_C,
    owned: false,
    section: "5",
    items: vec![
      artist("31", "Chipzel", "plex://artist/chipzel"),
      artist("32", "Pornophonique", "plex://artist/porno"),
      album("301", "31", "Spectra", "plex://album/spectra", 450),
      album("302", "32", "8-bit lagerfeuer", "plex://album/8bit", 60).viewed(250, 4),
      track("3001", "301", "Focus", 1, "mp3", 180, 5).guid("plex://track/focus"),
      track("3002", "301", "Only Human", 2, "mp3", 200, 8).guid("plex://track/human"),
      track("3003", "302", "Sad Robot", 1, "mp3", 212, 3).guid("plex://track/robot"),
      playlist("701", "Road trip", vec!["3001", "3002"]),
    ],
    streams: HashMap::new(),
  }
}

#[derive(Clone, Copy)]
enum Host {
  Tv,
  Pms(usize),
}

struct State {
  script: Script,
  seen: Vec<Seen>,
  polls: u32,
  stamp: i64,
  servers: Vec<Server>,
  urls: Vec<String>,
  tv_url: String,
  dead_url: String,
}

pub struct PlexStub {
  pub tv_url: String,
  pub dead_url: String,
  state: Arc<Mutex<State>>,
}

fn bind() -> (TcpListener, String) {
  let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port for the plex stub");
  let port = listener.local_addr().expect("a bound address").port();
  (listener, format!("http://127.0.0.1:{port}"))
}

impl PlexStub {
  pub fn serve() -> Self {
    Self::serve_with(Script::default())
  }

  pub fn serve_with(script: Script) -> Self {
    let (tv, tv_url) = bind();
    let (a, a_url) = bind();
    let (b, b_url) = bind();
    let (c, c_url) = bind();
    let dead_url = bind().1;
    let state = Arc::new(Mutex::new(State {
      script,
      seen: Vec::new(),
      polls: 0,
      stamp: STAMP + 900_000,
      servers: vec![tower(), attic(), friend()],
      urls: vec![a_url, b_url, c_url],
      tv_url: tv_url.clone(),
      dead_url: dead_url.clone(),
    }));
    for (listener, host) in [(tv, Host::Tv), (a, Host::Pms(0)), (b, Host::Pms(1)), (c, Host::Pms(2))] {
      let shared = Arc::clone(&state);
      thread::spawn(move || {
        for accepted in listener.incoming() {
          let Ok(socket) = accepted else { continue };
          let shared = Arc::clone(&shared);
          thread::spawn(move || answer(socket, host, &shared));
        }
      });
    }
    PlexStub {
      tv_url,
      dead_url,
      state,
    }
  }

  pub fn url(&self, machine: &str) -> String {
    let state = self.state.lock().unwrap();
    let at = state
      .servers
      .iter()
      .position(|server| server.machine == machine)
      .expect("a stub server");
    state.urls[at].clone()
  }

  pub fn seen(&self) -> Vec<Seen> {
    self.state.lock().unwrap().seen.clone()
  }

  pub fn on(&self, host: &str, path: &str) -> Vec<Seen> {
    self
      .seen()
      .into_iter()
      .filter(|seen| seen.host == host && seen.path == path)
      .collect()
  }

  pub fn polls(&self) -> u32 {
    self.state.lock().unwrap().polls
  }
}

enum Reply {
  Json(Value),
  Bytes(&'static str, Vec<u8>),
  Status(u16),
}

fn answer(mut socket: TcpStream, host: Host, state: &Arc<Mutex<State>>) {
  let mut head = Vec::new();
  let mut byte = [0u8; 1];
  while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
    match socket.read(&mut byte) {
      Ok(1) => head.push(byte[0]),
      _ => return,
    }
  }
  let text = String::from_utf8_lossy(&head);
  let mut lines = text.lines();
  let mut request = lines.next().unwrap_or_default().split(' ');
  let (Some(method), Some(target)) = (request.next(), request.next()) else {
    return;
  };
  let headers: HashMap<String, String> = lines
    .filter_map(|line| line.split_once(':'))
    .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
    .collect();
  let parsed = url::Url::parse(&format!("http://stub{target}")).expect("a parseable request target");
  let query: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
  let path = parsed.path().to_owned();
  let token = headers
    .get("x-plex-token")
    .or_else(|| query.get("X-Plex-Token"))
    .cloned();

  let mut state = state.lock().unwrap();
  let label = match host {
    Host::Tv => TV,
    Host::Pms(at) => state.servers[at].machine,
  };
  state.seen.push(Seen {
    host: label,
    method: method.to_owned(),
    path: path.clone(),
    query: query.clone(),
    token: token.clone(),
    accept: headers.get("accept").cloned(),
  });
  let reply = match host {
    Host::Tv => tv(&mut state, method, &path, token.as_deref()),
    Host::Pms(_)
      if path.starts_with("/library/streams/")
        && headers
          .get("accept")
          .is_some_and(|accept| accept.contains("application/json")) =>
    {
      Reply::Status(404)
    }
    Host::Pms(at) if token.as_deref() != Some(state.servers[at].token) => Reply::Status(401),
    Host::Pms(at) => {
      let State { servers, stamp, .. } = &mut *state;
      pms(&mut servers[at], stamp, method, &path, &query)
    }
  };
  drop(state);
  match reply {
    Reply::Json(body) => respond(&mut socket, 200, "application/json", body.to_string().as_bytes()),
    Reply::Bytes(content_type, body) => respond(&mut socket, 200, content_type, &body),
    Reply::Status(status) => respond(&mut socket, status, "text/plain", b""),
  }
}

fn tv(state: &mut State, method: &str, path: &str, token: Option<&str>) -> Reply {
  match (method, path) {
    ("POST", "/api/v2/pins") => {
      Reply::Json(json!({ "id": PIN_ID, "code": PIN_CODE, "expiresIn": 1800, "authToken": null }))
    }
    ("GET", "/api/v2/resources") if state.script.revoked || token != Some(ACCOUNT_TOKEN) => Reply::Status(401),
    ("GET", "/api/v2/resources") => {
      let connection = |uri: &str, local: bool, relay: bool| json!({ "uri": uri, "local": local, "relay": relay });
      let connections = [
        vec![
          connection(&state.tv_url, false, true),
          connection(&state.urls[0], true, false),
        ],
        vec![
          connection(&state.dead_url, true, false),
          connection(&state.urls[1], false, false),
        ],
        vec![connection(&state.urls[2], true, false)],
      ];
      let mut resources: Vec<Value> = state
        .servers
        .iter()
        .zip(connections)
        .map(|(server, connections)| {
          json!({
            "name": server.name, "product": "Plex Media Server", "clientIdentifier": server.machine,
            "provides": "server", "owned": server.owned, "accessToken": server.token, "connections": connections,
          })
        })
        .collect();
      resources.push(json!({
        "name": "Phone", "product": "Plexamp", "clientIdentifier": "player-1", "provides": "player,controller",
        "owned": true, "connections": [],
      }));
      Reply::Json(Value::Array(resources))
    }
    ("GET", "/identity") => Reply::Json(json!({ "MediaContainer": { "machineIdentifier": "relay" } })),
    ("GET", pin) if pin == format!("/api/v2/pins/{PIN_ID}") => {
      state.polls += 1;
      let token = (state.polls >= state.script.pin_after).then_some(ACCOUNT_TOKEN);
      Reply::Json(json!({ "id": PIN_ID, "code": PIN_CODE, "authToken": token }))
    }
    _ => Reply::Status(404),
  }
}

fn find<'a>(server: &'a Server, key: &str) -> Option<&'a Item> {
  server.items.iter().find(|item| item.key == key)
}

fn metadata(server: &Server, item: &Item, detailed: bool) -> Value {
  let mut value = json!({
    "ratingKey": item.key,
    "key": format!("/library/metadata/{}", item.key),
    "type": item.kind,
    "title": item.title,
    "librarySectionID": server.section.parse::<u32>().unwrap(),
  });
  let put = |value: &mut Value, name: &str, field: Value| {
    if !field.is_null() {
      value[name] = field;
    }
  };
  put(&mut value, "index", json!(item.index));
  put(&mut value, "duration", json!(item.duration));
  put(&mut value, "addedAt", json!(item.added_at));
  put(&mut value, "lastViewedAt", json!(item.viewed_at));
  put(&mut value, "viewCount", json!(item.view_count));
  put(&mut value, "guid", json!(item.guid));
  put(&mut value, "titleSort", json!(item.title_sort));
  put(&mut value, "ratingCount", json!(item.rating_count));
  if let Some(rated) = item.rated_at {
    value["userRating"] = json!(10.0);
    value["lastRatedAt"] = json!(rated);
  }
  let parent = item.parent.and_then(|key| find(server, key));
  match item.kind {
    "artist" => value["thumb"] = json!(item.thumb()),
    "album" => {
      value["thumb"] = json!(item.thumb());
      value["year"] = json!(2007);
      value["originallyAvailableAt"] = json!("2007-11-10");
      value["Format"] = json!([{ "tag": "Album" }]);
      if let Some(artist) = parent {
        value["parentRatingKey"] = json!(artist.key);
        value["parentTitle"] = json!(artist.title);
      }
    }
    "track" => {
      let album = parent.expect("every fixture track has an album");
      let artist = album.parent.and_then(|key| find(server, key)).expect("an album artist");
      value["parentRatingKey"] = json!(album.key);
      value["grandparentRatingKey"] = json!(artist.key);
      value["parentTitle"] = json!(album.title);
      value["grandparentTitle"] = json!(artist.title);
      value["thumb"] = json!(album.thumb());
      value["parentThumb"] = json!(album.thumb());
      let codec = item.codec.unwrap_or("mp3");
      let extension = match codec {
        "wmav2" => "wma",
        other => other,
      };
      let mut streams = vec![json!({ "streamType": 2, "codec": codec })];
      if detailed {
        streams.extend(
          item
            .lyrics
            .iter()
            .map(|(codec, key)| json!({ "streamType": 4, "codec": codec, "key": key })),
        );
      }
      value["Media"] = json!([{
        "audioCodec": codec,
        "Part": [{ "key": format!("/library/parts/{}/{STAMP}/file.{extension}", item.key), "Stream": streams }],
      }]);
    }
    "playlist" => {
      value["key"] = json!(format!("/playlists/{}/items", item.key));
      value["playlistType"] = json!("audio");
      value["leafCount"] = json!(item.members.len());
      value["composite"] = json!(format!("/playlists/{}/composite/{STAMP}", item.key));
    }
    _ => {}
  }
  value
}

fn container(server: &Server, items: &[&Item], query: &HashMap<String, String>) -> Value {
  let number = |name: &str| query.get(name).and_then(|value| value.parse::<usize>().ok());
  let start = number("X-Plex-Container-Start").unwrap_or(0);
  let size = number("X-Plex-Container-Size").unwrap_or(usize::MAX);
  let page: Vec<Value> = items
    .iter()
    .skip(start)
    .take(size)
    .map(|item| metadata(server, item, false))
    .collect();
  json!({ "MediaContainer": { "size": page.len(), "totalSize": items.len(), "Metadata": page } })
}

fn kind_of(code: Option<&String>) -> &'static str {
  match code.map(String::as_str) {
    Some("8") => "artist",
    Some("9") => "album",
    Some("10") => "track",
    _ => "",
  }
}

fn artist_of<'a>(server: &'a Server, item: &Item) -> Option<&'a str> {
  let album = find(server, item.parent?)?;
  album.parent
}

fn listed<'a>(server: &'a Server, query: &HashMap<String, String>) -> Vec<&'a Item> {
  let kind = kind_of(query.get("type"));
  let mut items: Vec<&Item> = server
    .items
    .iter()
    .filter(|item| item.kind == kind)
    .filter(|item| query.get("userRating").is_none_or(|_| item.rated_at.is_some()))
    .filter(|item| query.get("lastViewedAt>>").is_none_or(|_| item.viewed_at.is_some()))
    .filter(|item| {
      query
        .get("artist.id")
        .is_none_or(|artist| artist_of(server, item) == Some(artist.as_str()))
    })
    .collect();
  let sort = query.get("sort").map(String::as_str).unwrap_or("titleSort");
  let field = sort.trim_end_matches(":desc");
  match field {
    "titleSort" => items.sort_by_key(|item| item.title_sort.unwrap_or(item.title).to_lowercase()),
    "addedAt" | "lastViewedAt" | "lastRatedAt" | "ratingCount" | "viewCount" => items.sort_by_key(|item| {
      std::cmp::Reverse(match field {
        "addedAt" => item.added_at,
        "lastViewedAt" => item.viewed_at,
        "lastRatedAt" => item.rated_at,
        "ratingCount" => item.rating_count.map(|count| count as i64),
        "viewCount" => item.view_count.map(i64::from),
        _ => None,
      })
    }),
    _ => {}
  }
  items
}

fn pms(server: &mut Server, stamp: &mut i64, method: &str, path: &str, query: &HashMap<String, String>) -> Reply {
  let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
  match (method, segments.as_slice()) {
    ("GET", ["identity"]) => Reply::Json(json!({ "MediaContainer": { "machineIdentifier": server.machine } })),
    ("GET", ["library", "sections"]) => Reply::Json(json!({ "MediaContainer": { "Directory": [
      { "key": server.section, "type": "artist", "title": "Music" },
      { "key": "9", "type": "movie", "title": "Movies" },
    ] } })),
    ("GET", ["library", "sections", section, "all"]) if *section == server.section => {
      Reply::Json(container(server, &listed(server, query), query))
    }
    ("GET", ["library", "sections", section, "search"]) if *section == server.section => {
      let kind = kind_of(query.get("type"));
      let needle = query.get("query").cloned().unwrap_or_default().to_lowercase();
      let hits: Vec<&Item> = server
        .items
        .iter()
        .filter(|item| item.kind == kind && item.title.to_lowercase().contains(&needle))
        .collect();
      Reply::Json(container(server, &hits, query))
    }
    ("GET", ["library", "metadata", key, rest @ ..]) => {
      let Some(item) = find(server, key) else {
        return Reply::Status(404);
      };
      let related: Vec<&Item> = match rest {
        [] => {
          return Reply::Json(json!({ "MediaContainer": { "size": 1, "Metadata": [metadata(server, item, true)] } }));
        }
        ["children"] => {
          let mut children: Vec<&Item> = server.items.iter().filter(|child| child.parent == Some(key)).collect();
          children.sort_by_key(|child| (child.index, child.title));
          children
        }
        ["allLeaves"] => server
          .items
          .iter()
          .filter(|leaf| leaf.kind == "track" && artist_of(server, leaf) == Some(key))
          .collect(),
        ["similar"] => item.similar.iter().filter_map(|key| find(server, key)).collect(),
        ["nearest"] => item.nearest.iter().filter_map(|key| find(server, key)).collect(),
        _ => return Reply::Status(404),
      };
      Reply::Json(container(server, &related, query))
    }
    ("GET", ["playlists"]) if query.get("playlistType").map(String::as_str) == Some("audio") => {
      let lists: Vec<&Item> = server.items.iter().filter(|item| item.kind == "playlist").collect();
      Reply::Json(container(server, &lists, query))
    }
    ("GET", ["playlists", key, "items"]) => match find(server, key) {
      Some(list) => {
        let members: Vec<&Item> = list.members.iter().filter_map(|key| find(server, key)).collect();
        Reply::Json(container(server, &members, query))
      }
      None => Reply::Status(404),
    },
    ("PUT", [":", "rate"]) => {
      let key = query.get("key").cloned().unwrap_or_default();
      let Some(item) = server.items.iter_mut().find(|item| item.key == key) else {
        return Reply::Status(404);
      };
      *stamp += 1;
      item.rated_at = match query.get("rating").map(String::as_str) {
        Some("10") => Some(*stamp),
        _ => None,
      };
      Reply::Status(200)
    }
    ("GET", [":", "timeline"]) => Reply::Status(200),
    ("GET", ["library", "streams", _]) => match server.streams.get(path) {
      Some(text) => Reply::Bytes("text/plain", text.as_bytes().to_vec()),
      None => Reply::Status(404),
    },
    ("GET", ["photo", ":", "transcode"]) => Reply::Bytes(
      "image/jpeg",
      format!(
        "art:{}:{}",
        server.machine,
        query.get("url").cloned().unwrap_or_default()
      )
      .into_bytes(),
    ),
    ("GET", ["library", "parts", ..]) | ("GET", ["music", ":", "transcode", "universal", "start.mp3"]) => {
      Reply::Bytes("audio/mpeg", AUDIO_BYTES.to_vec())
    }
    _ => Reply::Status(404),
  }
}

fn respond(socket: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
  let reason = match status {
    200 => "OK",
    401 => "Unauthorized",
    _ => "Not Found",
  };
  let head = format!(
    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
    body.len()
  );
  let _ = socket.write_all(head.as_bytes());
  let _ = socket.write_all(body);
  let _ = socket.flush();
}

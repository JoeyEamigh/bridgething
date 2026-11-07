use bridgething_io::{HttpError, HttpExecutor, HttpHeader, HttpMethod, HttpRequest};
use serde::{Deserialize, de::DeserializeOwned};

pub const PLEX_TV: &str = "https://plex.tv";
const AUTH_APP: &str = "https://app.plex.tv/auth";
const PRODUCT: &str = "bridgething";
const PLATFORM: &str = "Generic";
const LIBRARY: &str = "com.plexapp.plugins.library";
const REQUEST_TIMEOUT_MS: u32 = 15_000;
const PROBE_TIMEOUT_MS: u32 = 4_000;
const ART_MASTER_EDGE: u32 = 600;
const TRANSCODE_BITRATE: &str = "320";
const TRANSCODE_PROFILE: &str =
  "add-transcode-target(type=musicProfile&context=streaming&protocol=http&container=mp3&audioCodec=mp3)";
const DIRECT_CODECS: [&str; 5] = ["mp3", "aac", "alac", "flac", "pcm"];
const LYRICS_STREAM: u32 = 4;
const JSON: &str = "application/json";
const TEXT: &str = "*/*";
const SLIM: [(&str, &str); 2] = [
  ("excludeFields", "summary"),
  (
    "excludeElements",
    "Genre,Style,Mood,Country,Similar,Field,Director,Label,Collection,Image,UltraBlurColors",
  ),
];
pub const LIKED_RATING: f32 = 10.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlexError {
  #[error("plex could not be reached: {0}")]
  Transport(String),
  #[error("plex answered http {0}")]
  Http(u16),
  #[error("plex refused the sign-in")]
  Unauthorized,
  #[error("plex sent something unexpected: {0}")]
  Malformed(String),
}

impl From<HttpError> for PlexError {
  fn from(error: HttpError) -> Self {
    PlexError::Transport(error.to_string())
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemType {
  Artist,
  Album,
  Track,
}

impl ItemType {
  fn code(self) -> &'static str {
    match self {
      ItemType::Artist => "8",
      ItemType::Album => "9",
      ItemType::Track => "10",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineState {
  Playing,
  Paused,
  Stopped,
}

impl TimelineState {
  fn wire(self) -> &'static str {
    match self {
      TimelineState::Playing => "playing",
      TimelineState::Paused => "paused",
      TimelineState::Stopped => "stopped",
    }
  }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
  pub id: u64,
  pub code: String,
  #[serde(default)]
  pub expires_in: Option<u64>,
  #[serde(default)]
  auth_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
  pub name: String,
  pub client_identifier: String,
  #[serde(default)]
  pub provides: String,
  #[serde(default)]
  pub access_token: Option<String>,
  #[serde(default)]
  pub owned: bool,
  #[serde(default)]
  pub connections: Vec<Connection>,
}

impl Resource {
  pub fn is_server(&self) -> bool {
    self.provides.split(',').any(|role| role == "server")
  }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
  pub uri: String,
  #[serde(default)]
  pub local: bool,
  #[serde(default)]
  pub relay: bool,
}

impl Connection {
  pub fn rank(&self) -> u8 {
    match (self.relay, self.local) {
      (false, true) => 0,
      (false, false) => 1,
      (true, _) => 2,
    }
  }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
  #[serde(default)]
  pub key: String,
  #[serde(default, rename = "type")]
  pub kind: String,
  #[serde(default)]
  pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Tag {
  #[serde(default)]
  pub tag: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
  #[serde(default)]
  pub stream_type: u32,
  #[serde(default)]
  pub codec: Option<String>,
  #[serde(default)]
  pub key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Part {
  #[serde(default)]
  pub key: String,
  #[serde(default, rename = "Stream")]
  pub streams: Vec<Stream>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
  #[serde(default)]
  pub audio_codec: Option<String>,
  #[serde(default, rename = "Part")]
  pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
  pub rating_key: String,
  #[serde(default, rename = "type")]
  pub kind: String,
  #[serde(default)]
  pub title: String,
  #[serde(default)]
  pub title_sort: Option<String>,
  #[serde(default)]
  pub guid: Option<String>,
  #[serde(default)]
  pub parent_rating_key: Option<String>,
  #[serde(default)]
  pub grandparent_rating_key: Option<String>,
  #[serde(default)]
  pub parent_title: Option<String>,
  #[serde(default)]
  pub grandparent_title: Option<String>,
  #[serde(default)]
  pub original_title: Option<String>,
  #[serde(default, rename = "librarySectionID")]
  pub library_section_id: Option<u32>,
  #[serde(default)]
  pub index: Option<u32>,
  #[serde(default)]
  pub year: Option<i32>,
  #[serde(default)]
  pub originally_available_at: Option<String>,
  #[serde(default)]
  pub thumb: Option<String>,
  #[serde(default)]
  pub parent_thumb: Option<String>,
  #[serde(default)]
  pub composite: Option<String>,
  #[serde(default)]
  pub duration: Option<u32>,
  #[serde(default)]
  pub user_rating: Option<f32>,
  #[serde(default)]
  pub last_rated_at: Option<i64>,
  #[serde(default)]
  pub last_viewed_at: Option<i64>,
  #[serde(default)]
  pub view_count: Option<u32>,
  #[serde(default)]
  pub added_at: Option<i64>,
  #[serde(default)]
  pub leaf_count: Option<u32>,
  #[serde(default)]
  pub rating_count: Option<u64>,
  #[serde(default, rename = "Media")]
  pub media: Vec<Media>,
  #[serde(default, rename = "Format")]
  pub format: Vec<Tag>,
  #[serde(default, rename = "Subformat")]
  pub subformat: Vec<Tag>,
}

impl Metadata {
  pub fn liked(&self) -> bool {
    self.user_rating.is_some_and(|rating| rating >= LIKED_RATING)
  }

  pub fn artwork(&self) -> Option<&str> {
    [&self.thumb, &self.composite, &self.parent_thumb]
      .into_iter()
      .find_map(|path| path.as_deref().filter(|path| !path.is_empty()))
  }

  pub fn artist_name(&self) -> Option<&str> {
    self
      .original_title
      .as_deref()
      .or(self.grandparent_title.as_deref())
      .filter(|name| !name.is_empty())
  }

  fn lyric_keys(&self) -> Vec<(&str, bool)> {
    let mut lyrics: Vec<(&str, bool)> = self
      .media
      .iter()
      .flat_map(|media| &media.parts)
      .flat_map(|part| &part.streams)
      .filter(|stream| stream.stream_type == LYRICS_STREAM)
      .filter_map(|stream| {
        let key = stream.key.as_deref()?;
        Some((key, stream.codec.as_deref() == Some("lrc")))
      })
      .collect();
    lyrics.sort_by_key(|(_, timed)| !timed);
    lyrics
  }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Container {
  #[serde(default)]
  total_size: Option<u32>,
  #[serde(default, rename = "Metadata")]
  metadata: Vec<Metadata>,
  #[serde(default, rename = "Directory")]
  directories: Vec<Directory>,
}

#[derive(Deserialize)]
struct Envelope {
  #[serde(default, rename = "MediaContainer")]
  container: Container,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
  pub items: Vec<Metadata>,
  pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyric {
  pub text: String,
  pub timed: bool,
}

fn headers(client_id: &str, token: Option<&str>, accept: &str) -> Vec<HttpHeader> {
  let mut headers = vec![
    HttpHeader {
      name: "Accept".into(),
      value: accept.into(),
    },
    HttpHeader {
      name: "X-Plex-Client-Identifier".into(),
      value: client_id.into(),
    },
    HttpHeader {
      name: "X-Plex-Product".into(),
      value: PRODUCT.into(),
    },
    HttpHeader {
      name: "X-Plex-Version".into(),
      value: env!("CARGO_PKG_VERSION").into(),
    },
    HttpHeader {
      name: "X-Plex-Platform".into(),
      value: PLATFORM.into(),
    },
    HttpHeader {
      name: "X-Plex-Device-Name".into(),
      value: PRODUCT.into(),
    },
  ];
  if let Some(token) = token {
    headers.push(HttpHeader {
      name: "X-Plex-Token".into(),
      value: token.into(),
    });
  }
  headers
}

fn query(params: &[(&str, &str)]) -> String {
  let mut query = url::form_urlencoded::Serializer::new(String::new());
  for (name, value) in params {
    query.append_pair(name, value);
  }
  query.finish()
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, PlexError> {
  serde_json::from_slice(body).map_err(|error| PlexError::Malformed(error.to_string()))
}

async fn send(
  http: &HttpExecutor,
  method: HttpMethod,
  url: String,
  client_id: &str,
  token: Option<&str>,
  accept: &str,
  timeout_ms: u32,
) -> Result<Vec<u8>, PlexError> {
  let response = http
    .execute(HttpRequest {
      method,
      url,
      headers: headers(client_id, token, accept),
      body: Vec::new(),
      timeout_ms,
    })
    .await?;
  match response.status {
    401 => Err(PlexError::Unauthorized),
    status if !response.ok() => Err(PlexError::Http(status)),
    _ => Ok(response.body),
  }
}

pub struct PlexAccount {
  base: String,
  client_id: String,
  http: HttpExecutor,
}

impl PlexAccount {
  pub fn new(base: &str, client_id: &str, http: HttpExecutor) -> Self {
    Self {
      base: base.trim_end_matches('/').to_owned(),
      client_id: client_id.to_owned(),
      http,
    }
  }

  pub fn client_id(&self) -> &str {
    &self.client_id
  }

  pub fn auth_url(&self, code: &str) -> String {
    format!(
      "{AUTH_APP}#?{}",
      query(&[
        ("clientID", &self.client_id),
        ("code", code),
        ("context[device][product]", PRODUCT),
      ])
    )
  }

  pub async fn create_pin(&self) -> Result<Pin, PlexError> {
    let body = send(
      &self.http,
      HttpMethod::Post,
      format!("{}/api/v2/pins?strong=true", self.base),
      &self.client_id,
      None,
      JSON,
      REQUEST_TIMEOUT_MS,
    )
    .await?;
    decode(&body)
  }

  pub async fn pin_token(&self, id: u64) -> Result<Option<String>, PlexError> {
    let body = send(
      &self.http,
      HttpMethod::Get,
      format!("{}/api/v2/pins/{id}", self.base),
      &self.client_id,
      None,
      JSON,
      REQUEST_TIMEOUT_MS,
    )
    .await?;
    let pin: Pin = decode(&body)?;
    Ok(pin.auth_token.filter(|token| !token.is_empty()))
  }

  pub async fn resources(&self, token: &str) -> Result<Vec<Resource>, PlexError> {
    let body = send(
      &self.http,
      HttpMethod::Get,
      format!("{}/api/v2/resources?includeHttps=1&includeRelay=1", self.base),
      &self.client_id,
      Some(token),
      JSON,
      REQUEST_TIMEOUT_MS,
    )
    .await?;
    decode(&body)
  }

  pub async fn reachable(&self, uri: &str, token: &str) -> bool {
    send(
      &self.http,
      HttpMethod::Get,
      format!("{}/identity", uri.trim_end_matches('/')),
      &self.client_id,
      Some(token),
      JSON,
      PROBE_TIMEOUT_MS,
    )
    .await
    .is_ok()
  }
}

pub struct PlexServer {
  pub machine: String,
  pub name: String,
  base: String,
  token: String,
  client_id: String,
  http: HttpExecutor,
}

impl PlexServer {
  pub fn new(machine: &str, name: &str, base: &str, token: &str, client_id: &str, http: HttpExecutor) -> Self {
    Self {
      machine: machine.to_owned(),
      name: name.to_owned(),
      base: base.trim_end_matches('/').to_owned(),
      token: token.to_owned(),
      client_id: client_id.to_owned(),
      http,
    }
  }

  pub fn base(&self) -> &str {
    &self.base
  }

  fn url(&self, path: &str, params: &[(&str, &str)]) -> String {
    match params.is_empty() {
      true => format!("{}{path}", self.base),
      false => format!("{}{path}?{}", self.base, query(params)),
    }
  }

  fn signed(&self, path: &str, params: &[(&str, &str)]) -> String {
    let mut all = params.to_vec();
    all.push(("X-Plex-Client-Identifier", &self.client_id));
    all.push(("X-Plex-Product", PRODUCT));
    all.push(("X-Plex-Token", &self.token));
    self.url(path, &all)
  }

  async fn call(&self, method: HttpMethod, path: &str, params: &[(&str, &str)]) -> Result<Vec<u8>, PlexError> {
    self.call_as(method, path, params, JSON).await
  }

  async fn call_as(
    &self,
    method: HttpMethod,
    path: &str,
    params: &[(&str, &str)],
    accept: &str,
  ) -> Result<Vec<u8>, PlexError> {
    send(
      &self.http,
      method,
      self.url(path, params),
      &self.client_id,
      Some(&self.token),
      accept,
      REQUEST_TIMEOUT_MS,
    )
    .await
  }

  async fn container(&self, path: &str, params: &[(&str, &str)]) -> Result<Container, PlexError> {
    let body = self.call(HttpMethod::Get, path, params).await?;
    Ok(decode::<Envelope>(&body)?.container)
  }

  async fn page(&self, path: &str, params: &[(&str, &str)], start: u32, size: u32) -> Result<Page, PlexError> {
    let (start, size) = (start.to_string(), size.to_string());
    let mut all = params.to_vec();
    all.push(("X-Plex-Container-Start", &start));
    all.push(("X-Plex-Container-Size", &size));
    let container = self.container(path, &all).await?;
    Ok(Page {
      total: container.total_size.unwrap_or(container.metadata.len() as u32),
      items: container.metadata,
    })
  }

  pub async fn music_sections(&self) -> Result<Vec<Directory>, PlexError> {
    Ok(
      self
        .container("/library/sections", &[])
        .await?
        .directories
        .into_iter()
        .filter(|directory| directory.kind == "artist")
        .collect(),
    )
  }

  pub async fn items(
    &self,
    section: &str,
    kind: ItemType,
    sort: &str,
    filters: &[(&str, &str)],
    start: u32,
    size: u32,
  ) -> Result<Page, PlexError> {
    let mut params = vec![("type", kind.code()), ("sort", sort)];
    params.extend_from_slice(filters);
    params.extend_from_slice(&SLIM);
    self
      .page(&format!("/library/sections/{section}/all"), &params, start, size)
      .await
  }

  pub async fn every(&self, section: &str, kind: ItemType) -> Result<Vec<Metadata>, PlexError> {
    let mut params = vec![("type", kind.code()), ("sort", "titleSort")];
    params.extend_from_slice(&SLIM);
    Ok(
      self
        .container(&format!("/library/sections/{section}/all"), &params)
        .await?
        .metadata,
    )
  }

  pub async fn search(
    &self,
    section: &str,
    kind: ItemType,
    text: &str,
    start: u32,
    size: u32,
  ) -> Result<Page, PlexError> {
    self
      .page(
        &format!("/library/sections/{section}/search"),
        &[("type", kind.code()), ("query", text)],
        start,
        size,
      )
      .await
  }

  pub async fn metadata(&self, key: &str) -> Result<Metadata, PlexError> {
    self
      .container(&format!("/library/metadata/{key}"), &[])
      .await?
      .metadata
      .into_iter()
      .next()
      .ok_or_else(|| PlexError::Malformed(format!("no metadata for {key}")))
  }

  pub async fn children(&self, key: &str) -> Result<Vec<Metadata>, PlexError> {
    Ok(
      self
        .container(&format!("/library/metadata/{key}/children"), &[])
        .await?
        .metadata,
    )
  }

  pub async fn all_tracks(&self, artist: &str) -> Result<Vec<Metadata>, PlexError> {
    Ok(
      self
        .container(&format!("/library/metadata/{artist}/allLeaves"), &[])
        .await?
        .metadata,
    )
  }

  pub async fn similar(&self, key: &str) -> Result<Vec<Metadata>, PlexError> {
    Ok(
      self
        .container(&format!("/library/metadata/{key}/similar"), &[])
        .await?
        .metadata,
    )
  }

  pub async fn nearest(&self, key: &str, limit: u32) -> Result<Vec<Metadata>, PlexError> {
    let limit = limit.to_string();
    Ok(
      self
        .container(&format!("/library/metadata/{key}/nearest"), &[("limit", &limit)])
        .await?
        .metadata,
    )
  }

  pub async fn playlists(&self) -> Result<Vec<Metadata>, PlexError> {
    Ok(
      self
        .container("/playlists", &[("playlistType", "audio")])
        .await?
        .metadata,
    )
  }

  pub async fn playlist_items(&self, key: &str, start: u32, size: u32) -> Result<Page, PlexError> {
    self.page(&format!("/playlists/{key}/items"), &[], start, size).await
  }

  pub async fn rate(&self, key: &str, liked: bool) -> Result<(), PlexError> {
    let rating = if liked { "10" } else { "-1" };
    self
      .call(
        HttpMethod::Put,
        "/:/rate",
        &[("key", key), ("identifier", LIBRARY), ("rating", rating)],
      )
      .await
      .map(|_| ())
  }

  pub async fn timeline(
    &self,
    key: &str,
    state: TimelineState,
    time_ms: u32,
    duration_ms: Option<u32>,
  ) -> Result<(), PlexError> {
    let path = format!("/library/metadata/{key}");
    let time = time_ms.to_string();
    let duration = duration_ms.map(|duration| duration.to_string());
    let mut params = vec![
      ("ratingKey", key),
      ("key", path.as_str()),
      ("identifier", LIBRARY),
      ("state", state.wire()),
      ("time", time.as_str()),
    ];
    if let Some(duration) = duration.as_deref() {
      params.push(("duration", duration));
    }
    self.call(HttpMethod::Get, "/:/timeline", &params).await.map(|_| ())
  }

  pub async fn lyrics(&self, key: &str) -> Result<Option<Lyric>, PlexError> {
    let track = self.metadata(key).await?;
    for (stream, timed) in track.lyric_keys() {
      match self.call_as(HttpMethod::Get, stream, &[], TEXT).await {
        Ok(body) => {
          let text = String::from_utf8_lossy(&body).trim().to_owned();
          let usable = match timed {
            true => !crate::lyrics::lrc::parse(&text).is_empty(),
            false => !text.is_empty(),
          };
          if usable {
            return Ok(Some(Lyric { text, timed }));
          }
        }
        Err(PlexError::Http(status)) => tracing::debug!(status, stream, "a listed plex lyric stream is gone"),
        Err(error) => return Err(error),
      }
    }
    Ok(None)
  }

  pub fn stream_url(&self, track: &Metadata) -> Option<String> {
    let media = track.media.first()?;
    let direct = media
      .audio_codec
      .as_deref()
      .is_some_and(|codec| DIRECT_CODECS.contains(&codec));
    match media.parts.first() {
      Some(part) if direct && !part.key.is_empty() => Some(self.signed(&part.key, &[])),
      _ => {
        let path = format!("/library/metadata/{}", track.rating_key);
        Some(self.signed(
          "/music/:/transcode/universal/start.mp3",
          &[
            ("path", &path),
            ("mediaIndex", "0"),
            ("partIndex", "0"),
            ("protocol", "http"),
            ("directPlay", "0"),
            ("directStream", "0"),
            ("musicBitrate", TRANSCODE_BITRATE),
            ("session", &self.client_id),
            ("X-Plex-Platform", PLATFORM),
            ("X-Plex-Client-Profile-Extra", TRANSCODE_PROFILE),
          ],
        ))
      }
    }
  }

  pub fn photo_url(&self, path: &str) -> String {
    let edge = ART_MASTER_EDGE.to_string();
    self.signed(
      "/photo/:/transcode",
      &[
        ("width", &edge),
        ("height", &edge),
        ("minSize", "1"),
        ("upscale", "0"),
        ("url", path),
      ],
    )
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::*;

  struct NoHttp;

  impl bridgething_io::HttpTransport for NoHttp {
    fn execute(&self, _request: HttpRequest, sink: Arc<bridgething_io::HttpSink>) {
      sink.fail("unused".into());
    }

    fn download(&self, _request: HttpRequest, sink: Arc<bridgething_io::HttpDownloadSink>) {
      sink.on_failed("unused".into());
    }
  }

  fn server() -> PlexServer {
    PlexServer::new(
      "m1",
      "Tower",
      "https://plex.example:32400/",
      "tok",
      "cid",
      HttpExecutor::new(Arc::new(NoHttp)),
    )
  }

  fn track(codec: &str) -> Metadata {
    Metadata {
      rating_key: "7".into(),
      kind: "track".into(),
      media: vec![Media {
        audio_codec: Some(codec.into()),
        parts: vec![Part {
          key: "/library/parts/3/1/file.flac".into(),
          streams: Vec::new(),
        }],
      }],
      ..Metadata::default()
    }
  }

  #[test]
  fn a_playable_codec_streams_the_part_directly_with_the_token_in_the_query() {
    let url = server().stream_url(&track("flac")).unwrap();
    assert_eq!(
      url,
      "https://plex.example:32400/library/parts/3/1/file.flac?X-Plex-Client-Identifier=cid&X-Plex-Product=bridgething&X-Plex-Token=tok"
    );
  }

  #[test]
  fn an_unplayable_codec_goes_through_the_mp3_transcoder() {
    let url = server().stream_url(&track("wmav2")).unwrap();
    assert!(
      url.starts_with(
        "https://plex.example:32400/music/:/transcode/universal/start.mp3?path=%2Flibrary%2Fmetadata%2F7&"
      )
    );
    assert!(url.contains("X-Plex-Platform=Generic"));
    assert!(url.contains("X-Plex-Client-Profile-Extra=add-transcode-target"));
    assert!(url.ends_with("X-Plex-Token=tok"));
  }

  #[test]
  fn the_auth_url_carries_the_client_and_the_pin_code() {
    let account = PlexAccount::new(PLEX_TV, "cid", HttpExecutor::new(Arc::new(NoHttp)));
    assert_eq!(
      account.auth_url("abc"),
      "https://app.plex.tv/auth#?clientID=cid&code=abc&context%5Bdevice%5D%5Bproduct%5D=bridgething"
    );
  }

  #[test]
  fn timed_lyrics_win_over_plain_text() {
    let mut track = track("mp3");
    track.media[0].parts[0].streams = vec![
      Stream {
        stream_type: 2,
        codec: Some("mp3".into()),
        key: None,
      },
      Stream {
        stream_type: 4,
        codec: Some("txt".into()),
        key: Some("/library/streams/2".into()),
      },
      Stream {
        stream_type: 4,
        codec: Some("lrc".into()),
        key: Some("/library/streams/1".into()),
      },
    ];
    assert_eq!(
      track.lyric_keys(),
      [("/library/streams/1", true), ("/library/streams/2", false)]
    );
  }

  #[test]
  fn connections_rank_local_then_remote_then_relay() {
    let connection = |local, relay| Connection {
      uri: String::new(),
      local,
      relay,
    };
    assert!(connection(true, false).rank() < connection(false, false).rank());
    assert!(connection(false, false).rank() < connection(false, true).rank());
  }
}

use bridgething_io::{HttpError, HttpExecutor, HttpMethod, HttpRequest};
use md5::{Digest, Md5};
use serde::{Deserialize, de::DeserializeOwned};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "bridgething";
const REQUEST_TIMEOUT_MS: u32 = 15_000;
const COVER_MASTER_EDGE: u32 = 600;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubsonicError {
  #[error("the server could not be reached: {0}")]
  Transport(String),
  #[error("the server answered http {0}")]
  Http(u16),
  #[error("the server sent something that is not a subsonic reply: {0}")]
  Malformed(String),
  #[error("{message}")]
  Server { code: u32, message: String },
}

impl From<HttpError> for SubsonicError {
  fn from(error: HttpError) -> Self {
    SubsonicError::Transport(error.to_string())
  }
}

pub fn normalize_server_url(raw: &str) -> Result<String, String> {
  let trimmed = raw.trim();
  if trimmed.is_empty() {
    return Err("enter the server address".into());
  }
  let with_scheme = if trimmed.contains("://") {
    trimmed.to_owned()
  } else {
    format!("https://{trimmed}")
  };
  let parsed = url::Url::parse(&with_scheme).map_err(|_| "the server address is not a url".to_string())?;
  if !matches!(parsed.scheme(), "http" | "https") {
    return Err("the server address must start with http:// or https://".into());
  }
  if parsed.host_str().is_none() {
    return Err("the server address needs a host".into());
  }
  Ok(with_scheme.trim_end_matches('/').to_owned())
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Song {
  pub id: String,
  #[serde(default)]
  pub title: String,
  #[serde(default)]
  pub album: Option<String>,
  #[serde(default)]
  pub album_id: Option<String>,
  #[serde(default)]
  pub artist: Option<String>,
  #[serde(default)]
  pub artist_id: Option<String>,
  #[serde(default)]
  pub track: Option<u32>,
  #[serde(default)]
  pub duration: Option<u32>,
  #[serde(default)]
  pub cover_art: Option<String>,
  #[serde(default)]
  pub starred: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Album {
  pub id: String,
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub artist: Option<String>,
  #[serde(default)]
  pub artist_id: Option<String>,
  #[serde(default)]
  pub cover_art: Option<String>,
  #[serde(default)]
  pub song_count: Option<u32>,
  #[serde(default)]
  pub starred: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetail {
  #[serde(flatten)]
  pub album: Album,
  #[serde(default, rename = "song")]
  pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
  pub id: String,
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub cover_art: Option<String>,
  #[serde(default)]
  pub album_count: Option<u32>,
  #[serde(default)]
  pub starred: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDetail {
  #[serde(flatten)]
  pub artist: Artist,
  #[serde(default, rename = "album")]
  pub albums: Vec<Album>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
  pub id: String,
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub owner: Option<String>,
  #[serde(default)]
  pub song_count: Option<u32>,
  #[serde(default)]
  pub cover_art: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDetail {
  #[serde(flatten)]
  pub playlist: Playlist,
  #[serde(default, rename = "entry")]
  pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hits {
  #[serde(default, rename = "artist")]
  pub artists: Vec<Artist>,
  #[serde(default, rename = "album")]
  pub albums: Vec<Album>,
  #[serde(default, rename = "song")]
  pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
  #[serde(default)]
  pub start: Option<u32>,
  #[serde(default)]
  pub value: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StructuredLyrics {
  #[serde(default)]
  pub synced: bool,
  #[serde(default, rename = "line")]
  pub lines: Vec<LyricLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumList {
  Alphabetical,
  RecentlyPlayed,
  Newest,
}

impl AlbumList {
  fn wire(self) -> &'static str {
    match self {
      AlbumList::Alphabetical => "alphabeticalByName",
      AlbumList::RecentlyPlayed => "recent",
      AlbumList::Newest => "newest",
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchCounts {
  pub artists: u32,
  pub albums: u32,
  pub songs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarTarget<'a> {
  Song(&'a str),
  Album(&'a str),
  Artist(&'a str),
}

#[derive(Deserialize)]
struct Indexes {
  #[serde(default, rename = "index")]
  indexes: Vec<Index>,
}

#[derive(Deserialize)]
struct Index {
  #[serde(default, rename = "artist")]
  artists: Vec<Artist>,
}

#[derive(Deserialize)]
struct AlbumListPayload {
  #[serde(default, rename = "album")]
  albums: Vec<Album>,
}

#[derive(Deserialize)]
struct PlaylistsPayload {
  #[serde(default, rename = "playlist")]
  playlists: Vec<Playlist>,
}

#[derive(Deserialize)]
struct SongsPayload {
  #[serde(default, rename = "song")]
  songs: Vec<Song>,
}

#[derive(Deserialize)]
struct LyricsPayload {
  #[serde(default)]
  value: Option<String>,
}

#[derive(Deserialize)]
struct LyricsListPayload {
  #[serde(default, rename = "structuredLyrics")]
  structured: Vec<StructuredLyrics>,
}

#[derive(Deserialize)]
struct ServerError {
  #[serde(default)]
  code: u32,
  #[serde(default)]
  message: String,
}

pub struct SubsonicClient {
  base: String,
  username: String,
  password: String,
  salt: String,
  http: HttpExecutor,
}

impl SubsonicClient {
  pub fn new(server_url: &str, username: &str, password: &str, http: HttpExecutor) -> Self {
    Self {
      base: server_url.trim_end_matches('/').to_owned(),
      username: username.to_owned(),
      password: password.to_owned(),
      salt: uuid::Uuid::now_v7().simple().to_string(),
      http,
    }
  }

  pub fn base(&self) -> &str {
    &self.base
  }

  fn token(&self) -> String {
    let digest = Md5::digest(format!("{}{}", self.password, self.salt).as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
  }

  fn url(&self, endpoint: &str, params: &[(&str, &str)]) -> String {
    let token = self.token();
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
      .append_pair("u", &self.username)
      .append_pair("t", &token)
      .append_pair("s", &self.salt)
      .append_pair("v", API_VERSION)
      .append_pair("c", CLIENT_NAME)
      .append_pair("f", "json");
    for (name, value) in params {
      query.append_pair(name, value);
    }
    format!("{}/rest/{endpoint}?{}", self.base, query.finish())
  }

  pub fn stream_url(&self, id: &str) -> String {
    self.url("stream", &[("id", id)])
  }

  pub fn cover_art_url(&self, id: &str) -> String {
    let size = COVER_MASTER_EDGE.to_string();
    self.url("getCoverArt", &[("id", id), ("size", &size)])
  }

  async fn call<T: DeserializeOwned>(
    &self,
    endpoint: &str,
    params: &[(&str, &str)],
    payload: &str,
  ) -> Result<Option<T>, SubsonicError> {
    let response = self
      .http
      .execute(HttpRequest {
        method: HttpMethod::Get,
        url: self.url(endpoint, params),
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms: REQUEST_TIMEOUT_MS,
      })
      .await?;
    if !response.ok() {
      return Err(SubsonicError::Http(response.status));
    }
    let body: serde_json::Value =
      serde_json::from_slice(&response.body).map_err(|error| SubsonicError::Malformed(error.to_string()))?;
    let reply = body
      .get("subsonic-response")
      .ok_or_else(|| SubsonicError::Malformed("no subsonic-response envelope".into()))?;
    if reply.get("status").and_then(|status| status.as_str()) != Some("ok") {
      let error: ServerError = reply
        .get("error")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| SubsonicError::Malformed(error.to_string()))?
        .unwrap_or(ServerError {
          code: 0,
          message: "the server refused the request".into(),
        });
      return Err(SubsonicError::Server {
        code: error.code,
        message: error.message,
      });
    }
    match reply.get(payload) {
      Some(value) => serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|error| SubsonicError::Malformed(format!("{payload}: {error}"))),
      None => Ok(None),
    }
  }

  async fn expect<T: DeserializeOwned>(
    &self,
    endpoint: &str,
    params: &[(&str, &str)],
    payload: &str,
  ) -> Result<T, SubsonicError> {
    self
      .call(endpoint, params, payload)
      .await?
      .ok_or_else(|| SubsonicError::Malformed(format!("the reply carried no {payload}")))
  }

  pub async fn ping(&self) -> Result<(), SubsonicError> {
    self.call::<serde_json::Value>("ping", &[], "ping").await.map(|_| ())
  }

  pub async fn artists(&self) -> Result<Vec<Artist>, SubsonicError> {
    let indexes: Indexes = self.expect("getArtists", &[], "artists").await?;
    Ok(indexes.indexes.into_iter().flat_map(|index| index.artists).collect())
  }

  pub async fn artist(&self, id: &str) -> Result<ArtistDetail, SubsonicError> {
    self.expect("getArtist", &[("id", id)], "artist").await
  }

  pub async fn album(&self, id: &str) -> Result<AlbumDetail, SubsonicError> {
    self.expect("getAlbum", &[("id", id)], "album").await
  }

  pub async fn album_list(&self, list: AlbumList, size: u32, offset: u32) -> Result<Vec<Album>, SubsonicError> {
    let size = size.to_string();
    let offset = offset.to_string();
    let payload: AlbumListPayload = self
      .expect(
        "getAlbumList2",
        &[("type", list.wire()), ("size", &size), ("offset", &offset)],
        "albumList2",
      )
      .await?;
    Ok(payload.albums)
  }

  pub async fn playlists(&self) -> Result<Vec<Playlist>, SubsonicError> {
    let payload: PlaylistsPayload = self.expect("getPlaylists", &[], "playlists").await?;
    Ok(payload.playlists)
  }

  pub async fn playlist(&self, id: &str) -> Result<PlaylistDetail, SubsonicError> {
    self.expect("getPlaylist", &[("id", id)], "playlist").await
  }

  pub async fn song(&self, id: &str) -> Result<Song, SubsonicError> {
    self.expect("getSong", &[("id", id)], "song").await
  }

  pub async fn random_songs(&self, size: u32) -> Result<Vec<Song>, SubsonicError> {
    let size = size.to_string();
    let payload: SongsPayload = self.expect("getRandomSongs", &[("size", &size)], "randomSongs").await?;
    Ok(payload.songs)
  }

  pub async fn search(&self, query: &str, counts: SearchCounts, offset: u32) -> Result<Hits, SubsonicError> {
    let offset = offset.to_string();
    let artists = counts.artists.to_string();
    let albums = counts.albums.to_string();
    let songs = counts.songs.to_string();
    let hits: Option<Hits> = self
      .call(
        "search3",
        &[
          ("query", query),
          ("artistCount", &artists),
          ("artistOffset", &offset),
          ("albumCount", &albums),
          ("albumOffset", &offset),
          ("songCount", &songs),
          ("songOffset", &offset),
        ],
        "searchResult3",
      )
      .await?;
    Ok(hits.unwrap_or_default())
  }

  pub async fn starred(&self) -> Result<Hits, SubsonicError> {
    let hits: Option<Hits> = self.call("getStarred2", &[], "starred2").await?;
    Ok(hits.unwrap_or_default())
  }

  pub async fn set_starred(&self, target: StarTarget<'_>, starred: bool) -> Result<(), SubsonicError> {
    let endpoint = if starred { "star" } else { "unstar" };
    let params = match target {
      StarTarget::Song(id) => [("id", id)],
      StarTarget::Album(id) => [("albumId", id)],
      StarTarget::Artist(id) => [("artistId", id)],
    };
    self
      .call::<serde_json::Value>(endpoint, &params, endpoint)
      .await
      .map(|_| ())
  }

  pub async fn lyrics(&self, artist: &str, title: &str) -> Result<Option<String>, SubsonicError> {
    let payload: Option<LyricsPayload> = self
      .call("getLyrics", &[("artist", artist), ("title", title)], "lyrics")
      .await?;
    Ok(
      payload
        .and_then(|lyrics| lyrics.value)
        .filter(|text| !text.trim().is_empty()),
    )
  }

  pub async fn lyrics_by_song(&self, id: &str) -> Result<Option<StructuredLyrics>, SubsonicError> {
    let payload: Option<LyricsListPayload> = self.call("getLyricsBySongId", &[("id", id)], "lyricsList").await?;
    Ok(payload.and_then(|list| {
      list
        .structured
        .iter()
        .find(|lyrics| lyrics.synced)
        .or_else(|| list.structured.first())
        .cloned()
        .filter(|lyrics| !lyrics.lines.is_empty())
    }))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_bare_host_becomes_https_and_loses_its_trailing_slash() {
    assert_eq!(
      normalize_server_url(" music.example.com/ ").unwrap(),
      "https://music.example.com"
    );
    assert_eq!(
      normalize_server_url("http://10.0.0.5:4533/navidrome/").unwrap(),
      "http://10.0.0.5:4533/navidrome"
    );
  }

  #[test]
  fn junk_addresses_are_refused_with_a_reason() {
    assert!(normalize_server_url("").is_err());
    assert!(normalize_server_url("ftp://music.example.com").is_err());
    assert!(normalize_server_url("https://").is_err());
  }

  #[test]
  fn the_token_is_the_salted_md5_of_the_password() {
    let client = SubsonicClient::new(
      "https://music.example.com",
      "demo",
      "demo",
      HttpExecutor::new(std::sync::Arc::new(NoHttp)),
    );
    let expected: String = Md5::digest(format!("demo{}", client.salt).as_bytes())
      .iter()
      .map(|byte| format!("{byte:02x}"))
      .collect();
    assert_eq!(client.token(), expected);
    let url = client.stream_url("tr 1");
    assert!(url.starts_with("https://music.example.com/rest/stream?u=demo&t="));
    assert!(url.ends_with("&v=1.16.1&c=bridgething&f=json&id=tr+1"));
  }

  struct NoHttp;

  impl bridgething_io::HttpTransport for NoHttp {
    fn execute(&self, _request: HttpRequest, sink: std::sync::Arc<bridgething_io::HttpSink>) {
      sink.fail("unused".into());
    }

    fn download(&self, _request: HttpRequest, sink: std::sync::Arc<bridgething_io::HttpDownloadSink>) {
      sink.on_failed("unused".into());
    }
  }
}

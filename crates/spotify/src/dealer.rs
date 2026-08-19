use std::{
  io::Read,
  sync::{Arc, Mutex},
  time::Duration,
};

use ::http::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use base64::Engine;
use bridgething_io::{HttpMethod, WsConnect, WsEvent, WsFrame, WsInbox, WsTransport};
use librespot_protocol::{
  connect::{
    Capabilities, Cluster, ClusterUpdate, ConnectLoggingParams, Device, DeviceInfo, MemberType, PutStateReason,
    PutStateRequest, SetVolumeCommand,
  },
  devices::DeviceType,
  player::ProvidedTrack,
};
use protobuf::{Message, MessageField};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::{
  error::{Error, Result},
  http::{ANDROID_CLIENT_ID, SPCLIENT, SpHttp, random_hex},
  httpx::with_query,
  model::LibraryScope,
  util::now_ms,
};

const QUEUE_PROVIDER: &str = "queue";
const IS_QUEUED: &str = "is_queued";

#[cfg(feature = "native-io")]
fn default_transport() -> Arc<dyn WsTransport> {
  Arc::new(bridgething_io::TungsteniteTransport::new())
}

#[cfg(not(feature = "native-io"))]
fn default_transport() -> Arc<dyn WsTransport> {
  struct NoTransport;

  impl WsTransport for NoTransport {
    fn connect(&self, connect: WsConnect, inbox: Arc<WsInbox>) {
      inbox.on_closed(connect.id, None, "no websocket transport installed".to_string());
    }

    fn send(&self, _id: uuid::Uuid, _frame: WsFrame) {}

    fn disconnect(&self, _id: uuid::Uuid, _code: Option<u16>, _reason: Option<String>) {}
  }

  Arc::new(NoTransport)
}

#[derive(Clone)]
pub struct Dealer {
  http: SpHttp,
  device_id: String,
  name: String,
  transport: Arc<Mutex<Arc<dyn WsTransport>>>,
}

impl Dealer {
  pub fn new(http: SpHttp, device_id: String) -> Self {
    Dealer {
      http,
      device_id,
      name: "bridgething".to_string(),
      transport: Arc::new(Mutex::new(default_transport())),
    }
  }

  pub fn set_transport(&self, transport: Arc<dyn WsTransport>) {
    *self.transport.lock().unwrap() = transport;
  }

  pub fn device_id(&self) -> &str {
    &self.device_id
  }

  async fn dealer_host(&self) -> Result<String> {
    let url = with_query(
      "https://apresolve.spotify.com/".to_string(),
      &[("type", "dealer".to_string())],
    )?;
    let resp = self
      .http
      .send(HttpMethod::Get, url, HeaderMap::new(), Vec::new(), 0)
      .await?;
    let v: Value = serde_json::from_slice(&resp.body)?;
    let host = v["dealer"][0]
      .as_str()
      .ok_or_else(|| Error::other("apresolve returned no dealer host"))?;
    Ok(host.split(':').next().unwrap_or(host).to_string())
  }

  pub async fn open(&self) -> Result<(DealerStream, DealerWriter)> {
    let host = self.dealer_host().await?;
    tracing::debug!(%host, "dealer: opening websocket");
    let bearer = self.http.auth.bearer().await?;
    let url = format!("wss://{host}/?access_token={bearer}");
    let transport = self.transport.lock().unwrap().clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsEvent>();
    let socket = uuid::Uuid::new_v4();
    transport.connect(
      WsConnect {
        id: socket,
        url,
        protocols: Vec::new(),
        headers: Vec::new(),
      },
      Arc::new(WsInbox::new(tx)),
    );
    let connection_id = loop {
      match rx.recv().await {
        Some(WsEvent::Frame {
          frame: WsFrame::Text(t),
          ..
        }) => {
          let v: Value = match serde_json::from_str(t.as_str()) {
            Ok(v) => v,
            Err(e) => {
              transport.disconnect(socket, None, None);
              return Err(e.into());
            }
          };
          if let Some(cid) = v["headers"]["Spotify-Connection-Id"].as_str() {
            tracing::debug!(connection_id = %cid, "dealer: websocket connected");
            break cid.to_string();
          }
        }
        Some(WsEvent::Frame { .. }) | Some(WsEvent::Open { .. }) => {}
        Some(WsEvent::Closed { reason, .. }) => {
          return Err(Error::other(format!("dealer closed before connection-id: {reason}")));
        }
        None => return Err(Error::other("dealer closed before connection-id")),
      }
    };
    let writer = DealerWriter {
      http: self.http.clone(),
      device_id: self.device_id.clone(),
      name: self.name.clone(),
      connection_id,
    };
    let mut ping = tokio::time::interval(DEALER_PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    Ok((
      DealerStream {
        rx,
        socket,
        transport,
        ping,
        awaiting_response: false,
      },
      writer,
    ))
  }
}

const DEALER_PING_INTERVAL: Duration = Duration::from_secs(20);

pub enum DealerEvent {
  Cluster(Cluster),
  LibraryChanged(LibraryScope),
}

pub struct DealerStream {
  rx: mpsc::UnboundedReceiver<WsEvent>,
  socket: uuid::Uuid,
  transport: Arc<dyn WsTransport>,
  ping: tokio::time::Interval,
  awaiting_response: bool,
}

impl Drop for DealerStream {
  fn drop(&mut self) {
    self.transport.disconnect(self.socket, None, None);
  }
}

impl DealerStream {
  pub async fn next_event(&mut self) -> Result<Option<DealerEvent>> {
    loop {
      let event = tokio::select! {
        e = self.rx.recv() => e,
        _ = self.ping.tick() => {
          if self.awaiting_response {
            tracing::warn!("dealer: ping unanswered within interval; treating link as dead");
            return Ok(None);
          }
          tracing::debug!("dealer: ping");
          self.transport.send(self.socket, WsFrame::Text(r#"{"type":"ping"}"#.to_string()));
          self.awaiting_response = true;
          continue;
        }
      };
      let Some(event) = event else { return Ok(None) };
      self.awaiting_response = false;
      let text = match event {
        WsEvent::Frame {
          frame: WsFrame::Text(t),
          ..
        } => t,
        WsEvent::Frame { .. } | WsEvent::Open { .. } => continue,
        WsEvent::Closed { .. } => return Ok(None),
      };
      let msg: Value = match serde_json::from_str(text.as_str()) {
        Ok(v) => v,
        Err(_) => continue,
      };
      let kind = msg["type"].as_str();
      tracing::trace!(?kind, frame = %text, "dealer: raw frame");
      match kind {
        Some("ping") => {
          self
            .transport
            .send(self.socket, WsFrame::Text(r#"{"type":"pong"}"#.to_string()));
        }
        Some("pong") => {}
        Some("request") => {
          if let Some(key) = msg["key"].as_str() {
            let reply = json!({"type": "reply", "key": key, "payload": {"success": true}});
            self.transport.send(self.socket, WsFrame::Text(reply.to_string()));
          }
        }
        Some("message") => {
          let uri = msg["uri"].as_str().unwrap_or("");
          if uri.starts_with("hm://collection/") {
            tracing::debug!("dealer: library changed (saved)");
            return Ok(Some(DealerEvent::LibraryChanged(LibraryScope::Saved)));
          }
          if uri.starts_with("hm://playlist/") {
            tracing::debug!("dealer: library changed (playlists)");
            return Ok(Some(DealerEvent::LibraryChanged(LibraryScope::Playlists)));
          }
          if !uri.contains("connect-state/v1/cluster") {
            tracing::trace!(%uri, "dealer: ignoring non-cluster message");
            continue;
          }
          let gz = msg["headers"]["Transfer-Encoding"].as_str() == Some("gzip");
          if let Some(payloads) = msg["payloads"].as_array() {
            for p in payloads {
              if let Some(s) = p.as_str() {
                let raw = match decode_payload(s, gz) {
                  Ok(r) => r,
                  Err(e) => {
                    tracing::warn!(?e, "dealer: skipping undecodable payload");
                    continue;
                  }
                };
                let upd = match ClusterUpdate::parse_from_bytes(&raw) {
                  Ok(u) => u,
                  Err(e) => {
                    tracing::warn!(?e, "dealer: skipping unparseable cluster update");
                    continue;
                  }
                };
                if let Some(cluster) = upd.cluster.into_option() {
                  let roster = cluster
                    .device
                    .iter()
                    .map(|(id, d)| format!("{id}={}", d.name))
                    .collect::<Vec<_>>()
                    .join(",");
                  tracing::debug!(
                    active_device = %cluster.active_device_id,
                    %roster,
                    playing = cluster.player_state.is_playing,
                    paused = cluster.player_state.is_paused,
                    track = %cluster.player_state.track.uri,
                    context = %cluster.player_state.context_uri,
                    "dealer: cluster update"
                  );
                  return Ok(Some(DealerEvent::Cluster(cluster)));
                }
              }
            }
          }
        }
        _ => {}
      }
    }
  }
}

#[derive(Clone)]
pub struct DealerWriter {
  http: SpHttp,
  device_id: String,
  name: String,
  connection_id: String,
}

impl DealerWriter {
  #[cfg(test)]
  pub(crate) fn for_test(http: SpHttp, device_id: &str) -> Self {
    DealerWriter {
      http,
      device_id: device_id.to_string(),
      name: "bridgething".to_string(),
      connection_id: "test-connection".to_string(),
    }
  }

  pub fn connection_id(&self) -> &str {
    &self.connection_id
  }

  fn observer_put_state(&self) -> PutStateRequest {
    let mut caps = Capabilities::new();
    caps.can_be_player = false;
    caps.is_observable = true;
    caps.hidden = true;
    caps.needs_full_player_state = true;
    caps.volume_steps = 0;
    caps.supported_types.push("audio/track".to_string());

    let mut di = DeviceInfo::new();
    di.name = self.name.clone();
    di.device_id = self.device_id.clone();
    di.device_type = DeviceType::OBSERVER.into();
    di.client_id = ANDROID_CLIENT_ID.to_string();
    di.device_software_version = "bridgething-sfp/0.1".to_string();
    di.capabilities = MessageField::some(caps);

    let mut device = Device::new();
    device.device_info = MessageField::some(di);

    let mut req = PutStateRequest::new();
    req.device = MessageField::some(device);
    req.member_type = MemberType::CONNECT_STATE.into();
    req.is_active = false;
    req.put_state_reason = PutStateReason::NEW_DEVICE.into();
    req.client_side_timestamp = now_ms();
    req
  }

  pub async fn cluster(&self) -> Result<Cluster> {
    let body = self.observer_put_state().write_to_bytes()?;
    let mut headers = self.http.headers(false).await?;
    headers.insert(
      "X-Spotify-Connection-Id",
      HeaderValue::from_str(&self.connection_id).map_err(Error::other)?,
    );
    let url = format!("{SPCLIENT}/connect-state/v1/devices/{}", self.device_id);
    let resp = self.http.send(HttpMethod::Put, url, headers, body, 0).await?;
    if !resp.ok() {
      return Err(Error::status("get_cluster", resp.status, resp.text()));
    }
    Ok(Cluster::parse_from_bytes(&resp.body)?)
  }

  async fn player_command(&self, target: &str, command: Value) -> Result<(u16, String)> {
    let url = format!(
      "{SPCLIENT}/connect-state/v1/player/command/from/{}/to/{}",
      self.device_id, target
    );
    let mut headers = self.http.headers(true).await?;
    headers.insert(
      "X-Spotify-Connection-Id",
      HeaderValue::from_str(&self.connection_id).map_err(Error::other)?,
    );
    let endpoint = command["endpoint"].as_str().unwrap_or("?");
    tracing::debug!(%target, %endpoint, command = %command, "dealer: player command");
    let body = serde_json::to_vec(&json!({ "command": command }))?;
    let resp = self.http.send(HttpMethod::Post, url, headers, body, 0).await?;
    if !resp.ok() {
      return Err(Error::status("player_command", resp.status, resp.text()));
    }
    tracing::trace!(%endpoint, status = resp.status, "dealer: player command ok");
    Ok((resp.status, resp.text()))
  }

  pub async fn pause(&self, target: &str) -> Result<(u16, String)> {
    self.player_command(target, json!({"endpoint": "pause"})).await
  }
  pub async fn resume(&self, target: &str) -> Result<(u16, String)> {
    self.player_command(target, json!({"endpoint": "resume"})).await
  }
  pub async fn skip_next(&self, target: &str) -> Result<(u16, String)> {
    self.player_command(target, json!({"endpoint": "skip_next"})).await
  }
  pub async fn skip_prev(&self, target: &str) -> Result<(u16, String)> {
    self.player_command(target, json!({"endpoint": "skip_prev"})).await
  }
  pub async fn seek_to(&self, target: &str, ms: i64) -> Result<(u16, String)> {
    self
      .player_command(target, json!({"endpoint": "seek_to", "value": ms}))
      .await
  }
  pub async fn set_shuffle(&self, target: &str, on: bool) -> Result<(u16, String)> {
    self
      .player_command(target, json!({"endpoint": "set_shuffling_context", "value": on}))
      .await
  }
  pub async fn set_repeat_context(&self, target: &str, on: bool) -> Result<(u16, String)> {
    self
      .player_command(target, json!({"endpoint": "set_repeating_context", "value": on}))
      .await
  }
  pub async fn set_repeat_track(&self, target: &str, on: bool) -> Result<(u16, String)> {
    self
      .player_command(target, json!({"endpoint": "set_repeating_track", "value": on}))
      .await
  }

  pub async fn play(&self, target: &str, command: Value) -> Result<(u16, String)> {
    self.player_command(target, command).await
  }

  pub async fn add_to_queue(&self, target: &str, uri: &str) -> Result<(u16, String)> {
    self
      .player_command(
        target,
        json!({"endpoint": "add_to_queue", "track": {"uri": uri, "provider": "queue"}}),
      )
      .await
  }

  pub async fn set_queue(
    &self,
    target: &str,
    next: &[ProvidedTrack],
    prev: &[ProvidedTrack],
    revision: &str,
  ) -> Result<(u16, String)> {
    let encode = |ts: &[ProvidedTrack]| ts.iter().map(provided_track_json).collect::<Vec<_>>();
    self
      .player_command(
        target,
        json!({
          "endpoint": "set_queue",
          "next_tracks": encode(next),
          "prev_tracks": encode(prev),
          "queue_revision": revision,
        }),
      )
      .await
  }

  pub async fn dj_signal(&self, target: &str) -> Result<(u16, String)> {
    self
      .player_command(target, json!({"endpoint": "signal", "signal_id": "jump"}))
      .await
  }

  pub async fn transfer(&self, target: &str) -> Result<(u16, String)> {
    let url = format!(
      "{SPCLIENT}/connect-state/v1/connect/transfer/from/{}/to/{}",
      self.device_id, target
    );
    let mut headers = self.http.headers(true).await?;
    headers.insert(
      "X-Spotify-Connection-Id",
      HeaderValue::from_str(&self.connection_id).map_err(Error::other)?,
    );
    let body = json!({
        "options": {"restore_paused": "restore", "restore_position": "extrapolate",
                    "restore_track": "only_current", "license": "premium"},
        "transfer_intent_id": random_hex(16),
        "command_id": random_hex(16),
        "interaction_id": random_hex(16),
    });
    let resp = self
      .http
      .send(HttpMethod::Post, url, headers, serde_json::to_vec(&body)?, 0)
      .await?;
    if !resp.ok() {
      return Err(Error::status("transfer", resp.status, resp.text()));
    }
    Ok((resp.status, resp.text()))
  }

  pub async fn set_volume(&self, target: &str, percent: f64) -> Result<(u16, i32)> {
    let raw = ((percent / 100.0 * 65535.0).round() as i32).clamp(0, 65535);
    let mut cmd = SetVolumeCommand::new();
    cmd.volume = raw;
    let mut lp = ConnectLoggingParams::new();
    lp.interaction_ids.push(random_hex(16));
    cmd.logging_params = MessageField::some(lp);
    cmd.connection_type = "wlan".to_string();
    let mut headers = self.http.headers(false).await?;
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/protobuf"));
    headers.insert(
      "X-Spotify-Connection-Id",
      HeaderValue::from_str(&self.connection_id).map_err(Error::other)?,
    );
    let url = format!(
      "{SPCLIENT}/connect-state/v1/connect/volume/from/{}/to/{}",
      self.device_id, target
    );
    let resp = self
      .http
      .send(HttpMethod::Put, url, headers, cmd.write_to_bytes()?, 0)
      .await?;
    if !resp.ok() {
      return Err(Error::status("set_volume", resp.status, resp.text()));
    }
    Ok((resp.status, raw))
  }
}

fn decode_payload(p: &str, gzipped: bool) -> Result<Vec<u8>> {
  let bytes = base64::engine::general_purpose::STANDARD
    .decode(p)
    .map_err(Error::other)?;
  if !gzipped {
    return Ok(bytes);
  }
  let mut out = Vec::new();
  flate2::read::GzDecoder::new(&bytes[..])
    .read_to_end(&mut out)
    .map_err(Error::other)?;
  Ok(out)
}

pub fn cluster_playing(cluster: &Cluster) -> bool {
  cluster.player_state.is_playing && !cluster.player_state.is_paused
}

pub fn queued_track(uri: &str) -> ProvidedTrack {
  let mut track = ProvidedTrack::new();
  track.uri = uri.to_string();
  track.provider = QUEUE_PROVIDER.to_string();
  track.metadata.insert(IS_QUEUED.to_string(), "true".to_string());
  track
}

pub fn is_queued(track: &ProvidedTrack) -> bool {
  track.provider == QUEUE_PROVIDER || track.metadata.get(IS_QUEUED).is_some_and(|v| v == "true")
}

pub fn provided_track_json(track: &ProvidedTrack) -> Value {
  let mut out = serde_json::Map::new();
  let mut string_field = |key: &str, value: &str| {
    if !value.is_empty() {
      out.insert(key.to_string(), json!(value));
    }
  };
  string_field("uri", &track.uri);
  string_field("uid", &track.uid);
  string_field("provider", &track.provider);
  string_field("album_uri", &track.album_uri);
  string_field("artist_uri", &track.artist_uri);
  if !track.metadata.is_empty() {
    out.insert("metadata".to_string(), json!(track.metadata));
  }
  for (key, list) in [
    ("removed", &track.removed),
    ("blocked", &track.blocked),
    ("disallow_reasons", &track.disallow_reasons),
  ] {
    if !list.is_empty() {
      out.insert(key.to_string(), json!(list));
    }
  }
  Value::Object(out)
}

pub fn provided_track_from_json(value: &Value) -> ProvidedTrack {
  let string_field = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
  let list_field = |key: &str| {
    value
      .get(key)
      .and_then(Value::as_array)
      .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
      .unwrap_or_default()
  };
  let mut track = ProvidedTrack::new();
  track.uri = string_field("uri");
  track.uid = string_field("uid");
  track.provider = string_field("provider");
  track.album_uri = string_field("album_uri");
  track.artist_uri = string_field("artist_uri");
  track.removed = list_field("removed");
  track.blocked = list_field("blocked");
  track.disallow_reasons = list_field("disallow_reasons");
  if let Some(md) = value.get("metadata").and_then(Value::as_object) {
    for (k, v) in md {
      if let Some(v) = v.as_str() {
        track.metadata.insert(k.clone(), v.to_string());
      }
    }
  }
  track
}

fn is_phone(info: &DeviceInfo) -> bool {
  matches!(
    info.device_type.enum_value_or_default(),
    DeviceType::SMARTPHONE | DeviceType::TABLET
  )
}

pub fn phone_device(cluster: &Cluster, me: &str) -> Option<String> {
  if !cluster.active_device_id.is_empty()
    && cluster.active_device_id != me
    && cluster.device.get(&cluster.active_device_id).is_some_and(is_phone)
  {
    return Some(cluster.active_device_id.clone());
  }
  let mut ids: Vec<&String> = cluster
    .device
    .iter()
    .filter(|(id, info)| id.as_str() != me && is_phone(info))
    .map(|(id, _)| id)
    .collect();
  ids.sort();
  ids.first().map(|s| s.to_string())
}

pub fn active_device(cluster: &Cluster, me: &str, last_active: Option<&str>) -> Option<String> {
  if !cluster.active_device_id.is_empty() {
    return Some(cluster.active_device_id.clone());
  }
  if let Some(la) = last_active
    && la != me
    && cluster.device.contains_key(la)
  {
    return Some(la.to_string());
  }
  None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
  #[default]
  Car,
  Desk,
}

pub fn start_device(cluster: &Cluster, me: &str, last_active: Option<&str>, placement: Placement) -> Option<String> {
  match placement {
    Placement::Car => phone_device(cluster, me),
    Placement::Desk => active_device(cluster, me, last_active).or_else(|| phone_device(cluster, me)),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn device_info(kind: DeviceType) -> DeviceInfo {
    let mut di = DeviceInfo::new();
    di.device_type = kind.into();
    di
  }

  fn cluster(active: &str, playing: bool, devices: &[(&str, DeviceType)]) -> Cluster {
    let mut c = Cluster::new();
    c.active_device_id = active.to_string();
    let ps = c.player_state.mut_or_insert_default();
    ps.is_playing = playing;
    ps.is_paused = !playing;
    for (id, kind) in devices {
      c.device.insert(id.to_string(), device_info(*kind));
    }
    c
  }

  #[test]
  fn active_device_honors_the_cluster_active_id_unconditionally() {
    let c = cluster(
      "avr-1",
      false,
      &[("avr-1", DeviceType::AUDIO_DONGLE), ("phone-1", DeviceType::SMARTPHONE)],
    );
    assert_eq!(active_device(&c, "me", None), Some("avr-1".to_string()));
  }

  #[test]
  fn active_device_falls_back_to_last_active_still_in_the_cluster() {
    let c = cluster("", false, &[("avr-1", DeviceType::AUDIO_DONGLE)]);
    assert_eq!(active_device(&c, "me", Some("avr-1")), Some("avr-1".to_string()));
    assert_eq!(active_device(&c, "me", Some("gone")), None);
  }

  #[test]
  fn active_device_never_guesses_when_nothing_is_or_was_active() {
    let c = cluster(
      "",
      false,
      &[("phone-1", DeviceType::SMARTPHONE), ("spk-1", DeviceType::SPEAKER)],
    );
    assert_eq!(active_device(&c, "me", None), None);
  }

  #[test]
  fn start_device_in_a_car_targets_the_phone_even_when_a_speaker_is_active() {
    let c = cluster(
      "spk-1",
      false,
      &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
    );
    assert_eq!(
      start_device(&c, "me", None, Placement::Car),
      Some("phone-1".to_string())
    );
    let speakers_only = cluster("spk-1", false, &[("spk-1", DeviceType::SPEAKER)]);
    assert_eq!(start_device(&speakers_only, "me", None, Placement::Car), None);
  }

  #[test]
  fn start_device_at_a_desk_follows_the_active_session_then_the_phone() {
    let c = cluster(
      "spk-1",
      false,
      &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
    );
    assert_eq!(start_device(&c, "me", None, Placement::Desk), Some("spk-1".to_string()));
    let idle = cluster(
      "",
      false,
      &[("spk-1", DeviceType::SPEAKER), ("phone-1", DeviceType::SMARTPHONE)],
    );
    assert_eq!(
      start_device(&idle, "me", None, Placement::Desk),
      Some("phone-1".to_string()),
      "no guessed speaker when nothing is active"
    );
  }

  #[test]
  fn active_device_never_targets_me() {
    let c = cluster("", false, &[("me", DeviceType::OBSERVER)]);
    assert_eq!(active_device(&c, "me", Some("me")), None);
  }

  #[test]
  fn phone_device_picks_smartphone_or_tablet_only() {
    let c = cluster(
      "",
      false,
      &[
        ("spk-1", DeviceType::SPEAKER),
        ("tab-1", DeviceType::TABLET),
        ("phone-1", DeviceType::SMARTPHONE),
      ],
    );
    assert_eq!(phone_device(&c, "me"), Some("phone-1".to_string()), "sorted id order");
    let no_phone = cluster("", false, &[("spk-1", DeviceType::SPEAKER)]);
    assert_eq!(phone_device(&no_phone, "me"), None);
  }

  #[test]
  fn phone_device_prefers_the_active_phone() {
    let c = cluster(
      "phone-2",
      false,
      &[("phone-1", DeviceType::SMARTPHONE), ("phone-2", DeviceType::SMARTPHONE)],
    );
    assert_eq!(phone_device(&c, "me"), Some("phone-2".to_string()));
  }

  #[test]
  fn cluster_playing_requires_playing_and_not_paused() {
    assert!(cluster_playing(&cluster("x", true, &[])));
    assert!(!cluster_playing(&cluster("x", false, &[])));
    let mut both = cluster("x", true, &[]);
    both.player_state.mut_or_insert_default().is_paused = true;
    assert!(!cluster_playing(&both));
  }

  #[test]
  fn queued_track_carries_both_markers_a_receiver_looks_at() {
    let t = queued_track("spotify:track:a");
    assert_eq!(t.provider, "queue");
    assert_eq!(t.metadata.get("is_queued").map(String::as_str), Some("true"));
    assert!(is_queued(&t));

    let mut context_track = ProvidedTrack::new();
    context_track.uri = "spotify:track:b".to_string();
    context_track.provider = "context".to_string();
    assert!(!is_queued(&context_track));
  }

  #[test]
  fn provided_track_json_omits_defaults_and_round_trips() {
    let mut t = ProvidedTrack::new();
    t.uri = "spotify:track:a".to_string();
    t.uid = "q0".to_string();
    t.provider = "queue".to_string();
    t.album_uri = "spotify:album:z".to_string();
    t.disallow_reasons.push("no_prev".to_string());
    t.metadata.insert("title".to_string(), "A".to_string());

    let encoded = provided_track_json(&t);
    let obj = encoded.as_object().unwrap();
    assert!(!obj.contains_key("artist_uri"));
    assert!(!obj.contains_key("removed"));
    assert_eq!(obj["metadata"]["title"], "A");

    let decoded = provided_track_from_json(&encoded);
    assert_eq!(decoded.uri, t.uri);
    assert_eq!(decoded.uid, t.uid);
    assert_eq!(decoded.provider, t.provider);
    assert_eq!(decoded.album_uri, t.album_uri);
    assert_eq!(decoded.disallow_reasons, t.disallow_reasons);
    assert_eq!(decoded.metadata, t.metadata);
  }
}

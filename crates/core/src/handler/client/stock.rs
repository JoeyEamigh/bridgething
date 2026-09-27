use std::{collections::HashMap, time::Duration};

use base64::Engine as _;
use libbridgething::{
  BrowseEntry, ItemKind, ItemRef, LibraryItem, QueuePosition,
  client::ClientLegacyStockCommand,
  gateway::{
    self, BridgeToGatewayLibraryMsgCommand, BridgeToGatewayPlayerMsgCommand, LibraryBrowseRequest,
    LibraryFavoritesContainsRequest, LibraryResolveContextRequest,
  },
  stock::{StockPreset, StockSetPreset},
  wire::RequestError,
};
use serde_json::{Value, json};

use super::{HandlerResult, MsgHandle, asset::AssetLane};
use crate::{
  asset::{CachedAsset, Retention, art, wait::FetchOutcome},
  bluetooth::BluetoothMan,
  state::State,
  stock::{
    GraphqlError, StockConnectionType, StockInterAppSend, StockInterAppSendPayload, StockPermissionsSend, StockTip,
    interapp::{stock_art, stock_art_id},
    presets,
  },
};

const DJ_PLAYLIST_URI: &str = "spotify:playlist:37i9dQZF1EYkqdzj48dyYq";
const STOCK_BROWSE_LIMIT_MAX: u32 = 100;
const STOCK_IMAGE_RETRY_BACKOFF: Duration = Duration::from_secs(5);
const STOCK_IMAGE_DEADLINE: Duration = Duration::from_secs(25);
const STOCK_HOME_SECTIONS: u32 = 10;

#[derive(Debug)]
pub struct LegacyStockHandler {
  handle: MsgHandle,
}

impl LegacyStockHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  pub async fn handle(self, msg: ClientLegacyStockCommand) -> HandlerResult {
    tracing::debug!(
      "({}) handling legacy stock message: id: {:?}; stock_msg_id: {:?}",
      &self.handle.from,
      &self.handle.id,
      &self.handle.stock_msg_id
    );

    match msg {
      ClientLegacyStockCommand::GetImage { id } => self.get_image(id).await,
      ClientLegacyStockCommand::GetThumbnailImage { id } => self.get_thumbnail_image(id).await,
      ClientLegacyStockCommand::GetNextTracks => self.get_next_tracks().await,

      ClientLegacyStockCommand::SpotifyGetChildren {
        parent_id,
        limit,
        offset,
      } => self.spotify_get_children(parent_id, limit, offset).await,
      ClientLegacyStockCommand::SpotifyGetHome { limit, limit_overrides } => {
        self.spotify_get_home(limit, limit_overrides).await
      }
      ClientLegacyStockCommand::SpotifyGetPermissions => self.spotify_get_permissions().await,
      ClientLegacyStockCommand::SpotifyGetPlayerState => self.spotify_get_player_state().await,
      ClientLegacyStockCommand::SpotifyGetPodcast { uri, limit, offset } => {
        self.spotify_get_podcast(uri, limit, offset).await
      }
      ClientLegacyStockCommand::SpotifyGetPresets => self.spotify_get_presets().await,
      ClientLegacyStockCommand::SpotifyGetSaved { id } => self.spotify_get_saved(id).await,
      ClientLegacyStockCommand::SpotifyGetSessionState => self.spotify_get_session_state().await,
      ClientLegacyStockCommand::SpotifyGetTips => self.spotify_get_tips().await,
      ClientLegacyStockCommand::SpotifyPlayPodcastTrailer { uri } => self.spotify_play_podcast_trailer(uri).await,
      ClientLegacyStockCommand::SpotifyQueueUri { uri } => self.spotify_queue_uri(uri).await,
      ClientLegacyStockCommand::SpotifySetPodcastPlaybackSpeed { playback_speed } => {
        self.spotify_set_podcast_playback_speed(playback_speed).await
      }
      ClientLegacyStockCommand::SpotifySetPreset { presets } => self.spotify_set_preset(presets).await,
      ClientLegacyStockCommand::SpotifySetSaved { id, uri, saved } => self.spotify_set_saved(id, uri, saved).await,
      ClientLegacyStockCommand::SpotifySummonDj => self.spotify_summon_dj().await,
      ClientLegacyStockCommand::SpotifyPlayUri {
        uri,
        feature_identifier,
        interaction_id,
        skip_to_uri,
        skip_to_uid,
      } => {
        self
          .spotify_play_uri(uri, feature_identifier, interaction_id, skip_to_uri, skip_to_uid)
          .await
      }
      ClientLegacyStockCommand::SpotifyGraphql { payload } => self.spotify_graphql(payload).await,
      ClientLegacyStockCommand::SuperbirdPhoneCallImage { phone_number } => {
        self.superbird_phone_call_image(phone_number).await
      }
    }
  }

  async fn get_image(self, id: String) -> HandlerResult {
    self.serve_asset_to_stock(art::hero(&id)).await
  }

  async fn get_thumbnail_image(self, id: String) -> HandlerResult {
    self
      .serve_asset_to_stock(art::with_edge(&id, art::THUMBNAIL_EDGE))
      .await
  }

  async fn serve_asset_to_stock(self, id: String) -> HandlerResult {
    tracing::debug!("({}) stock image lookup for id: {}", &self.handle.from, id);
    let stock_msg_id = self.handle.stock_msg_id;
    let deadline = tokio::time::Instant::now() + STOCK_IMAGE_DEADLINE;

    loop {
      if let FetchOutcome::Got(asset) =
        super::asset::resolve_asset(&self.handle.state, &self.handle.bluetooth, &id).await
      {
        return self.send_stock_image(stock_msg_id, &asset).await;
      }

      let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
      if remaining.is_zero() || super::asset::asset_lane(&self.handle.state, &id) != AssetLane::Companion {
        tracing::debug!("({}) no asset for stock image id: {}", &self.handle.from, id);
        return self.send_stock_image_error(stock_msg_id, &id).await;
      }

      tokio::time::sleep(STOCK_IMAGE_RETRY_BACKOFF.min(remaining)).await;
    }
  }

  async fn send_stock_image(&self, stock_msg_id: Option<usize>, asset: &CachedAsset) -> HandlerResult {
    let image_data = base64::engine::general_purpose::STANDARD.encode(&asset.bytes);
    self
      .handle
      .send_stock(StockInterAppSend::new(
        stock_msg_id,
        StockInterAppSendPayload::Image {
          height: 0,
          width: 0,
          image_data,
        },
      ))
      .await?;
    Ok(())
  }

  async fn send_stock_image_error(&self, stock_msg_id: Option<usize>, id: &str) -> HandlerResult {
    self
      .handle
      .send_stock(StockInterAppSend::new(
        stock_msg_id,
        StockInterAppSendPayload::CallError(format!("no asset for image id: {id}")),
      ))
      .await?;
    Ok(())
  }

  async fn get_next_tracks(&self) -> HandlerResult {
    let reply = self.handle.state.player.queue_reply();
    let payload = crate::stock::interapp::player_queue_to_stock(reply);
    self
      .handle
      .send_stock(StockInterAppSend::new(self.handle.stock_msg_id, payload))
      .await?;
    Ok(())
  }

  async fn spotify_get_children(&self, parent_id: String, limit: usize, offset: Option<usize>) -> HandlerResult {
    self
      .browse_through_modern(Some(parent_id), limit_to_u32(limit), offset_to_u32(offset))
      .await
  }

  async fn spotify_get_home(&self, limit: usize, _limit_overrides: HashMap<String, usize>) -> HandlerResult {
    let limit = limit_to_u32(limit);
    let payload = if self.has_gateway() {
      let req = LibraryBrowseRequest {
        node_id: None,
        limit: limit.min(STOCK_BROWSE_LIMIT_MAX),
        offset: 0,
        sections: Some(STOCK_HOME_SECTIONS),
        preview: None,
      };
      match super::library::browse_request(
        &self.handle.bluetooth.gateway_man,
        self.handle.state.capabilities.primary_addr(),
        &self.handle.state.root_browse,
        &self.handle.state.browse_content,
        &self.handle.state.player,
        req,
      )
      .await
      {
        Ok(reply) => crate::stock::interapp::library_browse_to_home(reply.result),
        Err(err) => {
          log_request_failure("library.browse (home)", &err);
          StockInterAppSendPayload::Home { items: Vec::new() }
        }
      }
    } else {
      StockInterAppSendPayload::Home { items: Vec::new() }
    };
    self
      .handle
      .send_stock(StockInterAppSend::new(self.handle.stock_msg_id, payload))
      .await?;
    Ok(())
  }

  async fn spotify_get_podcast(&self, uri: String, limit: Option<usize>, offset: Option<usize>) -> HandlerResult {
    let limit = limit.map(limit_to_u32).unwrap_or(STOCK_BROWSE_LIMIT_MAX);
    self
      .browse_through_modern(Some(uri), limit, offset_to_u32(offset))
      .await
  }

  async fn browse_through_modern(&self, node_id: Option<String>, limit: u32, offset: u32) -> HandlerResult {
    if !self.has_gateway() {
      return self.send_empty_children(limit, offset).await;
    }
    let req = LibraryBrowseRequest {
      node_id,
      limit: limit.min(STOCK_BROWSE_LIMIT_MAX),
      offset,
      sections: None,
      preview: None,
    };
    match super::library::browse_request(
      &self.handle.bluetooth.gateway_man,
      self.handle.state.capabilities.primary_addr(),
      &self.handle.state.root_browse,
      &self.handle.state.browse_content,
      &self.handle.state.player,
      req,
    )
    .await
    {
      Ok(reply) => {
        let payload = crate::stock::interapp::library_browse_to_stock(reply.result, limit, offset);
        self
          .handle
          .send_stock(StockInterAppSend::new(self.handle.stock_msg_id, payload))
          .await?;
      }
      Err(err) => {
        log_request_failure("library.browse", &err);
        self.send_empty_children(limit, offset).await?;
      }
    }
    Ok(())
  }

  async fn send_empty_children(&self, limit: u32, offset: u32) -> HandlerResult {
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::ItemChildren {
          limit: limit as usize,
          offset: offset as usize,
          total: offset as usize,
          items: Vec::new(),
        },
      ))
      .await?;
    Ok(())
  }

  async fn spotify_get_permissions(&self) -> HandlerResult {
    tracing::debug!("({}) handling Spotify permissions request", &self.handle.from);

    self
      .handle
      .send_stock(StockPermissionsSend::DevicePermissions {
        can_use_superbird: true,
        can_play_on_demand: None,
      })
      .await?;
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::Permissions {
          can_use_superbird: true,
        },
      ))
      .await?;

    Ok(())
  }

  async fn spotify_get_player_state(&self) -> HandlerResult {
    let reply = self.handle.state.player.state_reply();
    let payload = crate::stock::interapp::player_state_to_stock(reply);
    self
      .handle
      .send_stock(StockInterAppSend::new(self.handle.stock_msg_id, payload))
      .await?;
    Ok(())
  }

  async fn spotify_get_session_state(&self) -> HandlerResult {
    let snapshot = self.handle.state.capabilities.snapshot();
    let connection_type = match snapshot.network.kind {
      libbridgething::NetworkKind::Wifi | libbridgething::NetworkKind::Ethernet => StockConnectionType::Wlan,
      libbridgething::NetworkKind::Cellular => StockConnectionType::FourG,
      libbridgething::NetworkKind::Unknown => StockConnectionType::Wlan,
    };
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::SessionState {
          connection_type,
          is_in_forced_offline_mode: false,
          is_logged_in: true,
          is_offline: false,
        },
      ))
      .await?;
    Ok(())
  }

  async fn spotify_get_presets(&self) -> HandlerResult {
    let result = match presets::list(&self.handle.state.kv).await {
      Ok(list) => list,
      Err(err) => {
        tracing::warn!(?err, "stock get_presets read failed");
        Vec::new()
      }
    };
    if self.has_gateway() {
      let missing: Vec<StockPreset> = result
        .iter()
        .filter(|p| p.image_url.is_none() || p.name.is_none())
        .cloned()
        .collect();
      if !missing.is_empty() {
        let state = self.handle.state.clone();
        let bluetooth = self.handle.bluetooth.clone();
        tokio::spawn(async move { backfill_preset_art(state, bluetooth, missing).await });
      }
    }
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::Presets { result, success: true },
      ))
      .await?;
    Ok(())
  }

  async fn spotify_get_saved(&self, id: String) -> HandlerResult {
    if crate::player::is_synthetic_uri(&id) || !self.has_gateway() {
      return self.send_saved_result(false).await;
    }
    let req = LibraryFavoritesContainsRequest { uris: vec![id] };
    match super::library::favorites_contains_request(
      &self.handle.bluetooth.gateway_man,
      self.handle.state.capabilities.primary_addr(),
      req,
    )
    .await
    {
      Ok(reply) => {
        let liked = reply.liked.first().copied().unwrap_or(false);
        self.send_saved_result(liked).await?;
      }
      Err(err) => {
        log_request_failure("library.favoritesContains", &err);
        self.send_saved_result(false).await?;
      }
    }
    Ok(())
  }

  async fn send_saved_result(&self, liked: bool) -> HandlerResult {
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::Saved { saved: liked },
      ))
      .await?;
    Ok(())
  }

  async fn spotify_get_tips(&self) -> HandlerResult {
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::Tips { result: canned_tips() },
      ))
      .await?;
    Ok(())
  }

  async fn spotify_play_podcast_trailer(&self, uri: String) -> HandlerResult {
    self.forward_play(uri).await
  }

  async fn spotify_queue_uri(&self, uri: String) -> HandlerResult {
    if !self.has_gateway() {
      return self.ack().await;
    }
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast_command(BridgeToGatewayPlayerMsgCommand::Queue(gateway::QueueUri {
        uri,
        position: QueuePosition::Append,
      }))
      .await;
    self.ack().await
  }

  async fn spotify_set_podcast_playback_speed(&self, playback_speed: usize) -> HandlerResult {
    if !self.has_gateway() {
      return self.ack().await;
    }
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast_command(BridgeToGatewayPlayerMsgCommand::SetSpeed(gateway::SetSpeed {
        speed: playback_speed as f32 / 100.0,
      }))
      .await;
    self.ack().await
  }

  async fn spotify_set_preset(&self, requests: Vec<StockSetPreset>) -> HandlerResult {
    let old = presets::list(&self.handle.state.kv).await.unwrap_or_default();
    let has_gateway = self.has_gateway();
    let mut to_write: Vec<StockPreset> = Vec::with_capacity(requests.len());
    for req in requests {
      let resolved = if has_gateway {
        resolve_preset_context(&self.handle.bluetooth, &req.context_uri).await
      } else {
        None
      };
      let image_url = resolved.as_ref().and_then(|r| r.artwork_id.clone());
      let name = resolved.and_then(|r| r.name);
      if let Some(prev) = old.iter().find(|p| p.slot_index == req.slot_index)
        && let Some(prev_art) = &prev.image_url
        && Some(prev_art) != image_url.as_ref()
      {
        let _ = self.handle.state.assets.clear(prev_art).await;
      }
      if let Some(art) = &image_url {
        warm_preset_art(&self.handle.state, &self.handle.bluetooth, art).await;
      }
      to_write.push(StockPreset {
        context_uri: req.context_uri,
        image_url,
        slot_index: req.slot_index,
        name,
        description: None,
      });
    }
    if let Err(err) = presets::upsert_many(&self.handle.state.kv, &to_write).await {
      tracing::warn!(?err, "stock set_preset write failed");
    }
    let result = presets::list(&self.handle.state.kv).await.unwrap_or_default();
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::Presets { result, success: true },
      ))
      .await?;
    Ok(())
  }

  async fn spotify_set_saved(&self, id: Option<String>, uri: Option<String>, saved: bool) -> HandlerResult {
    if let Some(item_uri) = uri.or(id)
      && self.has_gateway()
      && !crate::player::is_synthetic_uri(&item_uri)
    {
      self
        .handle
        .bluetooth
        .gateway_man
        .broadcast_command(BridgeToGatewayLibraryMsgCommand::FavoritesSet(gateway::FavoritesSet {
          item: ItemRef {
            uri: item_uri,
            kind: ItemKind::Track,
            persistent_id: None,
          },
          liked: saved,
        }))
        .await;
    }
    self.ack().await
  }

  async fn spotify_summon_dj(&self) -> HandlerResult {
    self.forward_play(DJ_PLAYLIST_URI.to_string()).await
  }

  async fn spotify_play_uri(
    &self,
    uri: String,
    _feature_identifier: String,
    _interaction_id: Option<String>,
    _skip_to_uri: Option<String>,
    _skip_to_uid: Option<String>,
  ) -> HandlerResult {
    self.forward_play(uri).await
  }

  async fn spotify_graphql(&self, payload: String) -> HandlerResult {
    let result = match classify_graphql(&payload) {
      Some(GraphqlOp::Shelf { limit }) => self.graphql_shelf(limit).await,
      Some(GraphqlOp::Section { id, limit, offset }) => self.graphql_section(id, limit, offset).await,
      Some(GraphqlOp::TipsOnDemand) => Ok(graphql_tips_data()),
      Some(GraphqlOp::Presets) => Ok(self.graphql_presets_data().await),
      None => {
        tracing::debug!(?payload, "unrecognized graphql operation");
        Err("unrecognized graphql operation".to_string())
      }
    };
    let envelope = match result {
      Ok(data) => StockInterAppSendPayload::Graphql {
        data: Some(data),
        errors: None,
      },
      Err(message) => StockInterAppSendPayload::Graphql {
        data: None,
        errors: Some(vec![GraphqlError { message }]),
      },
    };
    self
      .handle
      .send_stock(StockInterAppSend::new(self.handle.stock_msg_id, envelope))
      .await?;
    Ok(())
  }

  async fn graphql_shelf(&self, limit: u32) -> Result<Value, String> {
    if !self.has_gateway() {
      return Ok(json!({ "shelf": { "items": [] } }));
    }
    let req = LibraryBrowseRequest {
      node_id: None,
      limit: limit.min(STOCK_BROWSE_LIMIT_MAX),
      offset: 0,
      sections: Some(STOCK_HOME_SECTIONS),
      preview: None,
    };
    match super::library::browse_request(
      &self.handle.bluetooth.gateway_man,
      self.handle.state.capabilities.primary_addr(),
      &self.handle.state.root_browse,
      &self.handle.state.browse_content,
      &self.handle.state.player,
      req,
    )
    .await
    {
      Ok(reply) => Ok(json!({
        "shelf": {
          "items": reply
            .result
            .entries
            .into_iter()
            .filter_map(shelf_section_value)
            .collect::<Vec<_>>(),
        }
      })),
      Err(err) => {
        log_request_failure("library.browse (shelf)", &err);
        Ok(json!({ "shelf": { "items": [] } }))
      }
    }
  }

  async fn graphql_section(&self, id: String, limit: u32, offset: u32) -> Result<Value, String> {
    if !self.has_gateway() {
      return Ok(json!({
        "section": { "id": id, "title": "", "children": [], "total": offset }
      }));
    }
    let req = LibraryBrowseRequest {
      node_id: Some(id.clone()),
      limit: limit.min(STOCK_BROWSE_LIMIT_MAX),
      offset,
      sections: None,
      preview: None,
    };
    match super::library::browse_request(
      &self.handle.bluetooth.gateway_man,
      self.handle.state.capabilities.primary_addr(),
      &self.handle.state.root_browse,
      &self.handle.state.browse_content,
      &self.handle.state.player,
      req,
    )
    .await
    {
      Ok(reply) => {
        let entries_len = reply.result.entries.len() as u32;
        let total = reply.result.total.unwrap_or_else(|| {
          let consumed = offset.saturating_add(entries_len);
          if reply.result.has_more {
            consumed.saturating_add(1)
          } else {
            consumed
          }
        });
        Ok(json!({
          "section": {
            "id": id,
            "title": "",
            "children": reply
              .result
              .entries
              .into_iter()
              .map(graphql_child_value)
              .collect::<Vec<_>>(),
            "total": total,
          }
        }))
      }
      Err(err) => {
        log_request_failure("library.browse (section)", &err);
        Ok(json!({
          "section": { "id": id, "title": "", "children": [], "total": offset }
        }))
      }
    }
  }

  async fn graphql_presets_data(&self) -> Value {
    let presets = presets::list(&self.handle.state.kv).await.unwrap_or_default();
    json!({
      "presets": {
        "presets": presets
          .iter()
          .map(|p| json!({
            "context_uri": p.context_uri,
            "name": p.name,
            "slot_index": p.slot_index,
            "description": p.description,
            "image_url": p.image_url,
          }))
          .collect::<Vec<_>>(),
      }
    })
  }

  async fn superbird_phone_call_image(&self, _phone_number: String) -> HandlerResult {
    self
      .handle
      .send_stock(StockInterAppSend::new(
        self.handle.stock_msg_id,
        StockInterAppSendPayload::Image {
          height: 0,
          width: 0,
          image_data: String::new(),
        },
      ))
      .await?;
    Ok(())
  }

  async fn forward_play(&self, uri: String) -> HandlerResult {
    if crate::player::is_synthetic_uri(&uri) || !self.has_gateway() {
      return self.ack().await;
    }
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast_command(BridgeToGatewayPlayerMsgCommand::Play(gateway::PlayUri {
        uri,
        context: None,
      }))
      .await;
    self.ack().await
  }

  fn has_gateway(&self) -> bool {
    self.handle.state.capabilities.snapshot().gateway.is_some()
  }

  async fn ack(&self) -> HandlerResult {
    self
      .handle
      .send_stock(StockInterAppSend::make_ack(self.handle.stock_msg_id))
      .await?;
    Ok(())
  }
}

fn limit_to_u32(limit: usize) -> u32 {
  u32::try_from(limit).unwrap_or(STOCK_BROWSE_LIMIT_MAX)
}

fn offset_to_u32(offset: Option<usize>) -> u32 {
  offset.and_then(|o| u32::try_from(o).ok()).unwrap_or(0)
}

fn log_request_failure(verb: &str, err: &RequestError<gateway::LibraryErrorReply>) {
  match err {
    RequestError::Domain(domain) => {
      tracing::debug!(?domain.error, "{verb} returned domain error; sending stock fallback");
    }
    RequestError::Protocol(err) => {
      tracing::warn!(?err, "{verb} protocol error; sending stock fallback");
    }
    RequestError::ResponseMismatch => {
      tracing::error!("{verb} response did not match expected shape; sending stock fallback");
    }
  }
}

enum GraphqlOp {
  Shelf { limit: u32 },
  Section { id: String, limit: u32, offset: u32 },
  TipsOnDemand,
  Presets,
}

fn classify_graphql(payload: &str) -> Option<GraphqlOp> {
  let body = payload.trim_start();
  let body = body.strip_prefix("query")?.trim_start();
  let body = body.strip_prefix('{')?.trim_start();

  if body.starts_with("section(") {
    let id = parse_quoted_arg(body, "id")?.to_string();
    let limit = parse_int_arg(body, "limit").unwrap_or(20);
    let offset = parse_int_arg(body, "offset").unwrap_or(0);
    Some(GraphqlOp::Section { id, limit, offset })
  } else if body.starts_with("shelf(") {
    let limit = parse_int_arg(body, "limit").unwrap_or(14);
    Some(GraphqlOp::Shelf { limit })
  } else if body.starts_with("tipsOnDemand") {
    Some(GraphqlOp::TipsOnDemand)
  } else if body.starts_with("presets(") {
    Some(GraphqlOp::Presets)
  } else {
    None
  }
}

fn parse_int_arg(text: &str, key: &str) -> Option<u32> {
  let needle = format!("{key}:");
  let pos = text.find(&needle)?;
  let rest = text[pos + needle.len()..].trim_start();
  let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
  if end == 0 { None } else { rest[..end].parse().ok() }
}

fn parse_quoted_arg<'a>(text: &'a str, key: &str) -> Option<&'a str> {
  let needle = format!("{key}:");
  let pos = text.find(&needle)?;
  let rest = text[pos + needle.len()..].trim_start();
  let rest = rest.strip_prefix('"')?;
  let end = rest.find('"')?;
  Some(&rest[..end])
}

fn shelf_section_value(entry: BrowseEntry) -> Option<Value> {
  let folder = match entry {
    BrowseEntry::Folder(f) => f,
    BrowseEntry::Item(_) => return None,
  };
  let preview = folder.preview_children.unwrap_or_default();
  let total = folder
    .total
    .unwrap_or_else(|| u32::try_from(preview.len()).unwrap_or(u32::MAX));
  Some(json!({
    "title": folder.title,
    "id": folder.node_id,
    "total": total,
    "children": preview.into_iter().map(graphql_child_value).collect::<Vec<_>>(),
  }))
}

fn graphql_child_value(entry: BrowseEntry) -> Value {
  match entry {
    BrowseEntry::Folder(f) => json!({
      "uri": f.node_id,
      "title": f.title,
      "subtitle": f.subtitle.unwrap_or_default(),
      "image_id": stock_art(f.artwork_id),
    }),
    BrowseEntry::Item(item) => library_item_to_graphql_child(item),
  }
}

fn library_item_to_graphql_child(item: LibraryItem) -> Value {
  match item {
    LibraryItem::Track(t) => json!({
      "uri": t.id,
      "title": t.name,
      "subtitle": t.artist.name,
      "image_id": stock_art_id(t.image_id),
    }),
    LibraryItem::Album(a) => json!({
      "uri": a.id,
      "title": a.name,
      "subtitle": "",
      "image_id": stock_art(a.artwork_id),
    }),
    LibraryItem::Playlist(p) => json!({
      "uri": p.uri,
      "title": p.name,
      "subtitle": p.owner_name.unwrap_or_default(),
      "image_id": stock_art(p.artwork_id),
    }),
    LibraryItem::PodcastEpisode(e) => json!({
      "uri": e.uri,
      "title": e.name,
      "subtitle": e.show_name.unwrap_or_default(),
      "image_id": stock_art(e.artwork_id),
    }),
    LibraryItem::Show(s) => json!({
      "uri": s.uri,
      "title": s.name,
      "subtitle": s.publisher.unwrap_or_default(),
      "image_id": stock_art(s.artwork_id),
    }),
    LibraryItem::Artist(a) => json!({
      "uri": a.id,
      "title": a.name,
      "subtitle": "",
      "image_id": stock_art(a.artwork_id),
    }),
    LibraryItem::Station(s) => json!({
      "uri": s.uri,
      "title": s.name,
      "subtitle": "",
      "image_id": stock_art(s.artwork_id),
    }),
  }
}

fn graphql_tips_data() -> Value {
  let tips = canned_tips()
    .into_iter()
    .map(|t| {
      json!({
        "id": t.id,
        "title": t.title,
        "description": t.description,
      })
    })
    .collect::<Vec<_>>();
  json!({ "tipsOnDemand": { "tips": tips } })
}

async fn resolve_preset_context(bluetooth: &BluetoothMan, uri: &str) -> Option<gateway::ContextResolveReply> {
  match bluetooth
    .gateway_man
    .request(None, LibraryResolveContextRequest { uri: uri.to_string() })
    .await
  {
    Ok(reply) => Some(reply),
    Err(err) => {
      tracing::debug!(?err, %uri, "preset context resolve failed");
      None
    }
  }
}

async fn warm_preset_art(state: &State, bluetooth: &BluetoothMan, id: &str) {
  if matches!(state.assets.contains(id).await, Ok(true)) {
    return;
  }
  super::asset::fetch_via_companion(state, bluetooth, id, Retention::DISK_PINNED).await;
}

async fn backfill_preset_art(state: State, bluetooth: BluetoothMan, missing: Vec<StockPreset>) {
  let mut updated: Vec<StockPreset> = Vec::new();
  for mut preset in missing {
    let Some(resolved) = resolve_preset_context(&bluetooth, &preset.context_uri).await else {
      continue;
    };
    if preset.name.is_none() {
      preset.name = resolved.name;
    }
    if preset.image_url.is_none() {
      preset.image_url = resolved.artwork_id;
    }
    if let Some(art) = &preset.image_url {
      warm_preset_art(&state, &bluetooth, art).await;
    }
    updated.push(preset);
  }
  if !updated.is_empty()
    && let Err(err) = presets::upsert_many(&state.kv, &updated).await
  {
    tracing::warn!(?err, "preset backfill write failed");
  }
}

fn canned_tips() -> Vec<StockTip> {
  vec![
    StockTip {
      id: 1,
      title: "running on bridgething".into(),
      description: "this car thing is alive thanks to thinglabs.".into(),
      action: "".into(),
    },
    StockTip {
      id: 2,
      title: "no spotify required".into(),
      description: "your phone's the boss now. spotify, apple music, whatever you connect.".into(),
      action: "".into(),
    },
    StockTip {
      id: 3,
      title: "press the wheel to chill".into(),
      description: "single click pauses, double-click skips. classic car thing moves.".into(),
      action: "".into(),
    },
  ]
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classify_shelf() {
    let payload =
      "query{shelf(limit:14 overrides:[]){...on Shelf{items{title id total children{uri}}}...on ShelfError{message}}}";
    let op = classify_graphql(payload).expect("shelf classifies");
    let GraphqlOp::Shelf { limit } = op else {
      panic!("expected Shelf");
    };
    assert_eq!(limit, 14);
  }

  #[test]
  fn classify_shelf_default_limit_when_missing() {
    let payload = "query{shelf(overrides:[]){...on Shelf{items{title}}}}";
    let op = classify_graphql(payload).expect("shelf classifies");
    let GraphqlOp::Shelf { limit } = op else {
      panic!("expected Shelf");
    };
    assert_eq!(limit, 14);
  }

  #[test]
  fn classify_section() {
    let payload =
      "query{section(id:\"spotify:section:home-recently-played\" limit:20 offset:5){...on ShelfSection{id title}}}";
    let op = classify_graphql(payload).expect("section classifies");
    let GraphqlOp::Section { id, limit, offset } = op else {
      panic!("expected Section");
    };
    assert_eq!(id, "spotify:section:home-recently-played");
    assert_eq!(limit, 20);
    assert_eq!(offset, 5);
  }

  #[test]
  fn classify_section_with_commas() {
    let payload = "query{section(id:\"abc\", limit:10, offset:0){children{uri}}}";
    let op = classify_graphql(payload).expect("section classifies with commas");
    let GraphqlOp::Section { id, limit, offset } = op else {
      panic!("expected Section");
    };
    assert_eq!(id, "abc");
    assert_eq!(limit, 10);
    assert_eq!(offset, 0);
  }

  #[test]
  fn classify_tips() {
    let payload = "query{tipsOnDemand{...on Tips{tips{id title description}}...on TipsError{message}}}";
    assert!(matches!(classify_graphql(payload), Some(GraphqlOp::TipsOnDemand)));
  }

  #[test]
  fn classify_presets() {
    let payload = "query{presets(serial:\"000000000000\"){... on Presets{presets{context_uri:contextUri}}}}";
    assert!(matches!(classify_graphql(payload), Some(GraphqlOp::Presets)));
  }

  #[test]
  fn classify_unknown() {
    assert!(classify_graphql("query{somethingElse}").is_none());
    assert!(classify_graphql("mutation{setPreset(input:{...})}").is_none());
    assert!(classify_graphql("").is_none());
  }

  #[test]
  fn classify_with_leading_whitespace() {
    let payload = "  query  {  shelf(limit:7){items{id}}  }";
    let op = classify_graphql(payload).expect("classifies after whitespace");
    let GraphqlOp::Shelf { limit } = op else {
      panic!("expected Shelf");
    };
    assert_eq!(limit, 7);
  }

  #[test]
  fn parse_int_arg_basic() {
    assert_eq!(parse_int_arg("limit:42 something", "limit"), Some(42));
    assert_eq!(parse_int_arg("limit: 42", "limit"), Some(42));
    assert_eq!(parse_int_arg("offset:0", "offset"), Some(0));
    assert_eq!(parse_int_arg("limit:abc", "limit"), None);
    assert_eq!(parse_int_arg("nothing here", "limit"), None);
  }

  #[test]
  fn parse_quoted_arg_basic() {
    assert_eq!(parse_quoted_arg("id:\"abc\"", "id"), Some("abc"));
    assert_eq!(parse_quoted_arg("id: \"with spaces\"", "id"), Some("with spaces"));
    assert_eq!(parse_quoted_arg("id:notquoted", "id"), None);
    assert_eq!(parse_quoted_arg("nothing here", "id"), None);
  }

  #[test]
  fn graphql_child_value_track() {
    use libbridgething::Track;
    let entry = BrowseEntry::Item(LibraryItem::Track(Track {
      id: "spotify:track:abc".into(),
      name: "Hey".into(),
      artist: libbridgething::Artist {
        id: "spotify:artist:x".into(),
        name: "Artist".into(),
        artwork_id: None,
      },
      ..Track::default()
    }));
    let v = graphql_child_value(entry);
    assert_eq!(v["uri"], "spotify:track:abc");
    assert_eq!(v["title"], "Hey");
    assert_eq!(v["subtitle"], "Artist");
  }

  #[test]
  fn shelf_section_value_uses_preview_children_and_total() {
    use libbridgething::BrowseFolder;
    let folder = BrowseFolder {
      node_id: "section:1".into(),
      title: "Recently Played".into(),
      subtitle: None,
      artwork_id: None,
      total: Some(50),
      preview_children: Some(vec![BrowseEntry::Folder(BrowseFolder {
        node_id: "child:1".into(),
        title: "Pop".into(),
        subtitle: Some("Top Hits".into()),
        artwork_id: Some("img-1".into()),
        total: None,
        preview_children: None,
      })]),
    };
    let v = shelf_section_value(BrowseEntry::Folder(folder)).expect("folder maps");
    assert_eq!(v["title"], "Recently Played");
    assert_eq!(v["id"], "section:1");
    assert_eq!(v["total"], 50);
    let children = v["children"].as_array().expect("children array");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["uri"], "child:1");
    assert_eq!(children[0]["title"], "Pop");
    assert_eq!(children[0]["subtitle"], "Top Hits");
    assert_eq!(children[0]["image_id"], "img-1");
  }

  #[test]
  fn shelf_section_value_filters_root_items() {
    use libbridgething::Track;
    let entry = BrowseEntry::Item(LibraryItem::Track(Track::default()));
    assert!(shelf_section_value(entry).is_none());
  }
}

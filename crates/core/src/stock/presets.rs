use std::time::Duration;

use futures::future::join_all;
use libbridgething::{
  gateway::{ContextResolveReply, LibraryResolveContextRequest},
  stock::StockPreset,
};
use uuid::Uuid;

use crate::{
  asset::{Retention, art, builtin::BUILTIN_ART_PREFIX, wait::FetchOutcome},
  bluetooth::BluetoothMan,
  chrome::ChromeCommand,
  handler::client::asset::resolve_asset,
  state::{KvStore, STOCK_WEBAPP_ID, State, StateResult},
};

const PRESET_SLOTS: [usize; 4] = [1, 2, 3, 4];
const PRESETS_KEY_PREFIX: &str = "presets:";
const STOCK_SCOPE: Uuid = Uuid::nil();
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

fn slot_key(slot: usize) -> String {
  format!("{PRESETS_KEY_PREFIX}{slot}")
}

pub async fn list(kv: &KvStore) -> StateResult<Vec<StockPreset>> {
  let rows = kv.data_list_prefix(STOCK_SCOPE, PRESETS_KEY_PREFIX).await?;
  let mut out: Vec<StockPreset> = rows
    .into_iter()
    .filter_map(|(_, raw)| serde_json::from_str::<StockPreset>(&raw).ok())
    .collect();
  out.sort_by_key(|p| p.slot_index);
  Ok(out)
}

async fn upsert_many(kv: &KvStore, presets: &[StockPreset]) -> StateResult<()> {
  let items: Vec<(String, String)> = presets
    .iter()
    .filter_map(|preset| {
      if !PRESET_SLOTS.contains(&preset.slot_index) {
        tracing::warn!(slot = preset.slot_index, "stock preset slot out of range; ignoring");
        return None;
      }
      let value = serde_json::to_string(preset).expect("StockPreset is always serializable");
      Some((slot_key(preset.slot_index), value))
    })
    .collect();
  if items.is_empty() {
    return Ok(());
  }
  kv.data_set_many(STOCK_SCOPE, items).await
}

pub async fn load(state: &State, bluetooth: &BluetoothMan) -> StateResult<Vec<StockPreset>> {
  let stored = list(&state.kv).await?;
  let repaired = join_all(
    stored
      .iter()
      .filter(|preset| lacks_metadata(preset))
      .cloned()
      .map(|preset| repair(state, bluetooth, preset)),
  )
  .await;
  let changed: Vec<StockPreset> = repaired.into_iter().filter(|preset| !stored.contains(preset)).collect();
  let presets = if changed.is_empty() {
    stored
  } else {
    upsert_many(&state.kv, &changed).await?;
    list(&state.kv).await?
  };
  pin_art(state, bluetooth, &presets);
  Ok(presets)
}

pub async fn repair_for_stock(state: &State, bluetooth: &BluetoothMan) {
  if !matches!(state.active_webapp().await, Ok(Some(id)) if id == STOCK_WEBAPP_ID) {
    return;
  }
  let Ok(stored) = list(&state.kv).await else {
    return;
  };
  if !stored.iter().any(lacks_metadata) {
    return;
  }
  match load(state, bluetooth).await {
    Ok(repaired) if repaired != stored => {
      tracing::info!("stock presets repaired after a companion connected; reloading the webapp");
      if let Err(err) = state.chrome.send(ChromeCommand::Reload).await {
        tracing::warn!(?err, "stock preset reload failed");
      }
    }
    Ok(_) => {}
    Err(err) => tracing::warn!(?err, "stock preset repair failed"),
  }
}

pub async fn save(state: &State, bluetooth: &BluetoothMan, drafts: Vec<StockPreset>) -> StateResult<Vec<StockPreset>> {
  let previous = list(&state.kv).await?;
  let saved = join_all(
    drafts
      .into_iter()
      .map(|draft| inherit(draft, &previous))
      .map(|draft| repair(state, bluetooth, draft)),
  )
  .await;
  upsert_many(&state.kv, &saved).await?;
  let current = list(&state.kv).await?;
  release_art(state, &previous, &current).await;
  pin_art(state, bluetooth, &saved);
  Ok(current)
}

fn lacks_metadata(preset: &StockPreset) -> bool {
  preset.name.is_none() || preset.image_url.is_none()
}

fn inherit(mut draft: StockPreset, previous: &[StockPreset]) -> StockPreset {
  if let Some(prev) = previous
    .iter()
    .find(|p| p.slot_index == draft.slot_index && p.context_uri == draft.context_uri)
  {
    draft.name = draft.name.or_else(|| prev.name.clone());
    draft.image_url = draft.image_url.or_else(|| prev.image_url.clone());
    draft.description = draft.description.or_else(|| prev.description.clone());
  }
  draft
}

async fn repair(state: &State, bluetooth: &BluetoothMan, mut preset: StockPreset) -> StockPreset {
  if !lacks_metadata(&preset) {
    return preset;
  }
  if let Some(resolved) = resolve(state, bluetooth, &preset.context_uri).await {
    preset.name = preset.name.or(resolved.name);
    preset.image_url = preset.image_url.or(resolved.artwork_id);
  }
  preset
}

async fn resolve(state: &State, bluetooth: &BluetoothMan, uri: &str) -> Option<ContextResolveReply> {
  state.gateway_info()?;
  let request = LibraryResolveContextRequest { uri: uri.to_string() };
  match bluetooth
    .gateway_man
    .request_with_timeout(None, request, RESOLVE_TIMEOUT)
    .await
  {
    Ok(reply) if reply.name.is_some() || reply.artwork_id.is_some() => Some(reply),
    Ok(_) => {
      tracing::debug!(%uri, "preset context resolved without metadata");
      None
    }
    Err(err) => {
      tracing::debug!(?err, %uri, "preset context resolve failed");
      None
    }
  }
}

async fn release_art(state: &State, previous: &[StockPreset], current: &[StockPreset]) {
  for url in previous.iter().filter_map(|p| p.image_url.as_deref()) {
    if current.iter().all(|p| p.image_url.as_deref() != Some(url)) {
      let _ = state.assets.set_retention(&art::hero(url), Retention::DISK_LRU).await;
    }
  }
}

fn pin_art(state: &State, bluetooth: &BluetoothMan, presets: &[StockPreset]) {
  let ids: Vec<String> = presets
    .iter()
    .filter_map(|p| p.image_url.as_deref())
    .map(art::hero)
    .filter(|id| !id.starts_with(BUILTIN_ART_PREFIX))
    .collect();
  if ids.is_empty() {
    return;
  }
  let state = state.clone();
  let bluetooth = bluetooth.clone();
  tokio::spawn(async move {
    for id in ids {
      let FetchOutcome::Got(asset) = resolve_asset(&state, &bluetooth, &id).await else {
        continue;
      };
      if let Err(err) = state
        .assets
        .insert_internal(id, asset.bytes, asset.mime, Retention::DISK_PINNED)
        .await
      {
        tracing::warn!(?err, "preset art pin failed");
      }
    }
  });
}

use std::{collections::HashSet, net::SocketAddr, path::PathBuf, sync::Arc};

use cache::{AssetLog, RangeCache};
use libbridgething::RangePart;
use tokio::{
  sync::{mpsc, oneshot},
  task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
  asset::AssetCache,
  bluetooth::{Address, BluetoothMan},
  transfer::sinks::TransferSinks,
};

mod cache;
mod layout;
mod server;

const BROKER_MAILBOX: usize = 64;

#[derive(Clone)]
pub struct RangeProxy {
  cmd_tx: mpsc::Sender<BrokerCmd>,
}

impl std::fmt::Debug for RangeProxy {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("RangeProxy").finish_non_exhaustive()
  }
}

pub struct RangeProxyHandle {
  pub proxy: RangeProxy,
  pub cancel: CancellationToken,
  #[cfg(feature = "test-tap")]
  pub bound_addr: Option<SocketAddr>,
  _broker: JoinHandle<()>,
  _server: Option<JoinHandle<()>>,
}

impl RangeProxy {
  pub async fn spawn(
    bluetooth: BluetoothMan,
    sinks: TransferSinks,
    assets: AssetCache,
    cache_dir: PathBuf,
    bind: SocketAddr,
  ) -> RangeProxyHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(BROKER_MAILBOX);
    let cancel = CancellationToken::new();

    let broker = BrokerActor {
      cmd_rx,
      sinks: sinks.clone(),
      assets,
      cache_dir,
      active: None,
      cache: None,
      inflight: Default::default(),
    };
    let _broker = tokio::spawn(broker.run());

    let proxy = RangeProxy { cmd_tx };
    let bound = match server::spawn(proxy.clone(), bluetooth, sinks, bind, cancel.clone()).await {
      Ok((addr, handle)) => Some((addr, handle)),
      Err(err) => {
        tracing::error!(
          ?err,
          "ota range proxy failed to bind {bind}; delta OTA over wire unavailable until restart",
        );
        None
      }
    };

    RangeProxyHandle {
      proxy,
      cancel,
      #[cfg(feature = "test-tap")]
      bound_addr: bound.as_ref().map(|(addr, _)| *addr),
      _broker,
      _server: bound.map(|(_, handle)| handle),
    }
  }

  pub async fn activate(&self, update_id: String, peer: Option<Address>) {
    if let Err(err) = self.cmd_tx.send(BrokerCmd::Activate { update_id, peer }).await {
      tracing::error!(?err, "range proxy mailbox closed; activate dropped");
    }
  }

  pub async fn deactivate(&self) {
    if let Err(err) = self.cmd_tx.send(BrokerCmd::Deactivate).await {
      tracing::error!(?err, "range proxy mailbox closed; deactivate dropped");
    }
  }

  pub async fn clear_cache(&self, update_id: String) {
    if let Err(err) = self.cmd_tx.send(BrokerCmd::ClearCache { update_id }).await {
      tracing::error!(?err, "range proxy mailbox closed; cache clear dropped");
    }
  }

  pub async fn note_write_failure(&self, update_id: String) {
    if let Err(err) = self.cmd_tx.send(BrokerCmd::NoteWriteFailure { update_id }).await {
      tracing::error!(?err, "range proxy mailbox closed; write failure not counted");
    }
  }

  async fn begin_range_active(&self, request_id: Uuid, asset: String) -> Result<RangeBegin, BeginRangeError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .cmd_tx
      .send(BrokerCmd::BeginRange {
        request_id,
        asset,
        reply: reply_tx,
      })
      .await
      .map_err(|_| BeginRangeError::ProxyDown)?;
    reply_rx.await.map_err(|_| BeginRangeError::ProxyDown)?
  }

  async fn end_range(&self, request_id: Uuid) {
    let _ = self.cmd_tx.send(BrokerCmd::EndRange { request_id }).await;
  }

  async fn store_ranges(&self, asset: String, parts: Vec<RangePart>, total: u32) {
    let _ = self.cmd_tx.send(BrokerCmd::StoreRanges { asset, parts, total }).await;
  }

  async fn load_ranges(&self, asset: String) -> Option<(Vec<RangePart>, u32)> {
    let (reply_tx, reply_rx) = oneshot::channel();
    self
      .cmd_tx
      .send(BrokerCmd::LoadRanges { asset, reply: reply_tx })
      .await
      .ok()?;
    reply_rx.await.ok().flatten()
  }
}

#[derive(Debug, Clone)]
struct RangeBegin {
  pub update_id: String,
  pub peer: Option<Address>,
  pub log: Arc<AssetLog>,
}

#[derive(Debug)]
enum BeginRangeError {
  NoActiveOta,
  ProxyDown,
  Cache(String),
}

#[derive(Debug)]
enum BrokerCmd {
  Activate {
    update_id: String,
    peer: Option<Address>,
  },
  Deactivate,
  ClearCache {
    update_id: String,
  },
  NoteWriteFailure {
    update_id: String,
  },
  BeginRange {
    request_id: Uuid,
    asset: String,
    reply: oneshot::Sender<Result<RangeBegin, BeginRangeError>>,
  },
  EndRange {
    request_id: Uuid,
  },
  StoreRanges {
    asset: String,
    parts: Vec<RangePart>,
    total: u32,
  },
  LoadRanges {
    asset: String,
    reply: oneshot::Sender<Option<(Vec<RangePart>, u32)>>,
  },
}

#[derive(Debug, Clone)]
struct ActiveOta {
  update_id: String,
  peer: Option<Address>,
}

struct BrokerActor {
  cmd_rx: mpsc::Receiver<BrokerCmd>,
  sinks: TransferSinks,
  assets: AssetCache,
  cache_dir: PathBuf,
  active: Option<ActiveOta>,
  cache: Option<RangeCache>,
  inflight: HashSet<Uuid>,
}

impl BrokerActor {
  async fn run(mut self) {
    tracing::info!("ota range proxy broker started");
    while let Some(cmd) = self.cmd_rx.recv().await {
      match cmd {
        BrokerCmd::Activate { update_id, peer } => {
          tracing::info!(%update_id, ?peer, "range proxy activated");
          self.cache = match RangeCache::open(&self.cache_dir, &update_id, self.assets.clone()).await {
            Ok(cache) => Some(cache),
            Err(err) => {
              tracing::error!(?err, %update_id, "range cache unavailable; delta ranges cannot be served");
              None
            }
          };
          self.active = Some(ActiveOta { update_id, peer });
        }
        BrokerCmd::Deactivate => {
          if let Some(active) = self.active.take() {
            tracing::info!(update_id = %active.update_id, "range proxy deactivated; cache retained");
          }
          self.cache = None;
          for request_id in self.inflight.drain() {
            self.sinks.unbind(request_id);
          }
        }
        BrokerCmd::ClearCache { update_id } => {
          self.close_cache_for(&update_id);
          cache::clear(&self.cache_dir, &update_id).await;
        }
        BrokerCmd::NoteWriteFailure { update_id } => {
          self.close_cache_for(&update_id);
          cache::note_write_failure(&self.cache_dir, &update_id).await;
        }
        BrokerCmd::BeginRange {
          request_id,
          asset,
          reply,
        } => {
          let result = self.begin_range(request_id, &asset).await;
          let _ = reply.send(result);
        }
        BrokerCmd::EndRange { request_id } => {
          self.inflight.remove(&request_id);
          self.sinks.unbind(request_id);
        }
        BrokerCmd::StoreRanges { asset, parts, total } => {
          if let Some(cache) = &self.cache {
            cache.store_ranges(asset, parts, total).await;
          }
        }
        BrokerCmd::LoadRanges { asset, reply } => {
          let remembered = match &self.cache {
            Some(cache) => cache.load_ranges(&asset).await,
            None => None,
          };
          let _ = reply.send(remembered);
        }
      }
    }
    tracing::info!("ota range proxy broker exiting");
  }

  fn close_cache_for(&mut self, update_id: &str) {
    if self.cache.as_ref().is_some_and(|c| c.update_id() == update_id) {
      self.cache = None;
    }
  }

  async fn begin_range(&mut self, request_id: Uuid, asset: &str) -> Result<RangeBegin, BeginRangeError> {
    let active = self.active.as_ref().ok_or(BeginRangeError::NoActiveOta)?;
    let cache = self
      .cache
      .as_ref()
      .ok_or_else(|| BeginRangeError::Cache("range cache is not open".into()))?;
    let log = cache
      .asset_log(asset)
      .await
      .map_err(|err| BeginRangeError::Cache(err.to_string()))?;
    self.inflight.insert(request_id);
    Ok(RangeBegin {
      update_id: active.update_id.clone(),
      peer: active.peer,
      log,
    })
  }
}

#[cfg(test)]
pub fn noop_proxy() -> RangeProxy {
  let (cmd_tx, mut cmd_rx) = mpsc::channel::<BrokerCmd>(16);
  tokio::spawn(async move { while cmd_rx.recv().await.is_some() {} });
  RangeProxy { cmd_tx }
}

#[cfg(test)]
mod tests {
  use tokio_util::bytes::Bytes;

  use super::*;
  use crate::transfer::sinks::{AckPolicy, TransferEvent};

  pub(super) async fn spawn_broker_only(sinks: TransferSinks, cache_dir: PathBuf) -> RangeProxy {
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    let broker = BrokerActor {
      cmd_rx,
      sinks,
      assets: cache::tests::assets().await,
      cache_dir,
      active: None,
      cache: None,
      inflight: Default::default(),
    };
    tokio::spawn(broker.run());
    RangeProxy { cmd_tx }
  }

  async fn broker() -> (RangeProxy, PathBuf) {
    let dir = cache::tests::temp_dir();
    let proxy = spawn_broker_only(TransferSinks::default(), dir.clone()).await;
    (proxy, dir)
  }

  #[tokio::test]
  async fn begin_range_with_no_active_returns_no_active_ota() {
    let (proxy, _dir) = broker().await;
    let result = proxy.begin_range_active(Uuid::now_v7(), "a.zck".into()).await;
    assert!(matches!(result, Err(BeginRangeError::NoActiveOta)));
  }

  #[tokio::test]
  async fn begin_range_returns_active_update_id_and_peer() {
    let (proxy, _dir) = broker().await;
    proxy.activate("expected-id".into(), None).await;
    let begin = proxy
      .begin_range_active(Uuid::now_v7(), "a.zck".into())
      .await
      .expect("begin should succeed");
    assert_eq!(begin.update_id, "expected-id");
    assert!(begin.peer.is_none());
  }

  #[tokio::test]
  async fn deactivate_unbinds_inflight_sinks() {
    let dir = cache::tests::temp_dir();
    let sinks = TransferSinks::default();
    let proxy = spawn_broker_only(sinks.clone(), dir).await;
    proxy.activate("a".into(), None).await;
    let req_id = Uuid::now_v7();
    let mut rx = sinks.bind_forward(req_id, AckPolicy::OnReceipt);
    proxy.begin_range_active(req_id, "a.zck".into()).await.unwrap();

    sinks.fragment(req_id, 0, Bytes::from_static(b"x"));
    assert!(matches!(rx.recv().await, Some(TransferEvent::Fragment { .. })));

    proxy.deactivate().await;
    let res = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
      .await
      .expect("recv should resolve once broker unbinds");
    assert!(res.is_none(), "expected channel closed, got event: {res:?}");
  }

  async fn cached_bytes(proxy: &RangeProxy, asset: &str) -> u64 {
    let request_id = Uuid::now_v7();
    let begin = proxy
      .begin_range_active(request_id, asset.into())
      .await
      .expect("begin should succeed");
    let bytes = begin.log.index().cached_bytes();
    proxy.end_range(request_id).await;
    bytes
  }

  async fn seed(proxy: &RangeProxy, asset: &str, zck_start: u32, bytes: &[u8]) {
    let request_id = Uuid::now_v7();
    let begin = proxy
      .begin_range_active(request_id, asset.into())
      .await
      .expect("begin should succeed");
    begin.log.append(zck_start, bytes).await.unwrap();
    proxy.end_range(request_id).await;
    proxy
      .store_ranges(
        asset.into(),
        vec![RangePart {
          start: zck_start,
          length: bytes.len() as u32,
        }],
        4096,
      )
      .await;
  }

  #[tokio::test]
  async fn deactivate_keeps_the_cache_for_the_next_attempt() {
    let (proxy, _dir) = broker().await;
    proxy.activate("u1".into(), None).await;
    seed(&proxy, "a.zck", 0, b"cached").await;

    proxy.deactivate().await;
    assert!(
      proxy.load_ranges("a.zck".into()).await.is_none(),
      "closed cache reads nothing"
    );

    proxy.activate("u1".into(), None).await;
    assert_eq!(cached_bytes(&proxy, "a.zck").await, 6);
    assert!(proxy.load_ranges("a.zck".into()).await.is_some());
  }

  #[tokio::test]
  async fn activating_a_different_update_clears_the_cache() {
    let (proxy, _dir) = broker().await;
    proxy.activate("u1".into(), None).await;
    seed(&proxy, "a.zck", 0, b"cached").await;

    proxy.activate("u2".into(), None).await;
    assert_eq!(cached_bytes(&proxy, "a.zck").await, 0);
    assert!(proxy.load_ranges("a.zck".into()).await.is_none());
  }

  #[tokio::test]
  async fn abandon_clears_the_cache() {
    let (proxy, _dir) = broker().await;
    proxy.activate("u1".into(), None).await;
    seed(&proxy, "a.zck", 0, b"cached").await;

    proxy.clear_cache("u1".into()).await;
    proxy.activate("u1".into(), None).await;
    assert_eq!(cached_bytes(&proxy, "a.zck").await, 0);
  }

  #[tokio::test]
  async fn one_write_failure_keeps_the_cache_and_two_clear_it() {
    let (proxy, _dir) = broker().await;
    proxy.activate("u1".into(), None).await;
    seed(&proxy, "a.zck", 0, b"cached").await;

    proxy.note_write_failure("u1".into()).await;
    proxy.activate("u1".into(), None).await;
    assert_eq!(cached_bytes(&proxy, "a.zck").await, 6, "one failure keeps the cache");

    proxy.note_write_failure("u1".into()).await;
    proxy.activate("u1".into(), None).await;
    assert_eq!(cached_bytes(&proxy, "a.zck").await, 0, "two in a row clear it");
  }

  #[tokio::test]
  async fn a_write_failure_for_another_update_leaves_the_cache_alone() {
    let (proxy, _dir) = broker().await;
    proxy.activate("u1".into(), None).await;
    seed(&proxy, "a.zck", 0, b"cached").await;

    proxy.note_write_failure("other".into()).await;
    proxy.note_write_failure("other".into()).await;
    proxy.activate("u1".into(), None).await;
    assert_eq!(cached_bytes(&proxy, "a.zck").await, 6);
  }
}

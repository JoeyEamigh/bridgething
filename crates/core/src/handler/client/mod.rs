pub(crate) mod asset;
mod audio;
mod bluetooth;
mod capabilities;
mod config;
mod doc;
mod geo;
mod hardware;
mod library;
mod lyrics;
mod net;
mod notifications;
mod phone;
mod player;
mod stock;
mod store;
mod system;
mod time;
mod voice;
mod webapp;

use asset::*;
use audio::*;
use bluetooth::*;
use capabilities::*;
use config::*;
use doc::*;
use geo::*;
use hardware::*;
use libbridgething::{ForwardMessage, ForwardRouted, gateway::BridgeToGatewayForwardMsgEvent, wire::WireError};
use library::*;
use lyrics::*;
use net::*;
use notifications::*;
use phone::*;
use player::*;
use stock::*;
use store::*;
use system::*;
use time::*;
use voice::*;
use webapp::*;

mod handle;
pub use handle::*;

mod msg;
pub use msg::*;

use super::HandlerResult;
use crate::{
  bluetooth::BluetoothMan, handler::HandlerError, net::WSError, state::State, stock::StockInterAppSend,
  transport::TransportController,
};

pub struct ClientHandler {
  state: State,
  bluetooth: BluetoothMan,
  transport: TransportController,
}

impl ClientHandler {
  pub fn new(state: State, bluetooth: BluetoothMan, transport: TransportController) -> Self {
    Self {
      state,
      bluetooth,
      transport,
    }
  }

  pub async fn handle(&self, msg: RecvMsg) -> HandlerResult {
    let handle = MsgHandle::new(self, msg.id, msg.from, msg.stock_msg_id);

    match msg.data {
      RecvMsgData::Asset(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&AssetHandler::new(h)).await },
        );
      }
      RecvMsgData::Audio(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&AudioHandler::new(h)).await },
        );
      }
      RecvMsgData::Bluetooth(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&BluetoothHandler::new(h)).await },
        );
      }
      RecvMsgData::Capabilities(msg) => {
        dispatch(handle, move |h| async move {
          msg.dispatch(&CapabilitiesHandler::new(h)).await
        });
      }
      RecvMsgData::Config(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&ConfigHandler::new(h)).await },
        );
      }
      RecvMsgData::Doc(msg) => {
        dispatch(handle, move |h| async move { msg.dispatch(&DocHandler::new(h)).await });
      }
      RecvMsgData::Geo(msg) => {
        dispatch(handle, move |h| async move { msg.dispatch(&GeoHandler::new(h)).await });
      }
      RecvMsgData::Hardware(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&HardwareHandler::new(h)).await },
        );
      }
      RecvMsgData::Library(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&LibraryHandler::new(h)).await },
        );
      }
      RecvMsgData::Lyrics(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&LyricsHandler::new(h)).await },
        );
      }
      RecvMsgData::Net(msg) => {
        dispatch(handle, move |h| async move { msg.dispatch(&NetHandler::new(h)).await });
      }
      RecvMsgData::Notifications(msg) => {
        dispatch(handle, move |h| async move {
          msg.dispatch(&NotificationsHandler::new(h)).await
        });
      }
      RecvMsgData::Phone(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&PhoneHandler::new(h)).await },
        );
      }
      RecvMsgData::Player(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&PlayerHandler::new(h)).await },
        );
      }
      RecvMsgData::Store(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&StorageHandler::new(h)).await },
        );
      }
      RecvMsgData::System(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&SystemHandler::new(h)).await },
        );
      }
      RecvMsgData::Time(msg) => {
        dispatch(handle, move |h| async move { msg.dispatch(&TimeHandler::new(h)).await });
      }
      RecvMsgData::Voice(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&VoiceHandler::new(h)).await },
        );
      }
      RecvMsgData::Webapp(msg) => {
        dispatch(
          handle,
          move |h| async move { msg.dispatch(&WebappHandler::new(h)).await },
        );
      }
      RecvMsgData::Forward(msg) => {
        dispatch(handle, move |h| async move {
          TopLevelHandler::new(h).handle_forward(msg).await
        });
      }

      RecvMsgData::LegacyStock(msg) => {
        dispatch(
          handle,
          move |h| async move { LegacyStockHandler::new(h).handle(msg).await },
        );
      }

      RecvMsgData::Response { request_id, .. } => {
        tracing::error!(
          "({}) Response-meta message {request_id} reached the handler - listener interception is broken",
          &handle.from
        );
      }

      RecvMsgData::Hole => {
        tracing::trace!("({}) received blackhole message", &handle.from);

        if let Some(msg_id) = handle.stock_msg_id {
          handle.send_stock(StockInterAppSend::make_ack(Some(msg_id))).await?;
        }
      }
      RecvMsgData::Unsupported(msg) => {
        tracing::trace!("({}) received unsupported message: {:?}", &handle.from, msg);

        if let Some(msg_id) = handle.stock_msg_id {
          handle.send_stock(StockInterAppSend::make_ack(Some(msg_id))).await?;
        }
      }

      RecvMsgData::ChangeMode(mode) => {
        if mode == ClientMode::Stock {
          self.state.peers.resync_stock_connection().await;
        };
      }

      RecvMsgData::ConnectionClosed(code, reason) => {
        tracing::info!(
          "({}) connection closed with code {:?}, reason {}",
          &handle.id,
          code,
          reason
        );
        net::cleanup_owner_routes(&handle).await;
        geo::cleanup_owner_watchers(&handle).await;
        handle
          .state
          .log_tap
          .drain_for_owner(crate::state::log_tap::LogOwner::Client(handle.from));
      }
      RecvMsgData::Error(error) => {
        tracing::error!("({}) failed to receive message: {:?}", &handle.from, error);
        return Err(HandlerError::WS(WSError::Websocket(error)));
      }
    }

    Ok(())
  }
}

#[derive(Debug)]
struct TopLevelHandler {
  handle: MsgHandle,
}

impl TopLevelHandler {
  pub fn new(handle: MsgHandle) -> Self {
    Self { handle }
  }

  pub async fn handle_forward(&mut self, message: ForwardMessage) -> HandlerResult {
    let Some(webapp) = self.handle.state.active_webapp().await? else {
      tracing::debug!(
        "({:?}) dropping a forward with no active webapp to stamp",
        &self.handle.from
      );
      return Ok(());
    };
    tracing::debug!("({:?}) forwarding a message from webapp {webapp}", &self.handle.from);
    self
      .handle
      .bluetooth
      .gateway_man
      .broadcast(BridgeToGatewayForwardMsgEvent::Routed(ForwardRouted {
        webapp,
        message,
      }))
      .await;

    Ok(())
  }
}

fn dispatch<F, Fut>(handle: MsgHandle, work: F)
where
  F: FnOnce(MsgHandle) -> Fut + Send + 'static,
  Fut: std::future::Future<Output = HandlerResult> + Send + 'static,
{
  let err_handle = handle.clone();
  tokio::spawn(async move {
    if let Err(e) = work(handle).await {
      tracing::error!("({:?}) handler failed: {:?}", err_handle.from, e);
      let _ = err_handle
        .respond(WireError::HandlerFailed {
          reason: format!("{e:?}"),
        })
        .await;
    }
  });
}

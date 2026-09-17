mod handle;
use handle::*;

mod asset;
mod audio;
mod authority;
mod capabilities;
mod chrome;
mod forward;
mod geo;
mod library;
mod net;
mod notifications;
mod phone;
mod player;
pub use player::SpotifyWakeGate;
mod system;
mod time;
mod tunnel;
mod voice;
pub mod webapp;

use asset::*;
use audio::*;
use authority::*;
use capabilities::*;
use chrome::*;
use forward::*;
use geo::*;
use libbridgething::{
  gateway::{
    BridgeToGatewayTransferMsgEvent, GatewayToBridgeAssetMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData,
    GatewayToBridgeTransferMsg, TransferAck, TransferBody,
  },
  wire::MsgMeta,
};
use library::*;
use net::*;
use notifications::*;
use phone::*;
use player::*;
use system::*;
use time::*;
use tunnel::*;
use voice::*;
use webapp::*;

use super::HandlerResult;
use crate::{
  bluetooth::{BluetoothMan, InboundGatewayMessage},
  ota::OtaOrchestrator,
  state::State,
  transport::TransportController,
};

pub struct GatewayHandler {
  state: State,
  bluetooth: BluetoothMan,
  ota: OtaOrchestrator,
  transport: TransportController,
}

impl GatewayHandler {
  pub fn new(state: State, bluetooth: BluetoothMan, ota: OtaOrchestrator, transport: TransportController) -> Self {
    Self {
      state,
      bluetooth,
      ota,
      transport,
    }
  }

  pub async fn handle(&self, data: InboundGatewayMessage) -> HandlerResult {
    tracing::trace!(
      "handling {:?} bluetooth event from {:?}: {:?}",
      data.protocol,
      data.address,
      data.msg
    );

    let InboundGatewayMessage {
      address,
      protocol,
      msg: GatewayToBridgeMsg { id, meta, data },
    } = data;

    if let MsgMeta::Response(meta_resp) = &meta
      && self
        .bluetooth
        .gateway_man
        .complete_pending(&meta_resp.request_id, data.clone())
    {
      return Ok(());
    }

    let handle = MsgHandle::new(self, id, address, protocol);

    match data {
      GatewayToBridgeMsgData::Asset(asset_msg) => match asset_msg {
        GatewayToBridgeAssetMsg::Got(reply) => {
          tracing::debug!(id = %reply.id, "late asset reply past its request timeout; caching");
          if let TransferBody::Stream(transfer) = &reply.body {
            self.state.transfer_sinks.bind_memory(transfer.id);
          }
          let assets = self.state.assets.clone();
          let sinks = self.state.transfer_sinks.clone();
          tokio::spawn(async move { cache_late_asset(assets, sinks, reply).await });
        }
        GatewayToBridgeAssetMsg::NotFound(reply) => {
          tracing::debug!(id = %reply.id, "late asset not-found past its request timeout; ignoring");
        }
        other => {
          if other.is_event_variant() {
            let ev = other.into_event().expect("checked above");
            if let Err(err) = ev.dispatch(&AssetHandler::new(handle)).await {
              tracing::error!(?err, "asset event handler failed");
            }
          } else {
            tracing::warn!(
              "({:?}) stray response-shape arrival on Asset surface with no matching pending request; dropping",
              handle.address,
            );
          }
        }
      },
      GatewayToBridgeMsgData::Audio(audio_msg) => {
        if let Some(event) = audio_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&AudioHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Authority(auth_msg) => {
        if let Some(event) = auth_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&AuthorityHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Capabilities(cap_msg) => {
        if let Some(event) = cap_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&CapabilitiesHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Chrome(chrome_msg) => {
        if let Some(cmd) = chrome_msg.into_command() {
          tokio::spawn(async move { cmd.dispatch(&ChromeHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Forward(forward_msg) => {
        if let Some(event) = forward_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&ForwardHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Geo(geo_msg) => {
        if let Some(event) = geo_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&GeoHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Library(lib_msg) => {
        if let Some(event) = lib_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&LibraryHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Lyrics(_) => {
        tracing::warn!(
          "({:?}) stray response-shape arrival on Lyrics surface with no matching pending request; dropping",
          handle.address,
        );
      }
      GatewayToBridgeMsgData::Net(net_msg) => {
        if let Some(event) = net_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&NetHandler::new(handle)).await });
        } else {
          tracing::warn!(
            "({:?}) stray response-shape Net arrival with no matching pending request; dropping",
            handle.address,
          );
        }
      }
      GatewayToBridgeMsgData::Notifications(notif_msg) => {
        if let Some(event) = notif_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&NotificationsHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Phone(phone_msg) => {
        if let Some(event) = phone_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&PhoneHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Player(player_msg) => {
        if player_msg.is_command_variant() {
          if let Some(cmd) = player_msg.into_command() {
            tokio::spawn(async move { cmd.dispatch(&PlayerHandler::new(handle)).await });
          }
        } else if let Some(event) = player_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&PlayerHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::System(system_msg) => {
        let ota = self.ota.clone();
        if system_msg.is_request_variant() {
          let req = system_msg.into_request().expect("checked above");
          tokio::spawn(async move { req.dispatch(&SystemHandler::new(handle, ota)).await });
        } else if let Some(cmd) = system_msg.into_command() {
          tokio::spawn(async move { cmd.dispatch(&SystemHandler::new(handle, ota)).await });
        }
      }
      GatewayToBridgeMsgData::Time(time_msg) => {
        if let Some(event) = time_msg.into_event() {
          tokio::spawn(async move { event.dispatch(&TimeHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Transfer(transfer_msg) => match transfer_msg {
        GatewayToBridgeTransferMsg::Fragment(f) => {
          let transfer_id = f.transfer_id;
          let ack_received = self.state.transfer_sinks.fragment(f.transfer_id, f.offset, f.bytes);
          if let Some(received) = ack_received
            && let Some(address) = handle.address
          {
            let ack = BridgeToGatewayTransferMsgEvent::Ack(TransferAck { transfer_id, received });
            let bluetooth = handle.bluetooth.clone();
            tokio::spawn(async move { bluetooth.gateway_man.send_event(address, ack).await });
          }
        }
        GatewayToBridgeTransferMsg::Abandon(a) => {
          self.state.transfer_sinks.abandon(a.transfer_id, a.reason);
        }
        GatewayToBridgeTransferMsg::Ack(a) => {
          self.state.transfer_outbound.note_ack(a.transfer_id, a.received);
        }
      },
      GatewayToBridgeMsgData::Tunnel(tunnel_msg) => {
        if let Some(event) = tunnel_msg.into_event()
          && let Err(err) = event.dispatch(&TunnelHandler::new(handle)).await
        {
          tracing::error!(?err, "tunnel event handler failed");
        }
      }
      GatewayToBridgeMsgData::Voice(voice_msg) => {
        if let Some(cmd) = voice_msg.into_command() {
          tokio::spawn(async move { cmd.dispatch(&VoiceHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Webapp(webapp_msg) => {
        if let Some(req) = webapp_msg.into_request() {
          tokio::spawn(async move { req.dispatch(&WebappHandler::new(handle)).await });
        }
      }
      GatewayToBridgeMsgData::Error(err) => {
        tracing::warn!("({:?}) gateway reported a protocol error: {:?}", &handle.address, err);
      }
    }

    Ok(())
  }
}

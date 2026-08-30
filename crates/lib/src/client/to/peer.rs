use std::collections::HashMap;

use bridgething_macros::BridgeEnum;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Peer;

/// Every known peer, keyed by peer id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(transparent)]
#[ts(export, export_to = "client.ts")]
pub struct PeerSnapshotMap(pub HashMap<String, Peer>);

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "client.ts")]
#[bridge_enum(into = crate::client::BridgeToClientMsgData)]
/// Every device the daemon knows about, with its pairing and companion-app connection state.
pub enum BridgeToClientPeerMsg {
  #[bridge_event]
  Snapshot(PeerSnapshotMap),
}

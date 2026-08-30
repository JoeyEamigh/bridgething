use bridgething_macros::BridgeEnum;
use derive_more::derive::Debug;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
  ArtProfile, ConfigEntry, DocEntry, WebappError, WebappInfo,
  gateway::{TransferBody, WebappResourceKind},
};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappResourceReply {
  #[ts(type = "string")]
  pub id: Uuid,
  pub kind: WebappResourceKind,
  pub sha256: String,
  pub mime: Option<String>,
  pub body: Option<TransferBody>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappSlots {
  #[ts(type = "string | null")]
  pub launcher: Option<Uuid>,
  #[ts(type = "string | null")]
  pub overlay: Option<Uuid>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappConfigGetReply {
  pub key: String,
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappConfigListReply {
  pub entries: Vec<ConfigEntry>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappConfigAck {
  pub key: String,
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappDocGetReply {
  pub key: String,
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappDocListReply {
  pub entries: Vec<DocEntry>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappDocAck {
  pub key: String,
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappDocChanged {
  #[ts(type = "string")]
  pub id: Uuid,
  pub key: String,
  pub value: Option<String>,
}

/// A settings write landed on the daemon. `value` is `None` when the key was cleared.
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappConfigChanged {
  #[ts(type = "string")]
  pub id: Uuid,
  pub key: String,
  pub value: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappList {
  pub webapps: Vec<WebappInfo>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappActive {
  #[ts(type = "string | null")]
  pub id: Option<Uuid>,
  pub name: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
pub struct WebappActiveChanged {
  #[ts(type = "string | null")]
  pub id: Option<Uuid>,
  pub name: Option<String>,
  pub art: Option<ArtProfile>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, BridgeEnum)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
#[ts(export, export_to = "gateway.ts")]
#[bridge_enum(into = crate::gateway::BridgeToGatewayMsgData)]
pub enum BridgeToGatewayWebappMsg {
  #[bridge_response]
  Webapps(WebappList),
  #[bridge_response]
  Active(WebappActive),
  #[bridge_response]
  Switched(WebappActive),
  #[bridge_response]
  Uninstalled(WebappActive),
  #[bridge_response]
  WebappError(WebappError),
  #[bridge_response]
  Resource(WebappResourceReply),
  #[bridge_response]
  Slots(WebappSlots),
  #[bridge_response]
  ConfigGet(WebappConfigGetReply),
  #[bridge_response]
  ConfigList(WebappConfigListReply),
  #[bridge_response]
  ConfigAck(WebappConfigAck),
  #[bridge_response]
  DocGet(WebappDocGetReply),
  #[bridge_response]
  DocList(WebappDocListReply),
  #[bridge_response]
  DocAck(WebappDocAck),
  #[bridge_event]
  DocChanged(WebappDocChanged),
  #[bridge_event]
  ConfigChanged(WebappConfigChanged),
  #[bridge_event]
  WebappInstalled(WebappInfo),
  #[bridge_event]
  ActiveChanged(WebappActiveChanged),
}

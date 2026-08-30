use std::sync::Arc;

use libbridgething::ForwardMessage;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ExtensionMessage {
  Text { text: String },
  Json { json: String },
  Binary { bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ExtensionConfigEntry {
  pub webapp: String,
  pub key: String,
  pub value: String,
}

#[uniffi::export(with_foreign)]
pub trait ExtensionHost: Send + Sync {
  fn start(&self, inbox: Arc<ExtensionHostInbox>);
  fn stop(&self);
  fn deliver(&self, device: String, webapp: String, message: ExtensionMessage);
  fn device_connected(&self, device: String, name: String, config: Vec<ExtensionConfigEntry>, webapps: Vec<String>);
  fn device_disconnected(&self, device: String);
  fn device_active(&self, device: String, webapp: String, active: bool);
  fn config_changed(&self, device: String, webapp: String, key: String, value: Option<String>);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionOutbound {
  SendToDevice {
    device: Option<String>,
    webapp: Uuid,
    message: ForwardMessage,
  },
  RunningChanged {
    webapps: Vec<Uuid>,
  },
}

#[derive(uniffi::Object)]
pub struct ExtensionHostInbox {
  tx: mpsc::UnboundedSender<ExtensionOutbound>,
}

impl ExtensionHostInbox {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<ExtensionOutbound>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[uniffi::export]
impl ExtensionHostInbox {
  pub fn send_to_device(&self, device: Option<String>, webapp: String, message: ExtensionMessage) {
    let Some(webapp) = parse_webapp(&webapp) else { return };
    let Some(message) = message.into_forward() else { return };
    let _ = self.tx.send(ExtensionOutbound::SendToDevice {
      device,
      webapp,
      message,
    });
  }

  pub fn running_changed(&self, webapps: Vec<String>) {
    let webapps = webapps.iter().filter_map(|id| parse_webapp(id)).collect();
    let _ = self.tx.send(ExtensionOutbound::RunningChanged { webapps });
  }
}

fn parse_webapp(raw: &str) -> Option<Uuid> {
  match Uuid::parse_str(raw) {
    Ok(id) => Some(id),
    Err(_) => {
      tracing::warn!(webapp = %raw, "an extension host named a webapp that is not a uuid");
      None
    }
  }
}

impl ExtensionMessage {
  fn into_forward(self) -> Option<ForwardMessage> {
    match self {
      Self::Text { text } => Some(ForwardMessage::Text(text)),
      Self::Binary { bytes } => Some(ForwardMessage::Binary(bytes)),
      Self::Json { json } => match serde_json::from_str(&json) {
        Ok(value) => Some(ForwardMessage::Json(value)),
        Err(err) => {
          tracing::warn!(?err, "an extension host sent a json forward that is not json");
          None
        }
      },
    }
  }
}

impl From<ForwardMessage> for ExtensionMessage {
  fn from(value: ForwardMessage) -> Self {
    match value {
      ForwardMessage::Text(text) => Self::Text { text },
      ForwardMessage::Binary(bytes) => Self::Binary { bytes },
      ForwardMessage::Json(value) => Self::Json {
        json: value.to_string(),
      },
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_json_forward_survives_the_string_hop_in_both_directions() {
    let original = ForwardMessage::Json(serde_json::json!({ "a": [1, 2], "b": null }));
    let crossed: ExtensionMessage = original.clone().into();
    assert_eq!(crossed.into_forward(), Some(original));
  }

  #[test]
  fn a_json_forward_that_is_not_json_is_dropped_rather_than_guessed_at() {
    assert_eq!(
      ExtensionMessage::Json { json: "{".into() }.into_forward(),
      None,
      "a malformed document must not reach the wire as a string"
    );
  }

  #[tokio::test]
  async fn the_inbox_refuses_a_webapp_that_is_not_a_uuid() {
    let (inbox, mut rx) = ExtensionHostInbox::channel();
    inbox.send_to_device(None, "not-a-uuid".into(), ExtensionMessage::Text { text: "hi".into() });
    inbox.running_changed(vec!["also-not-a-uuid".into()]);

    let running = rx.try_recv().expect("runningChanged still lands");
    assert_eq!(running, ExtensionOutbound::RunningChanged { webapps: Vec::new() });
    assert!(rx.try_recv().is_err(), "the unaddressable send was dropped");
  }
}

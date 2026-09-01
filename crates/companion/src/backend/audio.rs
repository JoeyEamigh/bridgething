use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Copy, Default, PartialEq, uniffi::Record)]
pub struct VolumeLevel {
  pub level: f32,
  pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeakEvent {
  Started,
  Finished { ok: bool },
}

#[uniffi::export(with_foreign)]
pub trait AudioBackend: Send + Sync {
  fn speak(&self, id: String, text: String, voice: Option<String>, sink: Arc<SpeakSink>);
  fn cancel(&self, id: String);
  fn cancel_all(&self);
  fn play_earcon(&self, name: String, sink: Arc<EarconSink>);
}

#[derive(uniffi::Object)]
pub struct SpeakSink {
  tx: mpsc::UnboundedSender<SpeakEvent>,
}

impl SpeakSink {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<SpeakEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[uniffi::export]
impl SpeakSink {
  pub fn on_start(&self) {
    let _ = self.tx.send(SpeakEvent::Started);
  }

  pub fn on_finished(&self, ok: bool) {
    let _ = self.tx.send(SpeakEvent::Finished { ok });
  }
}

#[derive(uniffi::Object)]
pub struct EarconSink {
  tx: std::sync::Mutex<Option<oneshot::Sender<bool>>>,
}

impl EarconSink {
  pub fn channel() -> (Arc<Self>, oneshot::Receiver<bool>) {
    let (tx, rx) = oneshot::channel();
    (
      Arc::new(Self {
        tx: std::sync::Mutex::new(Some(tx)),
      }),
      rx,
    )
  }
}

#[uniffi::export]
impl EarconSink {
  pub fn on_finished(&self, ok: bool) {
    if let Some(tx) = self.tx.lock().unwrap().take() {
      let _ = tx.send(ok);
    }
  }
}

/// output volume on the host itself. a host that leaves this unimplemented keeps the device
/// routing volume over iAP2 HID instead of over the gateway.
#[uniffi::export(with_foreign)]
pub trait VolumeBackend: Send + Sync {
  fn start(&self, inbox: Arc<VolumeInbox>);
  fn stop(&self);
  fn snapshot(&self) -> VolumeLevel;
  fn set_volume(&self, level: f32);
  fn set_mute(&self, muted: bool);
  fn volume_up(&self);
  fn volume_down(&self);
  fn mute_toggle(&self);
}

#[derive(uniffi::Object)]
pub struct VolumeInbox {
  tx: mpsc::UnboundedSender<VolumeLevel>,
}

impl VolumeInbox {
  pub fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<VolumeLevel>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Self { tx }), rx)
  }
}

#[uniffi::export]
impl VolumeInbox {
  pub fn on_changed(&self, level: f32, muted: bool) {
    let _ = self.tx.send(VolumeLevel { level, muted });
  }
}

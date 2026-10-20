use std::{sync::Arc, time::Duration};

use bridgething_gateway::OutboundLink;
use libbridgething::{
  Priority,
  gateway::{GatewayToBridgeTransferMsgEvent, TransferFragment},
  wire::MsgMeta,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{AckWindow, FragmentSource, FragmentStream, Pacer, SendError, SourceRange};
use crate::seam::Clock;

pub struct BytesSource(Arc<Vec<u8>>);

impl BytesSource {
  pub fn new(bytes: impl Into<Arc<Vec<u8>>>) -> Self {
    Self(bytes.into())
  }

  pub fn len(&self) -> u64 {
    self.0.len() as u64
  }

  pub fn is_empty(&self) -> bool {
    self.0.is_empty()
  }

  pub fn bytes(&self) -> &[u8] {
    &self.0
  }

  pub fn whole(&self) -> [SourceRange; 1] {
    [SourceRange {
      start: 0,
      length: self.len(),
    }]
  }
}

impl FragmentSource for BytesSource {
  fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
    let Ok(offset) = usize::try_from(offset) else {
      return Ok(0);
    };
    let Some(rest) = self.0.get(offset..) else {
      return Ok(0);
    };
    let take = rest.len().min(buf.len());
    buf[..take].copy_from_slice(&rest[..take]);
    Ok(take)
  }
}

#[derive(Clone)]
pub struct LinkPush {
  link: Arc<dyn OutboundLink>,
  acks: Arc<AckWindow>,
  clock: Arc<dyn Clock>,
}

impl LinkPush {
  pub fn new(link: Arc<dyn OutboundLink>, acks: Arc<AckWindow>, clock: Arc<dyn Clock>) -> Self {
    Self { link, acks, clock }
  }

  pub fn link(&self) -> &Arc<dyn OutboundLink> {
    &self.link
  }

  pub fn acks(&self) -> &Arc<AckWindow> {
    &self.acks
  }

  pub async fn run(
    &self,
    transfer_id: Uuid,
    source: &dyn FragmentSource,
    ranges: &[SourceRange],
    resume_from: u64,
    fragment_bytes: usize,
    ack_timeout: Duration,
  ) -> Result<u64, SendError> {
    let (fragments, mut queued) = mpsc::channel::<TransferFragment>(1);
    let link = self.link.clone();

    let writing = async move {
      while let Some(fragment) = queued.recv().await {
        let sent = link
          .send_data(
            MsgMeta::Event,
            GatewayToBridgeTransferMsgEvent::Fragment(fragment).into(),
            Priority::Background,
          )
          .await;
        if sent.is_err() {
          break;
        }
      }
    };

    let mut pacer = Pacer::new(self.clock.clone(), resume_from);
    let walking = async {
      let pushed = FragmentStream {
        transfer_id,
        source,
        ranges,
        resume_from,
        fragment_bytes,
        sink: &fragments,
        acks: &self.acks,
        ack_timeout,
      }
      .run(&mut pacer)
      .await;
      drop(fragments);
      pushed
    };

    futures::future::join(walking, writing).await.0
  }
}

use crate::{Priority, protocol::EndecError};

#[derive(Debug)]
pub enum DecodedFrame<T> {
  Frame(PrioritizedFrame<T>),
  Failed(EndecError),
}

impl<T> DecodedFrame<T> {
  pub fn frame(self) -> Option<PrioritizedFrame<T>> {
    match self {
      DecodedFrame::Frame(frame) => Some(frame),
      DecodedFrame::Failed(_) => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compress {
  #[default]
  Auto,
  Never,
  Always,
  IfSmaller,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrioritizedFrame<T> {
  pub priority: Priority,
  pub compress: Compress,
  pub msg: T,
}

impl<T> PrioritizedFrame<T> {
  pub fn new(priority: Priority, msg: T) -> Self {
    Self {
      priority,
      compress: Compress::Auto,
      msg,
    }
  }

  pub fn normal(msg: T) -> Self {
    Self::new(Priority::Normal, msg)
  }

  pub fn bulk(msg: T) -> Self {
    Self::new(Priority::Bulk, msg)
  }

  pub fn compressed(mut self, compress: Compress) -> Self {
    self.compress = compress;
    self
  }

  pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> PrioritizedFrame<U> {
    PrioritizedFrame {
      priority: self.priority,
      compress: self.compress,
      msg: f(self.msg),
    }
  }
}

use std::{fmt, future::Future, time::Duration};

pub use web_time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

impl fmt::Display for Elapsed {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("deadline has elapsed")
  }
}

impl std::error::Error for Elapsed {}

pub fn now() -> Instant {
  Instant::now()
}

pub fn unix_millis() -> u64 {
  web_time::SystemTime::now()
    .duration_since(web_time::UNIX_EPOCH)
    .map(|since| since.as_millis() as u64)
    .unwrap_or(0)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F: Future<Output = ()> + Send + 'static>(fut: F) {
  tokio::spawn(fut);
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn spawn_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
  match tokio::task::spawn_blocking(work).await {
    Ok(done) => done,
    Err(joined) => std::panic::resume_unwind(joined.into_panic()),
  }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep(duration: Duration) {
  tokio::time::sleep(duration).await;
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn timeout<F: Future>(duration: Duration, fut: F) -> Result<F::Output, Elapsed> {
  tokio::time::timeout(duration, fut).await.map_err(|_| Elapsed)
}

#[cfg(target_arch = "wasm32")]
pub fn spawn<F: Future<Output = ()> + Send + 'static>(fut: F) {
  wasm_bindgen_futures::spawn_local(fut);
}

#[cfg(target_arch = "wasm32")]
pub async fn spawn_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
  work()
}

#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
  let _ = wasm_timer::schedule(duration).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn timeout<F: Future>(duration: Duration, fut: F) -> Result<F::Output, Elapsed> {
  use futures::future::{Either, select};

  let fut = std::pin::pin!(fut);
  let timer = std::pin::pin!(sleep(duration));
  match select(fut, timer).await {
    Either::Left((output, _)) => Ok(output),
    Either::Right(((), _)) => Err(Elapsed),
  }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
  use super::*;

  #[tokio::test]
  async fn blocking_work_answers_from_off_the_runtime_thread() {
    let runtime = std::thread::current().id();
    let (answer, ran_on) = spawn_blocking(move || (21 * 2, std::thread::current().id())).await;

    assert_eq!(answer, 42);
    assert_ne!(ran_on, runtime, "the work leaves the thread its caller is polled on");
  }

  #[tokio::test]
  #[should_panic(expected = "the work gave up")]
  async fn a_panic_in_blocking_work_reaches_the_caller() {
    spawn_blocking(|| panic!("the work gave up")).await;
  }
}

#[cfg(target_arch = "wasm32")]
mod wasm_timer {
  use std::time::Duration;

  use futures::channel::oneshot;
  use wasm_bindgen::prelude::*;

  #[wasm_bindgen]
  extern "C" {
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(handler: &JsValue, timeout_ms: i32);
  }

  pub fn schedule(duration: Duration) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    let callback = Closure::once_into_js(move || {
      let _ = tx.send(());
    });
    set_timeout(&callback, duration.as_millis().min(i32::MAX as u128) as i32);
    rx
  }
}

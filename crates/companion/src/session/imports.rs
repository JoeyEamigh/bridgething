use std::{collections::HashMap, sync::Mutex};

use tokio::task::AbortHandle;

use crate::api::CompanionError;

#[derive(Default)]
struct Open {
  next: u64,
  tickets: HashMap<u64, Option<AbortHandle>>,
}

#[derive(Default)]
pub struct MediaImports(Mutex<Open>);

struct Running<'a> {
  imports: &'a MediaImports,
  ticket: u64,
  task: AbortHandle,
}

impl Drop for Running<'_> {
  fn drop(&mut self) {
    self.task.abort();
    self.imports.0.lock().unwrap().tickets.remove(&self.ticket);
  }
}

impl MediaImports {
  pub fn ticket(&self) -> u64 {
    let mut open = self.0.lock().unwrap();
    open.next += 1;
    let ticket = open.next;
    open.tickets.insert(ticket, None);
    ticket
  }

  pub fn cancel(&self, ticket: u64) {
    if let Some(Some(running)) = self.0.lock().unwrap().tickets.remove(&ticket) {
      running.abort();
    }
  }

  pub fn clear(&self) {
    for running in self
      .0
      .lock()
      .unwrap()
      .tickets
      .drain()
      .filter_map(|(_, running)| running)
    {
      running.abort();
    }
  }

  pub async fn run<T: Send + 'static>(
    &self,
    ticket: u64,
    work: impl Future<Output = Result<T, CompanionError>> + Send + 'static,
  ) -> Result<T, CompanionError> {
    let (task, _running) = {
      let mut open = self.0.lock().unwrap();
      let Some(slot) = open.tickets.get_mut(&ticket) else {
        return Err(CompanionError::Cancelled);
      };
      let task = tokio::spawn(work);
      *slot = Some(task.abort_handle());
      let running = Running {
        imports: self,
        ticket,
        task: task.abort_handle(),
      };
      (task, running)
    };
    match task.await {
      Ok(outcome) => outcome,
      Err(error) if error.is_panic() => Err(CompanionError::Runtime(format!("the import panicked: {error}"))),
      Err(_) => Err(CompanionError::Cancelled),
    }
  }

  #[cfg(test)]
  fn open(&self) -> usize {
    self.0.lock().unwrap().tickets.len()
  }
}

#[cfg(test)]
mod tests {
  use std::{
    sync::{
      Arc,
      atomic::{AtomicBool, Ordering},
    },
    time::Duration,
  };

  use super::*;

  struct Dropped(Arc<AtomicBool>);

  impl Drop for Dropped {
    fn drop(&mut self) {
      self.0.store(true, Ordering::SeqCst);
    }
  }

  fn held() -> (
    Arc<AtomicBool>,
    impl Future<Output = Result<u8, CompanionError>> + Send + 'static,
  ) {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = Dropped(Arc::clone(&dropped));
    let work = async move {
      let _guard = guard;
      std::future::pending::<()>().await;
      Ok(7)
    };
    (dropped, work)
  }

  async fn started(imports: &MediaImports, ticket: u64) {
    while imports
      .0
      .lock()
      .unwrap()
      .tickets
      .get(&ticket)
      .is_some_and(Option::is_none)
    {
      tokio::time::sleep(Duration::from_millis(1)).await;
    }
  }

  async fn dropped(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
      tokio::time::sleep(Duration::from_millis(1)).await;
    }
  }

  #[tokio::test]
  async fn an_import_runs_to_its_answer_and_frees_its_ticket() {
    let imports = MediaImports::default();
    let ticket = imports.ticket();
    assert_eq!(imports.run(ticket, async { Ok(7) }).await.unwrap(), 7);
    assert_eq!(imports.open(), 0);
    imports.cancel(ticket);
    assert_eq!(imports.open(), 0, "a late cancel leaves nothing behind");
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn cancelling_a_running_import_drops_its_work() {
    let imports = Arc::new(MediaImports::default());
    let ticket = imports.ticket();
    let (dropped_work, work) = held();
    let running = tokio::spawn({
      let imports = Arc::clone(&imports);
      async move { imports.run(ticket, work).await }
    });
    started(&imports, ticket).await;

    imports.cancel(ticket);

    assert!(matches!(running.await.unwrap(), Err(CompanionError::Cancelled)));
    dropped(&dropped_work).await;
    assert_eq!(imports.open(), 0);
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn an_import_whose_caller_walks_away_is_dropped_with_it() {
    let imports = Arc::new(MediaImports::default());
    let ticket = imports.ticket();
    let (dropped_work, work) = held();
    let running = tokio::spawn({
      let imports = Arc::clone(&imports);
      async move { imports.run(ticket, work).await }
    });
    started(&imports, ticket).await;

    running.abort();

    dropped(&dropped_work).await;
    assert_eq!(imports.open(), 0);
  }

  #[tokio::test]
  async fn an_import_cancelled_before_it_starts_never_runs() {
    let imports = MediaImports::default();
    let ticket = imports.ticket();
    imports.cancel(ticket);
    let (dropped_work, work) = held();

    assert!(matches!(
      imports.run(ticket, work).await,
      Err(CompanionError::Cancelled)
    ));
    assert!(dropped_work.load(Ordering::SeqCst));
    assert_eq!(imports.open(), 0);
  }

  #[tokio::test]
  async fn a_ticket_nobody_issued_runs_nothing() {
    let imports = MediaImports::default();
    let (dropped_work, work) = held();
    assert!(matches!(imports.run(41, work).await, Err(CompanionError::Cancelled)));
    assert!(dropped_work.load(Ordering::SeqCst));
  }

  #[tokio::test]
  async fn a_panicked_import_is_an_error_not_a_cancel() {
    let imports = MediaImports::default();
    let ticket = imports.ticket();
    let outcome = imports
      .run::<u8>(ticket, async { panic!("the scaler fell over") })
      .await;
    assert!(
      matches!(&outcome, Err(CompanionError::Runtime(message)) if message.contains("panicked")),
      "{outcome:?}"
    );
    assert_eq!(imports.open(), 0);
  }

  #[tokio::test(flavor = "multi_thread")]
  async fn clearing_drops_every_ticket_and_every_running_import() {
    let imports = Arc::new(MediaImports::default());
    let unused = imports.ticket();
    let ticket = imports.ticket();
    let (dropped_work, work) = held();
    let running = tokio::spawn({
      let imports = Arc::clone(&imports);
      async move { imports.run(ticket, work).await }
    });
    started(&imports, ticket).await;

    imports.clear();

    assert!(matches!(running.await.unwrap(), Err(CompanionError::Cancelled)));
    dropped(&dropped_work).await;
    assert_eq!(imports.open(), 0);
    assert!(matches!(
      imports.run(unused, async { Ok(1) }).await,
      Err(CompanionError::Cancelled)
    ));
  }
}

use std::sync::Mutex;

use bridgething::{ResetRig, ResetSupport, RigError};
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct FakeResetRig {
  state: Mutex<RigState>,
  stops_on_reboot: Mutex<Option<CancellationToken>>,
}

#[derive(Debug, Default)]
struct RigState {
  no_wipe_step: bool,
  reboots_fail: bool,
  disarms_fail: bool,
  armed: bool,
  probes: usize,
  arms: usize,
  reboots: usize,
}

impl FakeResetRig {
  pub fn remove_wipe_step(&self) {
    self.state.lock().unwrap().no_wipe_step = true;
  }

  pub fn fail_reboots(&self) {
    self.state.lock().unwrap().reboots_fail = true;
  }

  pub fn fail_disarms(&self) {
    self.state.lock().unwrap().disarms_fail = true;
  }

  pub(crate) fn stop_on_reboot(&self, daemon: CancellationToken) {
    *self.stops_on_reboot.lock().unwrap() = Some(daemon);
  }

  pub fn leave_armed(&self) {
    self.state.lock().unwrap().armed = true;
  }

  pub fn is_armed(&self) -> bool {
    self.state.lock().unwrap().armed
  }

  pub fn probes(&self) -> usize {
    self.state.lock().unwrap().probes
  }

  pub fn arms(&self) -> usize {
    self.state.lock().unwrap().arms
  }

  pub fn reboots(&self) -> usize {
    self.state.lock().unwrap().reboots
  }
}

impl ResetRig for FakeResetRig {
  fn support(&self) -> BoxFuture<'_, Result<ResetSupport, RigError>> {
    Box::pin(async {
      let mut state = self.state.lock().unwrap();
      state.probes += 1;
      Ok(if state.no_wipe_step {
        ResetSupport::NoWipeStep
      } else {
        ResetSupport::Ready
      })
    })
  }

  fn armed(&self) -> BoxFuture<'_, Result<bool, RigError>> {
    Box::pin(async { Ok(self.is_armed()) })
  }

  fn arm(&self) -> BoxFuture<'_, Result<(), RigError>> {
    Box::pin(async {
      let mut state = self.state.lock().unwrap();
      state.arms += 1;
      state.armed = true;
      Ok(())
    })
  }

  fn disarm(&self) -> BoxFuture<'_, Result<(), RigError>> {
    Box::pin(async {
      let mut state = self.state.lock().unwrap();
      if state.disarms_fail {
        return Err("the env partition is read-only".into());
      }
      state.armed = false;
      Ok(())
    })
  }

  fn reboot(&self) -> BoxFuture<'_, Result<(), RigError>> {
    Box::pin(async {
      let mut state = self.state.lock().unwrap();
      state.reboots += 1;
      if state.reboots_fail {
        return Err("systemd refused the reboot".into());
      }
      if let Some(daemon) = self.stops_on_reboot.lock().unwrap().as_ref() {
        daemon.cancel();
      }
      Ok(())
    })
  }
}

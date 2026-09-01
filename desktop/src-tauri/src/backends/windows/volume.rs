use std::{
  ptr::null,
  sync::{
    Arc, Mutex,
    mpsc::{Receiver, Sender, channel},
  },
  thread,
};

use bridgething_companion::backend::{VolumeBackend, VolumeInbox, VolumeLevel};
use windows::{
  Win32::{
    Media::Audio::{
      AUDIO_VOLUME_NOTIFICATION_DATA, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
      IMMNotificationClient_Impl, MMDeviceEnumerator,
      Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl},
      eMultimedia, eRender,
    },
    System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize},
  },
  core::implement,
};

const STEP: f32 = 1.0 / 16.0;

#[derive(Default)]
struct Shared {
  inbox: Mutex<Option<Arc<VolumeInbox>>>,
  level: Mutex<Option<VolumeLevel>>,
}

impl Shared {
  fn publish(&self, level: VolumeLevel) {
    let changed = {
      let mut held = self.level.lock().unwrap();
      let changed = *held != Some(level);
      *held = Some(level);
      changed
    };
    if !changed {
      return;
    }
    let inbox = self.inbox.lock().unwrap().clone();
    if let Some(inbox) = inbox {
      inbox.on_changed(level.level, level.muted);
    }
  }
}

enum Task {
  Set(f32),
  Mute(bool),
  Step(f32),
  ToggleMute,
  Rebind,
  Stop,
}

#[derive(Default)]
pub struct EndpointVolume {
  shared: Arc<Shared>,
  engine: Mutex<Option<Sender<Task>>>,
}

impl EndpointVolume {
  fn send(&self, task: Task) {
    let held = self.engine.lock().unwrap();
    if let Some(engine) = held.as_ref() {
      let _ = engine.send(task);
    }
  }
}

impl VolumeBackend for EndpointVolume {
  fn start(&self, inbox: Arc<VolumeInbox>) {
    *self.shared.inbox.lock().unwrap() = Some(inbox);
    let mut held = self.engine.lock().unwrap();
    if held.is_some() {
      return;
    }
    let (tx, rx) = channel();
    let shared = Arc::clone(&self.shared);
    let wake = tx.clone();
    match thread::Builder::new()
      .name("bridgething-volume".to_owned())
      .spawn(move || run(shared, rx, wake))
    {
      Ok(_) => *held = Some(tx),
      Err(error) => tracing::warn!(%error, "the volume engine could not be started"),
    }
  }

  fn stop(&self) {
    let engine = self.engine.lock().unwrap().take();
    if let Some(engine) = engine {
      let _ = engine.send(Task::Stop);
    }
    self.shared.inbox.lock().unwrap().take();
  }

  fn snapshot(&self) -> VolumeLevel {
    self.shared.level.lock().unwrap().unwrap_or_default()
  }

  fn set_volume(&self, level: f32) {
    self.send(Task::Set(level));
  }

  fn set_mute(&self, muted: bool) {
    self.send(Task::Mute(muted));
  }

  fn volume_up(&self) {
    self.send(Task::Step(STEP));
  }

  fn volume_down(&self) {
    self.send(Task::Step(-STEP));
  }

  fn mute_toggle(&self) {
    self.send(Task::ToggleMute);
  }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeNotify(Arc<Shared>);

impl IAudioEndpointVolumeCallback_Impl for VolumeNotify_Impl {
  fn OnNotify(&self, notify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
    if notify.is_null() {
      return Ok(());
    }
    // SAFETY: the endpoint hands over a valid record for the duration of the call, and the
    // struct is packed, so it is copied out rather than referenced in place.
    let data = unsafe { notify.read_unaligned() };
    self.0.publish(VolumeLevel {
      level: data.fMasterVolume.clamp(0.0, 1.0),
      muted: data.bMuted.as_bool(),
    });
    Ok(())
  }
}

#[implement(IMMNotificationClient)]
struct DeviceNotify(Mutex<Sender<Task>>);

impl IMMNotificationClient_Impl for DeviceNotify_Impl {
  fn OnDefaultDeviceChanged(
    &self,
    flow: EDataFlow,
    role: ERole,
    _id: &windows::core::PCWSTR,
  ) -> windows::core::Result<()> {
    if flow == eRender && role == eMultimedia {
      let _ = self.0.lock().unwrap().send(Task::Rebind);
    }
    Ok(())
  }

  fn OnDeviceStateChanged(
    &self,
    _id: &windows::core::PCWSTR,
    _state: windows::Win32::Media::Audio::DEVICE_STATE,
  ) -> windows::core::Result<()> {
    Ok(())
  }

  fn OnDeviceAdded(&self, _id: &windows::core::PCWSTR) -> windows::core::Result<()> {
    Ok(())
  }

  fn OnDeviceRemoved(&self, _id: &windows::core::PCWSTR) -> windows::core::Result<()> {
    Ok(())
  }

  fn OnPropertyValueChanged(
    &self,
    _id: &windows::core::PCWSTR,
    _key: &windows::Win32::Foundation::PROPERTYKEY,
  ) -> windows::core::Result<()> {
    Ok(())
  }
}

struct Bound {
  endpoint: IAudioEndpointVolume,
  callback: IAudioEndpointVolumeCallback,
}

impl Bound {
  fn release(&self) {
    // SAFETY: the callback is the one registered against this endpoint in bind
    let _ = unsafe { self.endpoint.UnregisterControlChangeNotify(&self.callback) };
  }
}

fn run(shared: Arc<Shared>, tasks: Receiver<Task>, wake: Sender<Task>) {
  // SAFETY: paired with the CoUninitialize below, on this same thread, after every com object is dropped.
  let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
  if let Err(error) = serve(&shared, tasks, wake) {
    tracing::warn!(%error, "windows refused an audio endpoint; this desktop cannot move its own volume");
  }
  // SAFETY: paired with the CoInitializeEx above, on this same thread, after every com object is dropped.
  unsafe { CoUninitialize() };
}

fn serve(shared: &Arc<Shared>, tasks: Receiver<Task>, wake: Sender<Task>) -> windows::core::Result<()> {
  // SAFETY: the enumerator is a documented in-proc com server and this thread is initialised
  let enumerator: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
  let watcher: IMMNotificationClient = DeviceNotify(Mutex::new(wake)).into();
  // SAFETY: the watcher outlives the registration, which is dropped before this returns
  unsafe { enumerator.RegisterEndpointNotificationCallback(&watcher) }?;

  let mut bound = bind(&enumerator, shared).ok();
  while let Ok(task) = tasks.recv() {
    if matches!(task, Task::Stop) {
      break;
    }
    if matches!(task, Task::Rebind) {
      if let Some(previous) = bound.take() {
        previous.release();
      }
      bound = bind(&enumerator, shared).ok();
      continue;
    }
    let Some(held) = bound.as_ref() else { continue };
    let level = shared.level.lock().unwrap().unwrap_or_default();
    // SAFETY: the endpoint is live for as long as it is bound
    let outcome = unsafe {
      match task {
        Task::Set(wanted) => held.endpoint.SetMasterVolumeLevelScalar(wanted.clamp(0.0, 1.0), null()),
        Task::Step(delta) => held
          .endpoint
          .SetMasterVolumeLevelScalar((level.level + delta).clamp(0.0, 1.0), null()),
        Task::Mute(muted) => held.endpoint.SetMute(muted, null()),
        Task::ToggleMute => held.endpoint.SetMute(!level.muted, null()),
        Task::Rebind | Task::Stop => Ok(()),
      }
    };
    if let Err(error) = outcome {
      tracing::debug!(%error, "the audio endpoint refused a volume write");
    }
  }

  if let Some(held) = bound {
    held.release();
  }
  // SAFETY: the watcher is the one registered above
  let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&watcher) };
  Ok(())
}

fn bind(enumerator: &IMMDeviceEnumerator, shared: &Arc<Shared>) -> windows::core::Result<Bound> {
  // SAFETY: com is initialised on this thread and the enumerator is live
  let endpoint: IAudioEndpointVolume = unsafe {
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    device.Activate(CLSCTX_ALL, None)?
  };
  let callback: IAudioEndpointVolumeCallback = VolumeNotify(Arc::clone(shared)).into();
  // SAFETY: the callback is held in the returned Bound until release unregisters it
  unsafe { endpoint.RegisterControlChangeNotify(&callback) }?;

  // SAFETY: the endpoint was just activated
  let (level, muted) = unsafe { (endpoint.GetMasterVolumeLevelScalar()?, endpoint.GetMute()?) };
  shared.publish(VolumeLevel {
    level: level.clamp(0.0, 1.0),
    muted: muted.as_bool(),
  });

  Ok(Bound { endpoint, callback })
}

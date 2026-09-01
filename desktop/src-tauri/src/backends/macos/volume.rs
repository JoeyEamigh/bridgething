use std::{
  mem::{MaybeUninit, size_of},
  ptr::{NonNull, null},
  sync::{Arc, Mutex},
};

use bridgething_companion::backend::{VolumeBackend, VolumeInbox, VolumeLevel};
use objc2_core_audio::{
  AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress,
  AudioObjectRemovePropertyListener, AudioObjectSetPropertyData, kAudioDevicePropertyMute,
  kAudioDevicePropertyVolumeScalar, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
  kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};

const STEP: f32 = 1.0 / 16.0;

#[derive(Default)]
struct Output {
  id: AudioObjectID,
  // devices without a main volume element answer only on their per-channel elements
  elements: Vec<u32>,
}

#[derive(Default)]
struct Shared {
  inbox: Mutex<Option<Arc<VolumeInbox>>>,
  level: Mutex<Option<VolumeLevel>>,
  output: Mutex<Output>,
}

impl Shared {
  fn rebind(&self) {
    let Some(id) = default_output() else { return };
    let previous = {
      let output = self.output.lock().unwrap();
      if output.id == id && !output.elements.is_empty() {
        return;
      }
      output.id
    };
    if previous != 0 {
      listen(previous, kAudioDevicePropertyVolumeScalar, self, false);
      listen(previous, kAudioDevicePropertyMute, self, false);
    }
    let elements = match read::<f32>(id, kAudioDevicePropertyVolumeScalar, kAudioObjectPropertyElementMain) {
      Some(_) => vec![kAudioObjectPropertyElementMain],
      None => vec![1, 2],
    };
    *self.output.lock().unwrap() = Output { id, elements };
    listen(id, kAudioDevicePropertyVolumeScalar, self, true);
    listen(id, kAudioDevicePropertyMute, self, true);
  }

  fn refresh(&self) {
    let (id, elements) = {
      let output = self.output.lock().unwrap();
      (output.id, output.elements.clone())
    };
    if id == 0 {
      return;
    }
    let level = elements
      .iter()
      .filter_map(|element| read::<f32>(id, kAudioDevicePropertyVolumeScalar, *element))
      .fold(f32::NAN, f32::max);
    let muted = read::<u32>(id, kAudioDevicePropertyMute, kAudioObjectPropertyElementMain).unwrap_or_default() != 0;
    if level.is_nan() {
      return;
    }
    self.publish(VolumeLevel {
      level: level.clamp(0.0, 1.0),
      muted,
    });
  }

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

  fn apply(&self, level: f32) {
    let (id, elements) = {
      let output = self.output.lock().unwrap();
      (output.id, output.elements.clone())
    };
    let level = level.clamp(0.0, 1.0);
    for element in elements {
      write(id, kAudioDevicePropertyVolumeScalar, element, level);
    }
    self.refresh();
  }

  fn apply_mute(&self, muted: bool) {
    let id = self.output.lock().unwrap().id;
    write(id, kAudioDevicePropertyMute, kAudioObjectPropertyElementMain, u32::from(muted));
    self.refresh();
  }
}

#[derive(Default)]
pub struct CoreAudioVolume {
  shared: Arc<Shared>,
  held: Mutex<Option<*const Shared>>,
}

// the raw pointer is only ever handed to CoreAudio as listener context and reclaimed in stop
unsafe impl Send for CoreAudioVolume {}
unsafe impl Sync for CoreAudioVolume {}

impl VolumeBackend for CoreAudioVolume {
  fn start(&self, inbox: Arc<VolumeInbox>) {
    *self.shared.inbox.lock().unwrap() = Some(inbox);
    let mut held = self.held.lock().unwrap();
    if held.is_some() {
      return;
    }
    let context = Arc::into_raw(Arc::clone(&self.shared));
    *held = Some(context);
    listen_system(context, true);
    self.shared.rebind();
    self.shared.refresh();
  }

  fn stop(&self) {
    self.shared.inbox.lock().unwrap().take();
    let Some(context) = self.held.lock().unwrap().take() else {
      return;
    };
    listen_system(context, false);
    let output = self.shared.output.lock().unwrap().id;
    if output != 0 {
      listen(output, kAudioDevicePropertyVolumeScalar, &self.shared, false);
      listen(output, kAudioDevicePropertyMute, &self.shared, false);
    }
    // SAFETY: the pointer came from Arc::into_raw in start and every listener holding it is gone
    unsafe { drop(Arc::from_raw(context)) };
  }

  fn snapshot(&self) -> VolumeLevel {
    self.shared.level.lock().unwrap().unwrap_or_default()
  }

  fn set_volume(&self, level: f32) {
    self.shared.apply(level);
  }

  fn set_mute(&self, muted: bool) {
    self.shared.apply_mute(muted);
  }

  fn volume_up(&self) {
    let level = self.snapshot().level;
    self.shared.apply(level + STEP);
  }

  fn volume_down(&self) {
    let level = self.snapshot().level;
    self.shared.apply(level - STEP);
  }

  fn mute_toggle(&self) {
    let muted = self.snapshot().muted;
    self.shared.apply_mute(!muted);
  }
}

unsafe extern "C-unwind" fn changed(
  _id: AudioObjectID,
  _count: u32,
  _addresses: NonNull<AudioObjectPropertyAddress>,
  context: *mut std::ffi::c_void,
) -> i32 {
  // SAFETY: context is the Arc<Shared> pointer registered in start, alive until its listeners are removed
  let shared = unsafe { &*(context as *const Shared) };
  shared.rebind();
  shared.refresh();
  0
}

fn address(selector: u32, scope: u32, element: u32) -> AudioObjectPropertyAddress {
  AudioObjectPropertyAddress {
    mSelector: selector,
    mScope: scope,
    mElement: element,
  }
}

fn read<T: Copy>(id: AudioObjectID, selector: u32, element: u32) -> Option<T> {
  let mut wanted = address(selector, kAudioObjectPropertyScopeOutput, element);
  let mut size = size_of::<T>() as u32;
  let mut value = MaybeUninit::<T>::uninit();
  // SAFETY: size matches the buffer and CoreAudio writes at most that many bytes into it
  let status = unsafe {
    AudioObjectGetPropertyData(
      id,
      NonNull::from(&mut wanted),
      0,
      null(),
      NonNull::from(&mut size),
      NonNull::new(value.as_mut_ptr().cast())?,
    )
  };
  // SAFETY: a zero status means CoreAudio initialised the buffer
  (status == 0 && size as usize == size_of::<T>()).then(|| unsafe { value.assume_init() })
}

fn write<T: Copy>(id: AudioObjectID, selector: u32, element: u32, mut value: T) {
  if id == 0 {
    return;
  }
  let mut wanted = address(selector, kAudioObjectPropertyScopeOutput, element);
  // SAFETY: size matches the value handed over
  let status = unsafe {
    AudioObjectSetPropertyData(
      id,
      NonNull::from(&mut wanted),
      0,
      null(),
      size_of::<T>() as u32,
      NonNull::from(&mut value).cast(),
    )
  };
  if status != 0 {
    tracing::debug!(status, selector, element, "core audio refused a volume write");
  }
}

fn default_output() -> Option<AudioObjectID> {
  read_global::<AudioObjectID>(
    kAudioObjectSystemObject as AudioObjectID,
    kAudioHardwarePropertyDefaultOutputDevice,
  )
  .filter(|id| *id != 0)
}

fn read_global<T: Copy>(id: AudioObjectID, selector: u32) -> Option<T> {
  let mut wanted = address(selector, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain);
  let mut size = size_of::<T>() as u32;
  let mut value = MaybeUninit::<T>::uninit();
  // SAFETY: size matches the buffer and CoreAudio writes at most that many bytes into it
  let status = unsafe {
    AudioObjectGetPropertyData(
      id,
      NonNull::from(&mut wanted),
      0,
      null(),
      NonNull::from(&mut size),
      NonNull::new(value.as_mut_ptr().cast())?,
    )
  };
  // SAFETY: a zero status means CoreAudio initialised the buffer
  (status == 0).then(|| unsafe { value.assume_init() })
}

fn listen(id: AudioObjectID, selector: u32, shared: &Shared, add: bool) {
  let mut wanted = address(selector, kAudioObjectPropertyScopeOutput, kAudioObjectPropertyElementMain);
  let context = (shared as *const Shared).cast_mut().cast();
  // SAFETY: the listener and its context outlive the registration, which stop removes
  unsafe {
    if add {
      AudioObjectAddPropertyListener(id, NonNull::from(&mut wanted), Some(changed), context);
    } else {
      AudioObjectRemovePropertyListener(id, NonNull::from(&mut wanted), Some(changed), context);
    }
  }
}

fn listen_system(context: *const Shared, add: bool) {
  let mut wanted = address(
    kAudioHardwarePropertyDefaultOutputDevice,
    kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyElementMain,
  );
  let id = kAudioObjectSystemObject as AudioObjectID;
  let context = context.cast_mut().cast();
  // SAFETY: the listener and its context outlive the registration, which stop removes
  unsafe {
    if add {
      AudioObjectAddPropertyListener(id, NonNull::from(&mut wanted), Some(changed), context);
    } else {
      AudioObjectRemovePropertyListener(id, NonNull::from(&mut wanted), Some(changed), context);
    }
  }
}

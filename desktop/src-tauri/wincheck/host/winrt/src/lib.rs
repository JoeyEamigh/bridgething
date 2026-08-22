#![allow(non_snake_case, non_upper_case_globals)]

pub mod core {
  use std::ffi::c_void;

  #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
  pub struct HRESULT(pub i32);

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub struct Error(pub String);

  impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "{}", self.0)
    }
  }

  pub type Result<T> = std::result::Result<T, Error>;

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub struct HSTRING(pub String);

  impl std::fmt::Display for HSTRING {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      write!(f, "{}", self.0)
    }
  }

  pub trait RuntimeType: Clone + 'static {}

  impl RuntimeType for bool {}
  impl RuntimeType for u32 {}
  impl RuntimeType for f64 {}
  impl RuntimeType for HSTRING {}

  pub trait Interface: Sized {
    fn from_raw(raw: *mut c_void) -> Self;
    fn as_raw(&self) -> *mut c_void;

    fn cast<T: Interface>(&self) -> Result<T> {
      Ok(T::from_raw(self.as_raw()))
    }
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub struct IUnknown(*mut c_void);

  unsafe impl Send for IUnknown {}
  unsafe impl Sync for IUnknown {}

  impl Interface for IUnknown {
    fn from_raw(raw: *mut c_void) -> Self {
      Self(raw)
    }

    fn as_raw(&self) -> *mut c_void {
      self.0
    }
  }

  impl RuntimeType for IUnknown {}

  pub struct Ref<'a, T>(pub &'a T);
}

macro_rules! runtime_class {
  ($name:ident) => {
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct $name(crate::core::IUnknown);

    unsafe impl Send for $name {}
    unsafe impl Sync for $name {}

    impl crate::core::Interface for $name {
      fn from_raw(raw: *mut std::ffi::c_void) -> Self {
        Self(crate::core::IUnknown::from_raw(raw))
      }

      fn as_raw(&self) -> *mut std::ffi::c_void {
        crate::core::Interface::as_raw(&self.0)
      }
    }

    impl crate::core::RuntimeType for $name {}
  };
}

pub mod Foundation {
  use crate::core::{Ref, Result, RuntimeType};

  #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
  pub struct TimeSpan {
    pub Duration: i64,
  }

  #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
  pub struct DateTime {
    pub UniversalTime: i64,
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub struct TypedEventHandler<TSender, TResult>(std::marker::PhantomData<(TSender, TResult)>)
  where
    TSender: RuntimeType,
    TResult: RuntimeType;

  impl<TSender: RuntimeType, TResult: RuntimeType> TypedEventHandler<TSender, TResult> {
    pub fn new<F: Fn(Ref<'_, TSender>, Ref<'_, TResult>) -> Result<()> + Send + 'static>(_invoke: F) -> Self {
      Self(std::marker::PhantomData)
    }
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub struct IAsyncOperation<TResult: RuntimeType>(std::marker::PhantomData<TResult>);

  impl<TResult: RuntimeType> IAsyncOperation<TResult> {
    pub fn join(&self) -> Result<TResult> {
      unimplemented!("the darwin lane never reaches a winrt call")
    }
  }

  #[derive(Clone, Debug, Eq, PartialEq)]
  pub struct IReference<T: RuntimeType>(std::marker::PhantomData<T>);

  impl<T: RuntimeType> IReference<T> {
    pub fn Value(&self) -> Result<T> {
      unimplemented!("the darwin lane never reaches a winrt call")
    }
  }

  pub mod Collections {
    use crate::core::{Result, RuntimeType};

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct IVectorView<T: RuntimeType>(std::marker::PhantomData<T>);

    impl<T: RuntimeType> IVectorView<T> {
      pub fn Size(&self) -> Result<u32> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn GetAt(&self, _index: u32) -> Result<T> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }
  }
}

pub mod Media {
  use crate::core::RuntimeType;

  #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
  pub struct MediaPlaybackAutoRepeatMode(pub i32);

  impl MediaPlaybackAutoRepeatMode {
    pub const List: Self = Self(2);
    pub const None: Self = Self(0);
    pub const Track: Self = Self(1);
  }

  impl RuntimeType for MediaPlaybackAutoRepeatMode {}

  pub mod Control {
    use crate::{
      Foundation::{Collections::IVectorView, DateTime, IAsyncOperation, IReference, TimeSpan, TypedEventHandler},
      Media::MediaPlaybackAutoRepeatMode,
      Storage::Streams::IRandomAccessStreamReference,
      core::{HSTRING, Result, RuntimeType},
    };

    runtime_class!(GlobalSystemMediaTransportControlsSessionManager);
    runtime_class!(GlobalSystemMediaTransportControlsSession);
    runtime_class!(GlobalSystemMediaTransportControlsSessionMediaProperties);
    runtime_class!(GlobalSystemMediaTransportControlsSessionPlaybackInfo);
    runtime_class!(GlobalSystemMediaTransportControlsSessionPlaybackControls);
    runtime_class!(GlobalSystemMediaTransportControlsSessionTimelineProperties);
    runtime_class!(SessionsChangedEventArgs);
    runtime_class!(CurrentSessionChangedEventArgs);
    runtime_class!(MediaPropertiesChangedEventArgs);
    runtime_class!(PlaybackInfoChangedEventArgs);
    runtime_class!(TimelinePropertiesChangedEventArgs);

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct GlobalSystemMediaTransportControlsSessionPlaybackStatus(pub i32);

    impl GlobalSystemMediaTransportControlsSessionPlaybackStatus {
      pub const Changing: Self = Self(2);
      pub const Closed: Self = Self(0);
      pub const Opened: Self = Self(1);
      pub const Paused: Self = Self(5);
      pub const Playing: Self = Self(4);
      pub const Stopped: Self = Self(3);
    }

    impl RuntimeType for GlobalSystemMediaTransportControlsSessionPlaybackStatus {}

    impl GlobalSystemMediaTransportControlsSessionManager {
      pub fn RequestAsync() -> Result<IAsyncOperation<Self>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn GetSessions(&self) -> Result<IVectorView<GlobalSystemMediaTransportControlsSession>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn SessionsChanged(&self, _handler: &TypedEventHandler<Self, SessionsChangedEventArgs>) -> Result<i64> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn RemoveSessionsChanged(&self, _token: i64) -> Result<()> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn CurrentSessionChanged(
        &self,
        _handler: &TypedEventHandler<Self, CurrentSessionChangedEventArgs>,
      ) -> Result<i64> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn RemoveCurrentSessionChanged(&self, _token: i64) -> Result<()> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl GlobalSystemMediaTransportControlsSession {
      pub fn SourceAppUserModelId(&self) -> Result<HSTRING> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn MediaPropertiesChanged(
        &self,
        _handler: &TypedEventHandler<Self, MediaPropertiesChangedEventArgs>,
      ) -> Result<i64> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn RemoveMediaPropertiesChanged(&self, _token: i64) -> Result<()> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn PlaybackInfoChanged(
        &self,
        _handler: &TypedEventHandler<Self, PlaybackInfoChangedEventArgs>,
      ) -> Result<i64> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn RemovePlaybackInfoChanged(&self, _token: i64) -> Result<()> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TimelinePropertiesChanged(
        &self,
        _handler: &TypedEventHandler<Self, TimelinePropertiesChangedEventArgs>,
      ) -> Result<i64> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn RemoveTimelinePropertiesChanged(&self, _token: i64) -> Result<()> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryGetMediaPropertiesAsync(
        &self,
      ) -> Result<IAsyncOperation<GlobalSystemMediaTransportControlsSessionMediaProperties>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn GetPlaybackInfo(&self) -> Result<GlobalSystemMediaTransportControlsSessionPlaybackInfo> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn GetTimelineProperties(&self) -> Result<GlobalSystemMediaTransportControlsSessionTimelineProperties> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryPlayAsync(&self) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryPauseAsync(&self) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TrySkipNextAsync(&self) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TrySkipPreviousAsync(&self) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryChangePlaybackPositionAsync(&self, _ticks: i64) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryChangeShuffleActiveAsync(&self, _on: bool) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryChangeAutoRepeatModeAsync(&self, _mode: MediaPlaybackAutoRepeatMode) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn TryChangePlaybackRateAsync(&self, _rate: f64) -> Result<IAsyncOperation<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl GlobalSystemMediaTransportControlsSessionMediaProperties {
      pub fn Title(&self) -> Result<HSTRING> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn Artist(&self) -> Result<HSTRING> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn AlbumArtist(&self) -> Result<HSTRING> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn AlbumTitle(&self) -> Result<HSTRING> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn Thumbnail(&self) -> Result<IRandomAccessStreamReference> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl GlobalSystemMediaTransportControlsSessionPlaybackInfo {
      pub fn PlaybackStatus(&self) -> Result<GlobalSystemMediaTransportControlsSessionPlaybackStatus> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn Controls(&self) -> Result<GlobalSystemMediaTransportControlsSessionPlaybackControls> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn IsShuffleActive(&self) -> Result<IReference<bool>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn AutoRepeatMode(&self) -> Result<IReference<MediaPlaybackAutoRepeatMode>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn PlaybackRate(&self) -> Result<IReference<f64>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl GlobalSystemMediaTransportControlsSessionPlaybackControls {
      pub fn IsPlaybackPositionEnabled(&self) -> Result<bool> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl GlobalSystemMediaTransportControlsSessionTimelineProperties {
      pub fn StartTime(&self) -> Result<TimeSpan> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn EndTime(&self) -> Result<TimeSpan> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn Position(&self) -> Result<TimeSpan> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn LastUpdatedTime(&self) -> Result<DateTime> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }
  }
}

pub mod Storage {
  pub mod Streams {
    use crate::{
      Foundation::IAsyncOperation,
      core::{HSTRING, Result},
    };

    runtime_class!(IRandomAccessStreamReference);
    runtime_class!(IRandomAccessStreamWithContentType);
    runtime_class!(IInputStream);
    runtime_class!(DataReader);

    impl IRandomAccessStreamReference {
      pub fn OpenReadAsync(&self) -> Result<IAsyncOperation<IRandomAccessStreamWithContentType>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl IRandomAccessStreamWithContentType {
      pub fn ContentType(&self) -> Result<HSTRING> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn Size(&self) -> Result<u64> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn GetInputStreamAt(&self, _position: u64) -> Result<IInputStream> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }

    impl DataReader {
      pub fn CreateDataReader(_stream: &IInputStream) -> Result<Self> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn LoadAsync(&self, _count: u32) -> Result<IAsyncOperation<u32>> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }

      pub fn ReadBytes(&self, _value: &mut [u8]) -> Result<()> {
        unimplemented!("the darwin lane never reaches a winrt call")
      }
    }
  }
}

pub mod Win32 {
  pub mod System {
    pub mod Com {
      use std::ffi::c_void;

      use crate::core::HRESULT;

      #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
      pub struct COINIT(pub i32);

      pub const COINIT_MULTITHREADED: COINIT = COINIT(0);

      pub unsafe fn CoInitializeEx(_reserved: Option<*const c_void>, _flags: COINIT) -> HRESULT {
        HRESULT(0)
      }

      pub unsafe fn CoUninitialize() {}
    }
  }
}

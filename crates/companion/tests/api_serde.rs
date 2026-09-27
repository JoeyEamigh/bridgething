use std::collections::BTreeSet;

use bridgething_companion::{
  api::{
    AncsAuthStatus, AuthKind, AuthState, CapabilityFlags, DeviceLogLine, LogOrigin, NowPlaying, NowPlayingPlayback,
    NowPlayingTrack, OtaPollConfig, PeerLinkStatus, ProviderInfo, RepeatMode, ServiceHealth, ServiceHealthKind,
    SessionEvent, SessionHostInfo, SessionPeer, SessionSnapshot, SignInMethod, VoiceModelState, VoiceModelStatus,
    ota::{
      OtaApplyPhase, OtaAvailable, OtaKind, OtaPhaseSnapshot, OtaPlanStep, OtaPollEvent, OtaPollStatus, OtaRun,
      OtaRunPhase, OtaStepKind, OtaStoreChange,
    },
  },
  backend::LogLevel,
};

fn collect_keys(value: &serde_json::Value, into: &mut BTreeSet<String>) {
  match value {
    serde_json::Value::Object(fields) => {
      for (key, nested) in fields {
        into.insert(key.clone());
        collect_keys(nested, into);
      }
    }
    serde_json::Value::Array(items) => {
      for item in items {
        collect_keys(item, into);
      }
    }
    _ => {}
  }
}

fn keys_of<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
  let mut keys = BTreeSet::new();
  collect_keys(
    &serde_json::to_value(value).expect("the api type serializes"),
    &mut keys,
  );
  keys
}

fn round_trip<T>(value: &T) -> T
where
  T: serde::Serialize + serde::de::DeserializeOwned,
{
  serde_json::from_str(&serde_json::to_string(value).expect("the api type serializes"))
    .expect("the api type deserializes from its own output")
}

fn snapshot() -> SessionSnapshot {
  SessionSnapshot {
    host_info: SessionHostInfo {
      app_name: "companion".into(),
      app_version: "0.8.0".into(),
      os_name: "macos".into(),
      os_version: "26.0".into(),
      host_identifier: "host".into(),
      lib_version: "0.8.0".into(),
      libbridgething_version: "0.8.0".into(),
      adapter_version: "0.8.0".into(),
    },
    providers: vec![ProviderInfo {
      id: "spotify".into(),
      display_name: "Spotify".into(),
      available: true,
      connected: false,
      sign_in: SignInMethod::Handshake,
      auth_state: AuthState {
        kind: AuthKind::Pending,
        user_code: Some("ABCD".into()),
        verification_url: None,
        verification_url_complete: None,
        message: None,
      },
      service_health: ServiceHealth {
        kind: ServiceHealthKind::RateLimited,
        retry_after_seconds: Some(30),
      },
    }],
    provider_priority: vec!["spotify".into()],
    library_provider: Some("spotify".into()),
    peers: vec![SessionPeer {
      id: "peer".into(),
      name: "Car Thing".into(),
      status: PeerLinkStatus::LinkFailed,
      link_error: Some("no route".into()),
    }],
    ancs_auth_statuses: vec![],
    now_playing: Some(NowPlaying {
      track: Some(NowPlayingTrack {
        id: Some("track".into()),
        title: Some("Title".into()),
        artist: None,
        album: None,
        artwork_url: None,
        duration_ms: Some(240_000),
      }),
      playback: NowPlayingPlayback {
        playing: true,
        position_ms: 12_345,
        shuffle: false,
        repeat_mode: RepeatMode::One,
      },
      app_name: Some("Spotify".into()),
    }),
    device_meta: vec![],
    capability_flags: CapabilityFlags {
      geo: true,
      notifications: false,
      net_fetch: true,
      net_ws: false,
      audio_tts: true,
      voice_model: false,
    },
    voice_model: VoiceModelState {
      status: VoiceModelStatus::Downloading,
      received_bytes: 512,
      total_bytes: 4096,
      version: Some("1".into()),
      error: None,
    },
    ota_poll_config: Some(OtaPollConfig {
      interval_seconds: 900,
      auto_push: true,
      root_url: None,
    }),
    webapps: vec![],
    ota_runs: vec![run()],
    ota_available: vec![OtaAvailable {
      device_id: "device".into(),
      release_version: Some("1.2.3".into()),
      daemon_version: Some("0.8.0".into()),
      image_version: Some("0.8.0".into()),
    }],
    ota_poll: OtaPollStatus {
      last_polled_at: Some("2026-08-09T00:00:00Z".into()),
      error: None,
    },
  }
}

fn run() -> OtaRun {
  OtaRun {
    run_id: "run".into(),
    device_id: "device".into(),
    kind: OtaKind::Image,
    phase: OtaRunPhase::Streaming,
    steps: vec![OtaPlanStep {
      id: 1,
      kind: OtaStepKind::Download,
      label: "image".into(),
      bytes: 4096,
    }],
    step_id: 1,
    started_at_ms: 1_754_700_000_000,
    phase_started_at_ms: 1_754_700_001_000,
    stage_received: Some(512),
    stage_total: Some(4096),
    rate_per_sec: Some(128.0),
    dwl_percent: Some(12),
    outcome: None,
    error: None,
    release_version: Some("1.2.3".into()),
    daemon_version: Some("0.8.0".into()),
    image_version: Some("0.8.0".into()),
    channel: Some("stable".into()),
    root_url: Some("https://ota.bridgething.com".into()),
    resumable: false,
    webapp_id: None,
    webapp_name: None,
  }
}

#[test]
fn a_session_snapshot_survives_a_round_trip_through_json() {
  let snapshot = snapshot();

  assert_eq!(round_trip(&snapshot), snapshot);
}

#[test]
fn a_session_snapshot_serializes_its_field_names_as_camel_case() {
  let keys = keys_of(&snapshot());

  for expected in [
    "hostInfo",
    "libbridgethingVersion",
    "providerPriority",
    "libraryProvider",
    "ancsAuthStatuses",
    "nowPlaying",
    "capabilityFlags",
    "netFetch",
    "voiceModel",
    "otaPollConfig",
    "intervalSeconds",
    "autoPush",
    "otaRuns",
    "otaAvailable",
    "otaPoll",
    "lastPolledAt",
    "artworkUrl",
    "durationMs",
    "positionMs",
    "repeatMode",
    "retryAfterSeconds",
    "verificationUrlComplete",
    "linkError",
  ] {
    assert!(keys.contains(expected), "snapshot lost the key {expected}: {keys:?}");
  }
}

#[test]
fn unit_enum_variants_serialize_as_camel_case_strings() {
  let json = serde_json::to_string(&snapshot()).expect("the snapshot serializes");

  assert!(json.contains(r#""linkFailed""#), "PeerLinkStatus::LinkFailed: {json}");
  assert!(
    json.contains(r#""rateLimited""#),
    "ServiceHealthKind::RateLimited: {json}"
  );
  assert!(
    json.contains(r#""downloading""#),
    "VoiceModelStatus::Downloading: {json}"
  );
}

#[test]
fn an_event_tags_its_variant_and_its_payload_fields_in_camel_case() {
  let event = SessionEvent::WebappDocChanged {
    device_id: "device".into(),
    webapp_id: "webapp".into(),
    key: "theme".into(),
    value: None,
  };

  let keys = keys_of(&event);
  assert!(keys.contains("webappDocChanged"), "variant tag: {keys:?}");
  assert!(keys.contains("deviceId"), "payload field: {keys:?}");
  assert!(keys.contains("webappId"), "payload field: {keys:?}");
  assert_eq!(round_trip(&event), event);
}

#[test]
fn an_event_carrying_a_backend_type_round_trips_with_it() {
  let event = SessionEvent::Log {
    origin: LogOrigin::Device,
    level: LogLevel::Warn,
    target: "bridgething".into(),
    message: "hello".into(),
  };

  let json = serde_json::to_string(&event).expect("the event serializes");
  assert!(json.contains(r#""warn""#), "LogLevel::Warn: {json}");
  assert!(json.contains(r#""device""#), "LogOrigin::Device: {json}");
  assert_eq!(round_trip(&event), event);

  let line = DeviceLogLine {
    seq: 7,
    ts_unix_ms: 1_754_700_000_000,
    origin: LogOrigin::Host,
    level: LogLevel::Error,
    target: "bridgething".into(),
    message: "hello".into(),
  };
  let keys = keys_of(&line);
  assert!(keys.contains("tsUnixMs"), "DeviceLogLine field: {keys:?}");
  assert_eq!(round_trip(&line), line);
}

#[test]
fn a_phase_snapshot_keeps_its_variant_payload_fields_camel_case() {
  let streaming = OtaPhaseSnapshot::Streaming {
    asset: "image.swu".into(),
    sent: 512,
    total: 4096,
    rate_per_sec: Some(128.0),
    eta_seconds: Some(28.0),
  };

  let keys = keys_of(&streaming);
  assert!(keys.contains("streaming"), "variant tag: {keys:?}");
  assert!(keys.contains("ratePerSec"), "payload field: {keys:?}");
  assert!(keys.contains("etaSeconds"), "payload field: {keys:?}");
  assert_eq!(round_trip(&streaming), streaming);

  let applying = OtaPhaseSnapshot::Applying {
    phase: OtaApplyPhase::Confirming,
    write_percent: 40,
    dwl_percent: 90,
    dwl_bytes: 4096,
  };
  let keys = keys_of(&applying);
  assert!(keys.contains("writePercent"), "payload field: {keys:?}");
  assert!(keys.contains("dwlBytes"), "payload field: {keys:?}");
  assert_eq!(round_trip(&applying), applying);
}

#[test]
fn no_serialized_key_on_the_api_surface_is_snake_case() {
  let mut keys = BTreeSet::new();
  keys.extend(keys_of(&snapshot()));
  keys.extend(keys_of(&SessionEvent::PeerDisconnected {
    device_id: "device".into(),
  }));
  keys.extend(keys_of(&SessionEvent::NowPlayingChanged { now_playing: None }));
  keys.extend(keys_of(&SessionEvent::AncsAuthStatusChanged {
    device_id: "device".into(),
    status: AncsAuthStatus::Authorized,
  }));
  keys.extend(keys_of(&SessionEvent::OtaRunChanged { run: run() }));
  keys.extend(keys_of(&OtaPollEvent::UpdateAvailable {
    device_id: "device".into(),
    release: "1.2.3".into(),
    daemon_version: "0.8.0".into(),
    image_version: "0.8.0".into(),
  }));
  keys.extend(keys_of(&OtaPollEvent::Progress {
    device_id: "device".into(),
    kind: OtaKind::Daemon,
    step_id: 1,
    snapshot: OtaPhaseSnapshot::Failed { reason: "nope".into() },
  }));
  keys.extend(keys_of(&OtaStoreChange::Run { run: Box::new(run()) }));

  let offenders: Vec<&String> = keys.iter().filter(|key| key.contains('_')).collect();
  assert!(
    offenders.is_empty(),
    "these keys reached the wire in snake_case, so an enum is missing rename_all_fields or a struct is missing \
     rename_all (map keys in this fixture are chosen underscore-free on purpose): {offenders:?}"
  );
}

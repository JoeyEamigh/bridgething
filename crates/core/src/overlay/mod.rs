use std::{collections::BTreeMap, sync::Arc};

use libbridgething::OverlayProfile;
use uuid::Uuid;

const OVERLAY_JS: &str = include_str!("overlay.js");
const GEO_JS: &str = include_str!("geo.js");

pub fn kiosk_origin(modern_port: u16) -> String {
  format!("http://127.0.0.1:{modern_port}")
}

/// The companion-app config of the webapp designated as the system overlay.
/// BTreeMap keeps the serialized key order deterministic.
pub struct OverlayAppConfig {
  pub id: Uuid,
  pub config: BTreeMap<String, String>,
}

pub fn injected_script(
  profile: &OverlayProfile,
  modern_port: u16,
  body: Option<&str>,
  geo_permitted: bool,
  overlay_app: Option<&OverlayAppConfig>,
) -> Option<Arc<String>> {
  let mut segments: Vec<String> = Vec::new();

  if profile.any_enabled() {
    let (app_id, app_config) = match overlay_app {
      Some(app) => (
        serde_json::Value::String(app.id.to_string()),
        serde_json::to_value(&app.config).unwrap_or_else(|_| serde_json::json!({})),
      ),
      None => (serde_json::Value::Null, serde_json::json!({})),
    };
    let config = serde_json::json!({
      "origin": kiosk_origin(modern_port),
      "appId": app_id,
      "config": app_config,
      "surfaces": {
        "notifications": profile.notifications,
        "call": profile.call,
        "pairing": profile.pairing,
        "connection": profile.connection,
        "volume": profile.volume,
        "voice": profile.voice,
      },
    });
    segments.push(format!(
      "window.__bridgethingOverlay = {config};\n{}",
      body.unwrap_or(OVERLAY_JS)
    ));
  }

  if geo_permitted {
    let config = serde_json::json!({ "origin": kiosk_origin(modern_port) });
    segments.push(format!("window.__bridgethingGeo = {config};\n{GEO_JS}"));
  }

  if segments.is_empty() {
    return None;
  }
  Some(Arc::new(segments.join("\n")))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn all_off_produces_no_script() {
    let off = OverlayProfile {
      notifications: false,
      call: false,
      pairing: false,
      connection: false,
      volume: false,
      voice: false,
    };
    assert!(injected_script(&off, 8891, None, false, None).is_none());
  }

  #[test]
  fn default_profile_injects_every_surface() {
    let script = injected_script(&OverlayProfile::default(), 8891, None, false, None).expect("script");
    assert!(script.starts_with("window.__bridgethingOverlay = "));
    for surface in ["notifications", "call", "pairing", "connection", "volume", "voice"] {
      assert!(script.contains(&format!("\"{surface}\":true")), "{surface} on");
    }
    assert!(script.contains(&kiosk_origin(8891)));
  }

  #[test]
  fn partial_profile_carries_per_surface_flags() {
    let profile = OverlayProfile {
      notifications: true,
      call: false,
      pairing: true,
      connection: false,
      volume: false,
      voice: false,
    };
    let script = injected_script(&profile, 8891, None, false, None).expect("script");
    assert!(script.contains("\"notifications\":true"));
    assert!(script.contains("\"call\":false"));
    assert!(script.contains("\"pairing\":true"));
    assert!(script.contains("\"connection\":false"));
  }

  #[test]
  fn custom_body_replaces_the_builtin_under_the_same_prelude() {
    let profile = OverlayProfile::default();
    let script = injected_script(&profile, 8891, Some("/* mine */"), false, None).expect("script");
    assert!(script.starts_with("window.__bridgethingOverlay = "));
    assert!(script.contains(&kiosk_origin(8891)));
    assert!(script.ends_with("/* mine */"));
    assert!(!script.contains("__bridgethingOverlayMounted"));
  }

  #[test]
  fn a_custom_body_still_honors_the_all_off_short_circuit() {
    let off = OverlayProfile {
      notifications: false,
      call: false,
      pairing: false,
      connection: false,
      volume: false,
      voice: false,
    };
    assert!(injected_script(&off, 8891, Some("/* mine */"), false, None).is_none());
  }

  fn all_off() -> OverlayProfile {
    OverlayProfile {
      notifications: false,
      call: false,
      pairing: false,
      connection: false,
      volume: false,
      voice: false,
    }
  }

  #[test]
  fn geo_alone_still_injects_with_every_overlay_surface_off() {
    let script = injected_script(&all_off(), 8891, None, true, None).expect("script");
    assert!(script.contains("window.__bridgethingGeo = "));
    assert!(!script.contains("window.__bridgethingOverlay = "));
    assert!(script.contains(&kiosk_origin(8891)));
  }

  #[test]
  fn geo_is_absent_when_the_webapp_never_declared_it() {
    let script = injected_script(&OverlayProfile::default(), 8891, None, false, None).expect("script");
    assert!(!script.contains("__bridgethingGeo"));
  }

  #[test]
  fn a_custom_overlay_body_cannot_displace_the_geo_bridge() {
    let script = injected_script(&OverlayProfile::default(), 8891, Some("/* mine */"), true, None).expect("script");
    assert!(script.contains("/* mine */"), "custom body still applies");
    assert!(
      script.contains("window.__bridgethingGeo = "),
      "substituting an overlay must not take navigator.geolocation with it"
    );
  }

  #[test]
  fn nothing_declared_and_nothing_enabled_injects_nothing() {
    assert!(injected_script(&all_off(), 8891, None, false, None).is_none());
  }

  #[test]
  fn overlay_app_config_is_embedded_in_the_prelude() {
    let app = OverlayAppConfig {
      id: Uuid::parse_str("01a0984d-3d32-7e4d-b3e3-74de735362cf").unwrap(),
      config: BTreeMap::from([
        ("weather_units".to_string(), "metric".to_string()),
        ("weather_location".to_string(), "Paris".to_string()),
      ]),
    };
    let script = injected_script(&OverlayProfile::default(), 8891, None, false, Some(&app)).expect("script");
    assert!(script.starts_with("window.__bridgethingOverlay = "));
    assert!(script.contains("\"appId\":\"01a0984d-3d32-7e4d-b3e3-74de735362cf\""));
    assert!(script.contains("\"config\":{\"weather_location\":\"Paris\",\"weather_units\":\"metric\"}"));
  }

  #[test]
  fn no_overlay_app_yields_null_app_id_and_empty_config() {
    let script = injected_script(&OverlayProfile::default(), 8891, None, false, None).expect("script");
    assert!(script.contains("\"appId\":null"));
    assert!(script.contains("\"config\":{}"));
  }
}

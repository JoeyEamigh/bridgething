use std::sync::Arc;

use libbridgething::OverlayProfile;

const OVERLAY_JS: &str = include_str!("overlay.js");
const GEO_JS: &str = include_str!("geo.js");

pub const OVERLAY_WORLD: &str = "bridgething-overlay";

pub fn kiosk_origin(modern_port: u16) -> String {
  format!("http://127.0.0.1:{modern_port}")
}

pub fn overlay_script(profile: &OverlayProfile, modern_port: u16, body: Option<&str>) -> Option<Arc<String>> {
  if !profile.any_enabled() {
    return None;
  }

  let config = serde_json::json!({
    "origin": kiosk_origin(modern_port),
    "url": format!("ws://127.0.0.1:{modern_port}/?scope=overlay"),
    "surfaces": {
      "notifications": profile.notifications,
      "call": profile.call,
      "pairing": profile.pairing,
      "connection": profile.connection,
      "volume": profile.volume,
      "voice": profile.voice,
    },
  });

  Some(Arc::new(format!(
    "window.__bridgethingOverlay = {config};\n{}",
    body.unwrap_or(OVERLAY_JS)
  )))
}

pub fn geo_script(modern_port: u16) -> Arc<String> {
  let config = serde_json::json!({ "origin": kiosk_origin(modern_port) });
  Arc::new(format!("window.__bridgethingGeo = {config};\n{GEO_JS}"))
}

#[cfg(test)]
mod tests {
  use super::*;

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
  fn all_off_produces_no_script() {
    assert!(overlay_script(&all_off(), 8891, None).is_none());
  }

  #[test]
  fn default_profile_injects_every_surface() {
    let script = overlay_script(&OverlayProfile::default(), 8891, None).expect("script");
    assert!(script.starts_with("window.__bridgethingOverlay = "));
    for surface in ["notifications", "call", "pairing", "connection", "volume", "voice"] {
      assert!(script.contains(&format!("\"{surface}\":true")), "{surface} on");
    }
    assert!(script.contains(&kiosk_origin(8891)));
    assert!(script.contains("ws://127.0.0.1:8891/?scope=overlay"));
  }

  #[test]
  fn a_partial_profile_reports_each_surface_as_declared() {
    let profile = OverlayProfile {
      notifications: true,
      call: false,
      pairing: true,
      connection: false,
      volume: false,
      voice: false,
    };
    let script = overlay_script(&profile, 8891, None).expect("script");
    assert!(script.contains("\"notifications\":true"));
    assert!(script.contains("\"call\":false"));
    assert!(script.contains("\"pairing\":true"));
    assert!(script.contains("\"connection\":false"));
  }

  #[test]
  fn custom_body_replaces_the_builtin_under_the_same_prelude() {
    let script = overlay_script(&OverlayProfile::default(), 8891, Some("/* mine */")).expect("script");
    assert!(script.starts_with("window.__bridgethingOverlay = "));
    assert!(script.contains(&kiosk_origin(8891)));
    assert!(script.ends_with("/* mine */"));
    assert!(!script.contains("__bridgethingOverlayMounted"));
  }

  #[test]
  fn a_custom_body_still_needs_a_surface_to_be_enabled() {
    assert!(overlay_script(&all_off(), 8891, Some("/* mine */")).is_none());
  }

  #[test]
  fn the_geo_bridge_stands_alone_whatever_the_overlay_profile_says() {
    let script = geo_script(8891);
    assert!(script.starts_with("window.__bridgethingGeo = "));
    assert!(script.contains(&kiosk_origin(8891)));
    assert!(!script.contains("window.__bridgethingOverlay = "));
  }
}

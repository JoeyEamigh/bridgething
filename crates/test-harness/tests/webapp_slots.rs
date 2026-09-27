use std::time::Duration;

use bridgething_gateway::RequestFailure;
use bridgething_test_harness::Harness;
use futures::StreamExt;
use libbridgething::{
  WebappError,
  client::{BridgeToClientConfigMsgEvent, ConfigGet, WebappActivate},
  gateway::{
    WebappConfigSet, WebappResource, WebappResourceKind, WebappSetSlot, WebappSlot, WebappSwitchTo, WebappUninstall,
  },
};
use uuid::Uuid;

const CUSTOM_OVERLAY_BODY: &str = "/* a custom overlay */";
const SETTLE: Duration = Duration::from_secs(3);

struct Planted {
  id: Uuid,
}

async fn plant(harness: &Harness, role: Option<&str>, overlay: bool) -> Planted {
  plant_bundle(harness, role, overlay, true).await
}

async fn plant_bundle(harness: &Harness, role: Option<&str>, overlay: bool, app_entry: bool) -> Planted {
  let id = Uuid::now_v7();
  let dir = harness.state_dir().join("webapps").join(id.simple().to_string());
  std::fs::create_dir_all(&dir).expect("bundle dir");
  if app_entry {
    std::fs::write(dir.join("index.html"), b"<h1>planted</h1>").expect("index");
  }

  let mut fields = format!(
    r#""id":"{id}","name":"planted","version":"0.1.0","config":[{{"type":"string","data":{{"key":"units","label":"Units"}}}}]"#
  );
  if let Some(role) = role {
    fields.push_str(&format!(r#","role":"{role}""#));
  }
  if overlay {
    std::fs::write(dir.join("overlay.js"), CUSTOM_OVERLAY_BODY).expect("overlay");
    fields.push_str(r#","overlay":"overlay.js""#);
  }
  std::fs::write(dir.join("manifest.json"), format!("{{{fields}}}")).expect("manifest");

  harness.state().webapps.rescan().await;
  Planted { id }
}

#[tokio::test]
async fn slots_start_empty_and_report_the_builtin() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");

  let slots = companion.webapp().get_slots().await.expect("get slots");
  assert_eq!(slots.launcher, None, "no launcher designated on a fresh device");
  assert_eq!(slots.overlay, None, "no overlay designated on a fresh device");
}

#[tokio::test]
async fn designating_a_launcher_moves_the_home_screen_off_the_builtin_hub() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, Some("launcher"), false).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let builtin_home = harness.state().launcher_webapp().await.expect("launcher");
  assert_ne!(builtin_home, Some(planted.id), "planted app is not the home screen yet");

  let slots = companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Launcher,
      id: Some(planted.id),
    })
    .await
    .expect("set launcher slot");
  assert_eq!(slots.launcher, Some(planted.id));

  assert_eq!(
    harness.state().launcher_webapp().await.expect("launcher"),
    Some(planted.id),
    "the designated launcher is now the home screen"
  );
}

#[tokio::test]
async fn clearing_the_launcher_slot_restores_the_builtin_hub() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, Some("launcher"), false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let builtin_home = harness.state().launcher_webapp().await.expect("launcher");

  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Launcher,
      id: Some(planted.id),
    })
    .await
    .expect("set launcher slot");

  let slots = companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Launcher,
      id: None,
    })
    .await
    .expect("clear launcher slot");
  assert_eq!(slots.launcher, None);
  assert_eq!(
    harness.state().launcher_webapp().await.expect("launcher"),
    builtin_home,
    "clearing the slot is the recovery path back to the builtin hub"
  );
}

#[tokio::test]
async fn a_standard_webapp_is_refused_the_launcher_slot() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, false).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let err = companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Launcher,
      id: Some(planted.id),
    })
    .await
    .expect_err("a standard bundle must not take the launcher slot");
  assert!(
    matches!(&err, RequestFailure::Domain(WebappError::NotALauncher { id }) if id == &planted.id.to_string()),
    "expected NotALauncher, got {err:?}"
  );
}

#[tokio::test]
async fn a_webapp_without_an_overlay_entry_is_refused_the_overlay_slot() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, false).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let err = companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(planted.id),
    })
    .await
    .expect_err("a bundle with no overlay entry must not take the overlay slot");
  assert!(
    matches!(&err, RequestFailure::Domain(WebappError::NoOverlay { id }) if id == &planted.id.to_string()),
    "expected NoOverlay, got {err:?}"
  );
}

#[tokio::test]
async fn an_unknown_id_is_refused_either_slot() {
  let harness = Harness::start().await.expect("harness start");
  let companion = harness.connect_android().await.expect("connect companion");
  let ghost = Uuid::now_v7();

  for slot in [WebappSlot::Launcher, WebappSlot::Overlay] {
    let err = companion
      .webapp()
      .set_slot(WebappSetSlot { slot, id: Some(ghost) })
      .await
      .expect_err("an uninstalled id must not take a slot");
    assert!(
      matches!(&err, RequestFailure::Domain(WebappError::WebappNotFound { id }) if id == &ghost.to_string()),
      "expected WebappNotFound for {slot:?}, got {err:?}"
    );
  }
}

#[tokio::test]
async fn the_designated_overlay_replaces_the_builtin_script_body() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, true).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let builtin = harness.state().resolve_overlay_script().await.expect("builtin script");
  assert!(
    !builtin.contains(CUSTOM_OVERLAY_BODY),
    "builtin overlay must not carry the planted body"
  );

  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(planted.id),
    })
    .await
    .expect("set overlay slot");

  let custom = harness.state().resolve_overlay_script().await.expect("custom script");
  assert!(
    custom.ends_with(CUSTOM_OVERLAY_BODY),
    "planted overlay body is injected"
  );
  assert!(
    custom.starts_with("window.__bridgethingOverlay = "),
    "a custom overlay still gets the config prelude"
  );
  assert!(
    custom.contains("\"pairing\":true"),
    "a custom overlay is told which surfaces are enabled"
  );
}

#[tokio::test]
async fn clearing_the_overlay_slot_restores_the_builtin_script() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, true).await;
  let companion = harness.connect_android().await.expect("connect companion");

  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(planted.id),
    })
    .await
    .expect("set overlay slot");
  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: None,
    })
    .await
    .expect("clear overlay slot");

  let script = harness.state().resolve_overlay_script().await.expect("script");
  assert!(
    !script.contains(CUSTOM_OVERLAY_BODY),
    "clearing the slot is the recovery path back to the builtin overlay"
  );
}

#[tokio::test]
async fn uninstalling_a_slot_holder_releases_both_slots() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, Some("launcher"), true).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let builtin_home = harness.state().launcher_webapp().await.expect("launcher");

  for slot in [WebappSlot::Launcher, WebappSlot::Overlay] {
    companion
      .webapp()
      .set_slot(WebappSetSlot {
        slot,
        id: Some(planted.id),
      })
      .await
      .expect("set slot");
  }

  companion
    .webapp()
    .uninstall(WebappUninstall { id: planted.id })
    .await
    .expect("uninstall");

  let slots = companion.webapp().get_slots().await.expect("get slots");
  assert_eq!(slots.launcher, None, "launcher slot released on uninstall");
  assert_eq!(slots.overlay, None, "overlay slot released on uninstall");
  assert_eq!(
    harness.state().launcher_webapp().await.expect("launcher"),
    builtin_home,
    "home screen falls back to the builtin hub"
  );
  let script = harness.state().resolve_overlay_script().await.expect("script");
  assert!(
    !script.contains(CUSTOM_OVERLAY_BODY),
    "overlay falls back to the builtin script"
  );
}

#[tokio::test]
async fn an_uninstall_reaches_the_launcher_that_is_drawing_the_grid() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let mut client = harness.connect_modern_client().await.expect("connect modern client");

  companion
    .webapp()
    .uninstall(WebappUninstall { id: planted.id })
    .await
    .expect("uninstall");

  let mut seen: Vec<String> = Vec::new();
  let deadline = tokio::time::Instant::now() + SETTLE;
  loop {
    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
    assert!(
      !left.is_zero(),
      "a launcher only redraws its grid when the daemon says an app left; it saw {seen:?}"
    );
    match tokio::time::timeout(left, client.recv()).await {
      Ok(Some(text)) if text.contains("webappUninstalled") => {
        assert!(
          text.contains(&planted.id.to_string()),
          "the event names the app that left so a grid can drop the right tile: {text}"
        );
        return;
      }
      Ok(Some(text)) => seen.push(text),
      Ok(None) => panic!("the client link closed before the event arrived"),
      Err(_) => {}
    }
  }
}

#[tokio::test]
async fn a_bundle_that_stops_declaring_launcher_degrades_to_the_builtin() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, Some("launcher"), false).await;
  let companion = harness.connect_android().await.expect("connect companion");
  let builtin_home = harness.state().launcher_webapp().await.expect("launcher");

  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Launcher,
      id: Some(planted.id),
    })
    .await
    .expect("set launcher slot");

  let dir = harness
    .state_dir()
    .join("webapps")
    .join(planted.id.simple().to_string());
  std::fs::write(
    dir.join("manifest.json"),
    format!(r#"{{"id":"{}","name":"planted","version":"0.2.0"}}"#, planted.id),
  )
  .expect("manifest rewrite");
  harness.state().webapps.rescan().await;

  assert_eq!(
    harness.state().launcher_webapp().await.expect("launcher"),
    builtin_home,
    "an ineligible designation must not leave the device without a home screen"
  );
  let slots = companion.webapp().get_slots().await.expect("get slots");
  assert_eq!(slots.launcher, None, "the stale designation reads as empty");
}

#[tokio::test]
async fn the_overlay_is_fetchable_as_a_resource() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, true).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let list = companion.webapp().list().await.expect("list");
  let info = list
    .webapps
    .iter()
    .find(|w| w.id == planted.id)
    .expect("planted listed");
  assert!(info.overlay_hash.is_some(), "an overlay provider advertises its hash");

  let reply = companion
    .webapp()
    .resource(WebappResource {
      id: planted.id,
      kind: WebappResourceKind::Overlay,
      have: None,
    })
    .await
    .expect("overlay resource");
  assert_eq!(reply.mime.as_deref(), Some("text/javascript"));
  assert_eq!(reply.sha256, info.overlay_hash.clone().expect("hash"));
}

#[tokio::test]
async fn an_overlay_only_bundle_cannot_become_the_active_webapp() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant_bundle(&harness, None, true, false).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let before = harness.state().active_webapp().await.expect("active");
  assert_ne!(before, Some(planted.id), "an overlay-only bundle does not start active");

  let err = companion
    .webapp()
    .switch_to(WebappSwitchTo { id: planted.id })
    .await
    .expect_err("a bundle with nothing to show must not become the active webapp");
  assert!(
    matches!(&err, RequestFailure::Domain(WebappError::MissingIndexHtml)),
    "expected MissingIndexHtml, got {err:?}"
  );
  assert_eq!(
    harness.state().active_webapp().await.expect("active"),
    before,
    "the refused switch leaves the screen where it was"
  );
}

#[tokio::test]
async fn an_overlay_only_bundle_is_refused_the_launcher_slot() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant_bundle(&harness, Some("launcher"), true, false).await;
  let companion = harness.connect_android().await.expect("connect companion");

  let err = companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Launcher,
      id: Some(planted.id),
    })
    .await
    .expect_err("a bundle with no app entry cannot be the home screen");
  assert!(
    matches!(&err, RequestFailure::Domain(WebappError::NotALauncher { id }) if id == &planted.id.to_string()),
    "expected NotALauncher, got {err:?}"
  );
}

#[tokio::test]
async fn losing_an_app_entry_on_disk_drops_the_active_webapp_back_to_the_home_screen() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant(&harness, None, true).await;
  let companion = harness.connect_android().await.expect("connect companion");

  companion
    .webapp()
    .switch_to(WebappSwitchTo { id: planted.id })
    .await
    .expect("switch to the planted app");
  assert_eq!(harness.state().active_webapp().await.expect("active"), Some(planted.id));

  let dir = harness
    .state_dir()
    .join("webapps")
    .join(planted.id.simple().to_string());
  std::fs::remove_file(dir.join("index.html")).expect("strip the app entry");
  harness.state().webapps.rescan().await;

  let home = harness.state().launcher_webapp().await.expect("launcher");
  assert_eq!(
    harness.state().active_webapp().await.expect("active"),
    home,
    "a bundle that lost its app entry cannot stay active"
  );
}

#[tokio::test]
async fn the_on_device_launcher_cannot_activate_an_overlay_only_bundle() {
  let harness = Harness::start().await.expect("harness start");
  let planted = plant_bundle(&harness, None, true, false).await;
  let client = harness.connect_command_client().await.expect("connect client");

  let err = client
    .webapp()
    .activate(WebappActivate { id: planted.id })
    .await
    .expect_err("the launcher must not put a bundle with nothing to show on screen");
  assert!(
    matches!(&err, RequestFailure::Domain(WebappError::MissingIndexHtml)),
    "expected MissingIndexHtml, got {err:?}"
  );
}

#[tokio::test]
async fn an_overlay_scoped_client_reads_its_own_config_not_the_foreground_app_s() {
  let harness = Harness::start().await.expect("harness start");
  let foreground = plant(&harness, None, false).await;
  let overlay = plant(&harness, None, true).await;
  let companion = harness.connect_android().await.expect("connect companion");

  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(overlay.id),
    })
    .await
    .expect("set overlay slot");
  harness
    .state()
    .set_active_webapp(foreground.id)
    .await
    .expect("activate");

  for (id, value) in [(foreground.id, "imperial"), (overlay.id, "metric")] {
    companion
      .webapp()
      .config_set(WebappConfigSet {
        id,
        key: "units".into(),
        value: value.into(),
      })
      .await
      .expect("config set");
  }

  let overlay_client = harness
    .connect_overlay_command_client()
    .await
    .expect("overlay scoped client");
  let read = overlay_client
    .config()
    .get(ConfigGet { key: "units".into() })
    .await
    .expect("overlay config get");
  assert_eq!(
    read.value.as_deref(),
    Some("metric"),
    "an overlay reads the settings of the app holding the overlay slot"
  );

  let foreground_client = harness.connect_command_client().await.expect("command client");
  let read = foreground_client
    .config()
    .get(ConfigGet { key: "units".into() })
    .await
    .expect("foreground config get");
  assert_eq!(read.value.as_deref(), Some("imperial"));
}

#[tokio::test]
async fn a_config_change_reaches_the_scope_that_owns_it_and_no_other() {
  let harness = Harness::start().await.expect("harness start");
  let foreground = plant(&harness, None, false).await;
  let overlay = plant(&harness, None, true).await;
  let companion = harness.connect_android().await.expect("connect companion");

  companion
    .webapp()
    .set_slot(WebappSetSlot {
      slot: WebappSlot::Overlay,
      id: Some(overlay.id),
    })
    .await
    .expect("set overlay slot");
  harness
    .state()
    .set_active_webapp(foreground.id)
    .await
    .expect("activate");

  let overlay_client = harness
    .connect_overlay_command_client()
    .await
    .expect("overlay scoped client");
  let foreground_client = harness.connect_command_client().await.expect("command client");
  let mut overlay_events = Box::pin(overlay_client.config().events());
  let mut foreground_events = Box::pin(foreground_client.config().events());

  companion
    .webapp()
    .config_set(WebappConfigSet {
      id: overlay.id,
      key: "units".into(),
      value: "metric".into(),
    })
    .await
    .expect("config set");

  let BridgeToClientConfigMsgEvent::Changed(changed) = tokio::time::timeout(SETTLE, overlay_events.next())
    .await
    .expect("overlay hears its own config change")
    .expect("stream open");
  assert_eq!(changed.key, "units");
  assert_eq!(changed.value.as_deref(), Some("metric"));

  assert!(
    tokio::time::timeout(Duration::from_millis(250), foreground_events.next())
      .await
      .is_err(),
    "the foreground webapp is not told about the overlay app's settings"
  );
}

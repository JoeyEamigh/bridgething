use std::{
  io::Write,
  path::Path,
  sync::atomic::{AtomicBool, Ordering},
};

const SERVICE_DIR: &str = "/run/avahi/services";
const SERVICE_FILE_NAME: &str = "bridgething.service";

static PUBLISH_FAILURE_WARNED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub enum AvahiError {
  #[error("avahi service-file write failed: {0}")]
  Io(#[from] std::io::Error),
  #[cfg(feature = "systemd")]
  #[error("avahi reload dbus call failed: {0}")]
  Dbus(#[from] zbus::Error),
  #[cfg(not(feature = "systemd"))]
  #[error("systemd cargo feature disabled; avahi reload unavailable")]
  Disabled,
}

#[cfg(feature = "systemd")]
#[zbus::proxy(
  interface = "org.freedesktop.systemd1.Manager",
  default_service = "org.freedesktop.systemd1",
  default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
  fn reload_unit(&self, name: &str, mode: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

fn render_service_xml(nickname: Option<&str>, serial: &str) -> String {
  let short = short_serial(serial);
  let mut out = String::with_capacity(512);
  out.push_str("<?xml version=\"1.0\" standalone='no'?>\n");
  out.push_str("<!DOCTYPE service-group SYSTEM \"avahi-service.dtd\">\n");
  out.push_str("<service-group>\n");
  out.push_str("  <name replace-wildcards=\"yes\">");
  match short.is_empty() {
    true => out.push_str("%h Bridgething Gateway"),
    false => {
      out.push_str("%h-");
      out.push_str(&xml_escape_text(&short));
      out.push_str(" Bridgething Gateway");
    }
  }
  out.push_str("</name>\n");
  out.push_str("  <service>\n");
  out.push_str("    <type>");
  out.push_str(libbridgething::BRIDGETHING_MDNS_SERVICE_TYPE);
  out.push_str("</type>\n");
  out.push_str("    <port>");
  out.push_str(&libbridgething::BRIDGETHING_NETWORK_GATEWAY_PORT.to_string());
  out.push_str("</port>\n");
  if let Some(value) = nickname {
    out.push_str("    <txt-record>nickname=");
    out.push_str(&xml_escape_text(value));
    out.push_str("</txt-record>\n");
  }
  if !serial.is_empty() {
    out.push_str("    <txt-record>serial=");
    out.push_str(&xml_escape_text(serial));
    out.push_str("</txt-record>\n");
  }
  out.push_str("  </service>\n");
  out.push_str("</service-group>\n");
  out
}

pub fn short_serial(serial: &str) -> String {
  let kept: String = serial
    .chars()
    .filter(|c| c.is_ascii_alphanumeric())
    .map(|c| c.to_ascii_lowercase())
    .collect();
  kept[kept.len().saturating_sub(4)..].to_owned()
}

fn xml_escape_text(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  for c in s.chars() {
    match c {
      '&' => out.push_str("&amp;"),
      '<' => out.push_str("&lt;"),
      '>' => out.push_str("&gt;"),
      '"' => out.push_str("&quot;"),
      '\'' => out.push_str("&apos;"),
      _ => out.push(c),
    }
  }
  out
}

pub async fn publish_bridgething_service(nickname: Option<&str>, serial: &str) {
  let err = match try_publish(nickname, serial).await {
    Ok(()) => {
      PUBLISH_FAILURE_WARNED.store(false, Ordering::Relaxed);
      return;
    }
    Err(err) => err,
  };

  if PUBLISH_FAILURE_WARNED.swap(true, Ordering::Relaxed) {
    tracing::debug!(?err, "avahi publish still failing");
  } else {
    tracing::warn!(?err, "avahi publish failed; gateway will not be discoverable over mdns");
  }
}

async fn try_publish(nickname: Option<&str>, serial: &str) -> Result<(), AvahiError> {
  write_service_file(Path::new(SERVICE_DIR), nickname, serial)?;
  reload_avahi().await
}

fn write_service_file(dir: &Path, nickname: Option<&str>, serial: &str) -> Result<(), AvahiError> {
  std::fs::create_dir_all(dir)?;
  let xml = render_service_xml(nickname, serial);
  let path = dir.join(SERVICE_FILE_NAME);
  let tmp = path.with_extension("service.tmp");
  {
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(xml.as_bytes())?;
    f.sync_all()?;
  }
  std::fs::rename(&tmp, &path)?;
  Ok(())
}

#[cfg(feature = "systemd")]
async fn reload_avahi() -> Result<(), AvahiError> {
  let conn = zbus::Connection::system().await?;
  let proxy = SystemdManagerProxy::new(&conn).await?;
  proxy.reload_unit("avahi-daemon.service", "replace").await?;
  Ok(())
}

#[cfg(not(feature = "systemd"))]
async fn reload_avahi() -> Result<(), AvahiError> {
  Err(AvahiError::Disabled)
}

#[cfg(test)]
mod tests {
  use super::*;

  const SERIAL: &str = "8558R481Q61R";

  #[test]
  fn renders_without_nickname() {
    let xml = render_service_xml(None, SERIAL);
    assert!(xml.contains("<port>8892</port>"));
    assert!(!xml.contains("nickname="));
  }

  #[test]
  fn renders_with_nickname() {
    let xml = render_service_xml(Some("Joey's Car Thing"), SERIAL);
    assert!(xml.contains("<txt-record>nickname=Joey&apos;s Car Thing</txt-record>"));
  }

  #[test]
  fn the_serial_names_the_instance_and_rides_a_txt_record() {
    let xml = render_service_xml(None, SERIAL);
    assert!(
      xml.contains(r#"<name replace-wildcards="yes">%h-q61r Bridgething Gateway</name>"#),
      "two devices publishing the same hostname still browse as two instances, got {xml}"
    );
    assert!(
      xml.contains("<txt-record>serial=8558R481Q61R</txt-record>"),
      "the full serial is what a browser dedups on, got {xml}"
    );
  }

  #[test]
  fn a_device_with_no_serial_publishes_the_bare_name() {
    let xml = render_service_xml(None, "");
    assert!(xml.contains(r#"<name replace-wildcards="yes">%h Bridgething Gateway</name>"#));
    assert!(!xml.contains("serial="));
  }

  #[test]
  fn the_short_serial_is_lowercase_and_hostname_safe() {
    assert_eq!(short_serial(SERIAL), "q61r");
    assert_eq!(short_serial("ab"), "ab", "a short serial is used whole");
    assert_eq!(
      short_serial("12-34:56 78"),
      "5678",
      "punctuation is not a hostname label"
    );
    assert_eq!(short_serial(""), "");
  }

  #[test]
  fn escapes_xml_special_chars() {
    let xml = render_service_xml(Some("a&b<c>d\"e'f"), SERIAL);
    assert!(xml.contains("nickname=a&amp;b&lt;c&gt;d&quot;e&apos;f"));
  }

  #[test]
  fn writes_service_file_creating_missing_dirs() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("avahi/services");

    write_service_file(&dir, Some("Kitchen Thing"), SERIAL).unwrap();

    let written = std::fs::read_to_string(dir.join(SERVICE_FILE_NAME)).unwrap();
    assert_eq!(written, render_service_xml(Some("Kitchen Thing"), SERIAL));
  }

  #[test]
  fn rewrite_replaces_previous_contents_and_leaves_no_temp_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path();

    write_service_file(dir, Some("old"), SERIAL).unwrap();
    write_service_file(dir, Some("new"), SERIAL).unwrap();

    let written = std::fs::read_to_string(dir.join(SERVICE_FILE_NAME)).unwrap();
    assert!(written.contains("nickname=new"));
    assert!(!written.contains("nickname=old"));

    let leftovers: Vec<_> = std::fs::read_dir(dir)
      .unwrap()
      .map(|e| e.unwrap().file_name())
      .filter(|name| name != SERVICE_FILE_NAME)
      .collect();
    assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
  }

  #[test]
  fn unwritable_service_dir_surfaces_io_error() {
    let root = tempfile::tempdir().unwrap();
    let blocker = root.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();

    let err = write_service_file(&blocker.join("services"), None, SERIAL).unwrap_err();
    assert!(matches!(err, AvahiError::Io(_)));
  }
}

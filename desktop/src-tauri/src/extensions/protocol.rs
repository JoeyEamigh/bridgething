use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use bridgething_companion::backend::ExtensionMessage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "lowercase")]
pub enum WireForward {
  Text { data: String },
  Json { data: serde_json::Value },
  Binary { data: String },
}

impl TryFrom<ExtensionMessage> for WireForward {
  type Error = String;

  fn try_from(message: ExtensionMessage) -> Result<Self, Self::Error> {
    Ok(match message {
      ExtensionMessage::Text { text } => Self::Text { data: text },
      ExtensionMessage::Binary { bytes } => Self::Binary {
        data: STANDARD.encode(bytes),
      },
      ExtensionMessage::Json { json } => Self::Json {
        data: serde_json::from_str(&json).map_err(|error| error.to_string())?,
      },
    })
  }
}

impl WireForward {
  pub fn into_message(self) -> Result<ExtensionMessage, String> {
    Ok(match self {
      Self::Text { data } => ExtensionMessage::Text { text: data },
      Self::Json { data } => ExtensionMessage::Json { json: data.to_string() },
      Self::Binary { data } => ExtensionMessage::Binary {
        bytes: STANDARD.decode(data).map_err(|error| error.to_string())?,
      },
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebappIdentity {
  pub id: String,
  pub name: String,
  pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "t")]
pub enum HostMessage {
  #[serde(rename = "hello")]
  Hello {
    api: u32,
    webapp: WebappIdentity,
    #[serde(rename = "dataDir")]
    data_dir: String,
  },
  #[serde(rename = "device.connected")]
  DeviceConnected {
    device: String,
    name: String,
    config: BTreeMap<String, String>,
    active: bool,
  },
  #[serde(rename = "device.disconnected")]
  DeviceDisconnected { device: String },
  #[serde(rename = "device.active")]
  DeviceActive { device: String, active: bool },
  #[serde(rename = "device.message")]
  DeviceMessage { device: String, message: WireForward },
  #[serde(rename = "config.changed")]
  ConfigChanged {
    device: String,
    key: String,
    value: Option<String>,
  },
  #[serde(rename = "reply")]
  Reply {
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
  },
  #[serde(rename = "stop")]
  Stop,
}

impl HostMessage {
  pub fn answer(id: String, value: serde_json::Value) -> Self {
    Self::Reply {
      id,
      ok: true,
      value: Some(value),
      error: None,
    }
  }

  pub fn refuse(id: String, error: impl Into<String>) -> Self {
    Self::Reply {
      id,
      ok: false,
      value: None,
      error: Some(error.into()),
    }
  }

  pub fn line(&self) -> String {
    let body = serde_json::to_string(self).unwrap_or_else(|_| r#"{"t":"stop"}"#.to_owned());
    format!("{body}\n")
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireLogLevel {
  Debug,
  Info,
  Warn,
  Error,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t")]
pub enum ChildMessage {
  #[serde(rename = "device.send")]
  DeviceSend {
    #[serde(default)]
    device: Option<String>,
    message: WireForward,
  },
  #[serde(rename = "kv.get")]
  KvGet { id: String, key: String },
  #[serde(rename = "kv.set")]
  KvSet {
    id: String,
    key: String,
    value: serde_json::Value,
  },
  #[serde(rename = "kv.delete")]
  KvDelete { id: String, key: String },
  #[serde(rename = "kv.list")]
  KvList { id: String },
  #[serde(rename = "auth.authorize")]
  Authorize { id: String, url: String },
  #[serde(rename = "log")]
  Log { level: WireLogLevel, message: String },
  #[serde(rename = "ready")]
  Ready,
}

pub enum Stdout {
  Protocol(Box<ChildMessage>),
  Output(String),
}

pub fn read_line(line: &str) -> Stdout {
  match serde_json::from_str::<ChildMessage>(line) {
    Ok(message) => Stdout::Protocol(Box::new(message)),
    Err(_) => Stdout::Output(line.to_owned()),
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeSet;

  use libbridgething::EXTENSION_API_VERSION;

  use super::*;

  const PROTOCOL_FIXTURE: &str = include_str!("../../../../packages/extension-ts/fixtures/protocol.v1.json");

  fn fixture() -> serde_json::Value {
    serde_json::from_str(PROTOCOL_FIXTURE).expect("the shared protocol fixture is json")
  }

  fn entry(section: &str, name: &str) -> serde_json::Value {
    fixture()
      .get(section)
      .and_then(|section| section.get(name))
      .cloned()
      .unwrap_or_else(|| panic!("the shared fixture has no {section} entry named {name}"))
  }

  fn names(section: &str) -> BTreeSet<String> {
    fixture()[section]
      .as_object()
      .expect("a fixture section is an object")
      .keys()
      .cloned()
      .collect()
  }

  fn host_name(message: &HostMessage) -> &'static str {
    match message {
      HostMessage::Hello { .. } => "hello",
      HostMessage::DeviceConnected { .. } => "device.connected",
      HostMessage::DeviceDisconnected { .. } => "device.disconnected",
      HostMessage::DeviceActive { .. } => "device.active",
      HostMessage::DeviceMessage {
        message: WireForward::Text { .. },
        ..
      } => "device.message.text",
      HostMessage::DeviceMessage {
        message: WireForward::Json { .. },
        ..
      } => "device.message.json",
      HostMessage::DeviceMessage {
        message: WireForward::Binary { .. },
        ..
      } => "device.message.binary",
      HostMessage::ConfigChanged { value: Some(_), .. } => "config.changed.set",
      HostMessage::ConfigChanged { value: None, .. } => "config.changed.reset",
      HostMessage::Reply { ok: true, .. } => "reply.ok",
      HostMessage::Reply { ok: false, .. } => "reply.error",
      HostMessage::Stop => "stop",
    }
  }

  fn child_name(message: &ChildMessage) -> &'static str {
    match message {
      ChildMessage::DeviceSend { device: None, .. } => "device.send.broadcast",
      ChildMessage::DeviceSend {
        message: WireForward::Binary { .. },
        ..
      } => "device.send.binary",
      ChildMessage::DeviceSend { .. } => "device.send.text",
      ChildMessage::KvGet { .. } => "kv.get",
      ChildMessage::KvSet { .. } => "kv.set",
      ChildMessage::KvDelete { .. } => "kv.delete",
      ChildMessage::KvList { .. } => "kv.list",
      ChildMessage::Authorize { .. } => "auth.authorize",
      ChildMessage::Log { .. } => "log",
      ChildMessage::Ready => "ready",
    }
  }

  fn parse(line: &str) -> ChildMessage {
    match read_line(line) {
      Stdout::Protocol(message) => *message,
      Stdout::Output(raw) => panic!("{raw} should have decoded as protocol"),
    }
  }

  #[test]
  fn hello_carries_the_field_names_the_client_reads() {
    let line = HostMessage::Hello {
      api: 1,
      webapp: WebappIdentity {
        id: "9c1b".into(),
        name: "weather".into(),
        version: "1.2.0".into(),
      },
      data_dir: "/data".into(),
    }
    .line();

    assert_eq!(
      line,
      "{\"t\":\"hello\",\"api\":1,\"webapp\":{\"id\":\"9c1b\",\"name\":\"weather\",\"version\":\"1.2.0\"},\"dataDir\":\"/data\"}\n"
    );
  }

  #[test]
  fn every_host_message_is_one_line_ending_in_a_newline() {
    let messages = [
      HostMessage::DeviceDisconnected { device: "sn".into() },
      HostMessage::DeviceActive {
        device: "sn".into(),
        active: true,
      },
      HostMessage::ConfigChanged {
        device: "sn".into(),
        key: "zip".into(),
        value: Some("10001".into()),
      },
      HostMessage::Stop,
    ];

    for message in messages {
      let line = message.line();
      assert_eq!(line.matches('\n').count(), 1, "{line} must be a single line");
      assert!(line.ends_with('\n'));
    }
  }

  #[test]
  fn a_reset_setting_crosses_as_null_not_as_an_empty_string() {
    assert_eq!(
      HostMessage::ConfigChanged {
        device: "sn".into(),
        key: "zip".into(),
        value: None,
      }
      .line()
      .trim(),
      r#"{"t":"config.changed","device":"sn","key":"zip","value":null}"#
    );
  }

  #[test]
  fn an_ok_reply_omits_error_and_a_refusal_omits_value() {
    assert_eq!(
      HostMessage::answer("7".into(), serde_json::Value::Null).line().trim(),
      r#"{"t":"reply","id":"7","ok":true,"value":null}"#,
      "a kv miss answers with an explicit null, which the client reads as undefined"
    );
    assert_eq!(
      HostMessage::refuse("7".into(), "busy").line().trim(),
      r#"{"t":"reply","id":"7","ok":false,"error":"busy"}"#
    );
  }

  #[test]
  fn a_connected_device_serializes_its_config_as_an_object() {
    let line = HostMessage::DeviceConnected {
      device: "sn".into(),
      name: "car thing".into(),
      config: BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
      active: false,
    }
    .line();

    assert_eq!(
      line.trim(),
      r#"{"t":"device.connected","device":"sn","name":"car thing","config":{"zip":"10001"},"active":false}"#
    );
  }

  #[test]
  fn every_extension_to_host_type_decodes() {
    assert_eq!(
      parse(r#"{"t":"device.send","message":{"encoding":"text","data":"hi"}}"#),
      ChildMessage::DeviceSend {
        device: None,
        message: WireForward::Text { data: "hi".into() },
      },
      "an omitted device means broadcast, not a decode failure"
    );
    assert_eq!(
      parse(r#"{"t":"kv.set","id":"1","key":"a","value":{"b":2}}"#),
      ChildMessage::KvSet {
        id: "1".into(),
        key: "a".into(),
        value: serde_json::json!({ "b": 2 }),
      }
    );
    assert_eq!(
      parse(r#"{"t":"kv.list","id":"2"}"#),
      ChildMessage::KvList { id: "2".into() }
    );
    assert_eq!(
      parse(r#"{"t":"kv.get","id":"3","key":"a"}"#),
      ChildMessage::KvGet {
        id: "3".into(),
        key: "a".into(),
      }
    );
    assert_eq!(
      parse(r#"{"t":"kv.delete","id":"4","key":"a"}"#),
      ChildMessage::KvDelete {
        id: "4".into(),
        key: "a".into(),
      }
    );
    assert_eq!(
      parse(r#"{"t":"auth.authorize","id":"5","url":"https://example/authorize"}"#),
      ChildMessage::Authorize {
        id: "5".into(),
        url: "https://example/authorize".into(),
      }
    );
    assert_eq!(
      parse(r#"{"t":"log","level":"warn","message":"careful"}"#),
      ChildMessage::Log {
        level: WireLogLevel::Warn,
        message: "careful".into(),
      }
    );
    assert_eq!(parse(r#"{"t":"ready"}"#), ChildMessage::Ready);
  }

  #[test]
  fn a_line_that_is_not_protocol_is_kept_as_extension_output() {
    for line in ["hello from console.log", "{}", r#"{"t":"nonsense"}"#, "[1,2,3]"] {
      match read_line(line) {
        Stdout::Output(raw) => assert_eq!(raw, line),
        Stdout::Protocol(message) => panic!("{line} decoded as {message:?}"),
      }
    }
  }

  #[test]
  fn binary_forwards_are_base64_in_both_directions() {
    let bytes = vec![0u8, 1, 2, 250, 255];
    let wire = WireForward::try_from(ExtensionMessage::Binary { bytes: bytes.clone() }).expect("binary crosses");
    assert_eq!(
      wire,
      WireForward::Binary {
        data: "AAEC+v8=".to_owned()
      }
    );
    assert_eq!(
      wire.into_message(),
      Ok(ExtensionMessage::Binary { bytes }),
      "the bytes survive the base64 hop unchanged"
    );
  }

  #[test]
  fn a_json_forward_crosses_as_a_document_not_a_string() {
    let wire = WireForward::try_from(ExtensionMessage::Json {
      json: r#"{"a":[1,2]}"#.into(),
    })
    .expect("json crosses");
    assert_eq!(
      wire,
      WireForward::Json {
        data: serde_json::json!({ "a": [1, 2] })
      }
    );
    assert_eq!(
      wire.into_message(),
      Ok(ExtensionMessage::Json {
        json: r#"{"a":[1,2]}"#.into()
      })
    );
  }

  #[test]
  fn a_payload_that_does_not_cross_is_refused_rather_than_reshaped() {
    assert!(
      WireForward::Binary {
        data: "not base64!!".into(),
      }
      .into_message()
      .is_err()
    );
    assert!(
      WireForward::try_from(ExtensionMessage::Json { json: "{".into() }).is_err(),
      "a malformed document must not silently become a text forward"
    );
  }

  #[test]
  fn the_api_revision_is_the_one_the_shared_fixture_pins() {
    assert_eq!(fixture()["api"], serde_json::json!(EXTENSION_API_VERSION));
  }

  #[test]
  fn every_host_message_serializes_to_its_shared_fixture_entry() {
    let device = "0f3ab21c".to_owned();
    let messages = [
      HostMessage::Hello {
        api: 1,
        webapp: WebappIdentity {
          id: "019e6701-13f8-71b5-ba04-85d326630e98".into(),
          name: "weather".into(),
          version: "1.2.0".into(),
        },
        data_dir: "/data/weather".into(),
      },
      HostMessage::DeviceConnected {
        device: device.clone(),
        name: "car thing".into(),
        config: BTreeMap::from([("zip".to_owned(), "10001".to_owned())]),
        active: true,
      },
      HostMessage::DeviceDisconnected { device: device.clone() },
      HostMessage::DeviceActive {
        device: device.clone(),
        active: false,
      },
      HostMessage::DeviceMessage {
        device: device.clone(),
        message: WireForward::Text { data: "pong".into() },
      },
      HostMessage::DeviceMessage {
        device: device.clone(),
        message: WireForward::Json {
          data: serde_json::json!({ "ok": true }),
        },
      },
      HostMessage::DeviceMessage {
        device: device.clone(),
        message: WireForward::Binary {
          data: "AAEC+v8=".into(),
        },
      },
      HostMessage::ConfigChanged {
        device: device.clone(),
        key: "zip".into(),
        value: Some("10001".into()),
      },
      HostMessage::ConfigChanged {
        device,
        key: "zip".into(),
        value: None,
      },
      HostMessage::answer("1".into(), serde_json::json!({ "token": "abc" })),
      HostMessage::refuse("1".into(), "the host refused"),
      HostMessage::Stop,
    ];

    let mut covered = BTreeSet::new();
    for message in &messages {
      let name = host_name(message);
      let wire = serde_json::to_value(message).expect("a host message serializes");
      assert_eq!(
        wire,
        entry("hostToExtension", name),
        "{name} drifted from the shared fixture"
      );
      covered.insert(name.to_owned());
    }

    assert_eq!(
      covered,
      names("hostToExtension"),
      "every host to extension fixture entry must be asserted here"
    );
  }

  #[test]
  fn every_extension_message_in_the_shared_fixture_decodes() {
    let device = Some("0f3ab21c".to_owned());
    let expected = [
      ChildMessage::Ready,
      ChildMessage::Log {
        level: WireLogLevel::Info,
        message: "listening".into(),
      },
      ChildMessage::DeviceSend {
        device: device.clone(),
        message: WireForward::Text { data: "ping".into() },
      },
      ChildMessage::DeviceSend {
        device: None,
        message: WireForward::Json {
          data: serde_json::json!({ "cmd": "refresh" }),
        },
      },
      ChildMessage::DeviceSend {
        device,
        message: WireForward::Binary {
          data: "AAEC+v8=".into(),
        },
      },
      ChildMessage::KvGet {
        id: "1".into(),
        key: "creds".into(),
      },
      ChildMessage::KvSet {
        id: "2".into(),
        key: "creds".into(),
        value: serde_json::json!({ "token": "abc" }),
      },
      ChildMessage::KvDelete {
        id: "3".into(),
        key: "creds".into(),
      },
      ChildMessage::KvList { id: "4".into() },
      ChildMessage::Authorize {
        id: "5".into(),
        url: "https://example.test/authorize".into(),
      },
    ];

    let mut covered = BTreeSet::new();
    for want in expected {
      let name = child_name(&want);
      let line = serde_json::to_string(&entry("extensionToHost", name)).expect("a fixture entry re-serializes");
      assert_eq!(parse(&line), want, "{name} drifted from the shared fixture");
      covered.insert(name.to_owned());
    }

    assert_eq!(
      covered,
      names("extensionToHost"),
      "every extension to host fixture entry must be asserted here"
    );
  }
}

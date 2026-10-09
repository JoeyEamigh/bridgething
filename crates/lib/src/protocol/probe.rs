use rmpv::ValueRef;
use serde_json::Value;
use uuid::Uuid;

fn vref_str<'a>(v: &'a ValueRef<'a>) -> Option<&'a str> {
  match v {
    ValueRef::String(s) => s.as_str(),
    _ => None,
  }
}

/// Best-effort envelope-level fields extracted from a frame whose typed
/// decode failed. Every field is optional - the probe walks an untyped
/// representation of the payload and pulls what it can. Used for
/// structured logging and for auto-nacking unknown requests so the
/// sender's pending future resolves immediately instead of hitting the
/// 10s timeout.
///
/// Wire envelope shape across both protocols:
/// `{ id, meta: { kind, data? }, data: { type, data: { event, data? } } }`
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EnvelopeProbe {
  pub id: Option<Uuid>,
  pub meta_kind: Option<String>,
  pub data_type: Option<String>,
  pub data_event: Option<String>,
  pub request_id: Option<Uuid>,
}

impl EnvelopeProbe {
  pub fn is_request(&self) -> bool {
    matches!(self.meta_kind.as_deref(), Some("request"))
  }

  pub fn is_command(&self) -> bool {
    matches!(self.meta_kind.as_deref(), Some("command"))
  }
}

/// Walk an msgpack body without typed defs and pull out the envelope
/// fields we care about. Tolerates partial structures - any miss
/// returns None on that field rather than failing the whole probe.
pub fn try_probe_envelope_msgpack(bytes: &[u8]) -> EnvelopeProbe {
  let mut probe = EnvelopeProbe::default();
  let value = match rmpv::decode::read_value_ref(&mut &bytes[..]) {
    Ok(v) => v,
    Err(_) => return probe,
  };
  let map = match value {
    ValueRef::Map(m) => m,
    _ => return probe,
  };

  for (k, v) in &map {
    let Some(key) = vref_str(k) else { continue };
    match key {
      "id" => probe.id = msgpack_uuid(v),
      "meta" => {
        if let ValueRef::Map(meta_map) = v {
          for (mk, mv) in meta_map {
            match vref_str(mk) {
              Some("kind") => probe.meta_kind = vref_str(mv).map(str::to_owned),
              Some("data") => {
                if let ValueRef::Map(data_map) = mv {
                  for (dk, dv) in data_map {
                    if vref_str(dk) == Some("requestId") {
                      probe.request_id = msgpack_uuid(dv);
                    }
                  }
                }
              }
              _ => {}
            }
          }
        }
      }
      "data" => {
        if let ValueRef::Map(data_map) = v {
          for (dk, dv) in data_map {
            match vref_str(dk) {
              Some("type") => probe.data_type = vref_str(dv).map(str::to_owned),
              Some("data") => {
                if let ValueRef::Map(inner) = dv {
                  for (ik, iv) in inner {
                    if vref_str(ik) == Some("event") {
                      probe.data_event = vref_str(iv).map(str::to_owned);
                    }
                  }
                }
              }
              _ => {}
            }
          }
        }
      }
      _ => {}
    }
  }

  probe
}

pub fn try_probe_envelope_json(bytes: &[u8]) -> EnvelopeProbe {
  let mut probe = EnvelopeProbe::default();
  let value: Value = match serde_json::from_slice(bytes) {
    Ok(v) => v,
    Err(_) => return probe,
  };
  let Value::Object(map) = value else {
    return probe;
  };

  if let Some(id) = map.get("id") {
    probe.id = json_uuid(id);
  }
  if let Some(Value::Object(meta)) = map.get("meta") {
    if let Some(Value::String(kind)) = meta.get("kind") {
      probe.meta_kind = Some(kind.clone());
    }
    if let Some(Value::Object(meta_data)) = meta.get("data")
      && let Some(rid) = meta_data.get("requestId")
    {
      probe.request_id = json_uuid(rid);
    }
  }
  if let Some(Value::Object(data)) = map.get("data") {
    if let Some(Value::String(type_str)) = data.get("type") {
      probe.data_type = Some(type_str.clone());
    }
    if let Some(Value::Object(inner)) = data.get("data")
      && let Some(Value::String(event)) = inner.get("event")
    {
      probe.data_event = Some(event.clone());
    }
  }

  probe
}

fn msgpack_uuid(value: &ValueRef) -> Option<Uuid> {
  match value {
    ValueRef::Binary(bytes) if bytes.len() == 16 => Uuid::from_slice(bytes).ok(),
    ValueRef::Array(arr) if arr.len() == 16 => {
      let mut buf = [0u8; 16];
      for (i, v) in arr.iter().enumerate() {
        let n = v.as_u64()?;
        if n > u8::MAX as u64 {
          return None;
        }
        buf[i] = n as u8;
      }
      Some(Uuid::from_bytes(buf))
    }
    ValueRef::String(s) => s.as_str().and_then(|s: &str| Uuid::parse_str(s).ok()),
    _ => None,
  }
}

fn json_uuid(value: &Value) -> Option<Uuid> {
  match value {
    Value::String(s) => Uuid::parse_str(s).ok(),
    Value::Array(arr) if arr.len() == 16 => {
      let mut buf = [0u8; 16];
      for (i, v) in arr.iter().enumerate() {
        let n = v.as_u64()?;
        if n > u8::MAX as u64 {
          return None;
        }
        buf[i] = n as u8;
      }
      Some(Uuid::from_bytes(buf))
    }
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn msgpack_probe_extracts_envelope() {
    use rmp_serde::to_vec_named;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Inner {
      event: &'static str,
    }
    #[derive(Serialize)]
    struct Data {
      r#type: &'static str,
      data: Inner,
    }
    #[derive(Serialize)]
    struct Meta {
      kind: &'static str,
    }
    #[derive(Serialize)]
    struct Envelope {
      id: Uuid,
      meta: Meta,
      data: Data,
    }

    let id = Uuid::now_v7();
    let bytes = to_vec_named(&Envelope {
      id,
      meta: Meta { kind: "request" },
      data: Data {
        r#type: "library",
        data: Inner {
          event: "favoritesToggle",
        },
      },
    })
    .unwrap();

    let probe = try_probe_envelope_msgpack(&bytes);
    assert_eq!(probe.id, Some(id));
    assert_eq!(probe.meta_kind.as_deref(), Some("request"));
    assert_eq!(probe.data_type.as_deref(), Some("library"));
    assert_eq!(probe.data_event.as_deref(), Some("favoritesToggle"));
    assert!(probe.is_request());
  }

  #[test]
  fn json_probe_extracts_envelope() {
    let id = Uuid::now_v7();
    let body = serde_json::json!({
      "id": id.to_string(),
      "meta": { "kind": "request" },
      "data": {
        "type": "library",
        "data": { "event": "favoritesToggle" }
      }
    });
    let probe = try_probe_envelope_json(body.to_string().as_bytes());
    assert_eq!(probe.id, Some(id));
    assert_eq!(probe.meta_kind.as_deref(), Some("request"));
    assert_eq!(probe.data_type.as_deref(), Some("library"));
    assert_eq!(probe.data_event.as_deref(), Some("favoritesToggle"));
  }

  #[test]
  fn probe_returns_default_on_garbage() {
    let probe = try_probe_envelope_msgpack(&[0xff, 0xff, 0xff]);
    assert_eq!(probe, EnvelopeProbe::default());
  }

  #[test]
  fn probe_picks_up_partial_envelope() {
    let body = serde_json::json!({ "meta": { "kind": "command" } });
    let probe = try_probe_envelope_json(body.to_string().as_bytes());
    assert_eq!(probe.meta_kind.as_deref(), Some("command"));
    assert_eq!(probe.id, None);
    assert_eq!(probe.data_type, None);
  }

  #[test]
  fn probe_handles_real_gateway_envelope() {
    use crate::{
      gateway::{AssetNotFoundReply, GatewayToBridgeAssetMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData},
      wire::MsgMeta,
    };

    let id = Uuid::now_v7();
    let msg = GatewayToBridgeMsg {
      id,
      meta: MsgMeta::Request,
      data: GatewayToBridgeMsgData::Asset(GatewayToBridgeAssetMsg::NotFound(AssetNotFoundReply {
        id: "iap2/art/abc/0".into(),
      })),
    };
    let bytes = rmp_serde::to_vec_named(&msg).unwrap();
    let probe = try_probe_envelope_msgpack(&bytes);
    assert_eq!(probe.id, Some(id), "probe id should match envelope id");
    assert_eq!(probe.meta_kind.as_deref(), Some("request"));
    assert_eq!(probe.data_type.as_deref(), Some("asset"));
    assert_eq!(probe.data_event.as_deref(), Some("notFound"));
    assert!(probe.is_request());
  }
}

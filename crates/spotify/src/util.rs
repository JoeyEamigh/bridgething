use std::time::{SystemTime, UNIX_EPOCH};

const B62: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

pub(crate) fn now_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0)
}

pub fn gid_to_base62(gid: &[u8]) -> String {
  let mut n: u128 = 0;
  for &b in gid.iter().take(16) {
    n = (n << 8) | b as u128;
  }
  let mut out = [0u8; 22];
  for slot in out.iter_mut().rev() {
    *slot = B62[(n % 62) as usize];
    n /= 62;
  }
  String::from_utf8(out.to_vec()).unwrap()
}

pub fn image_hex(group: &librespot_protocol::metadata::ImageGroup) -> String {
  use librespot_protocol::metadata::image::Size;
  fn rank(img: &librespot_protocol::metadata::Image) -> i64 {
    if img.width() > 0 {
      return img.width() as i64;
    }
    match img.size() {
      Size::XLARGE => 3,
      Size::LARGE => 2,
      Size::DEFAULT => 1,
      Size::SMALL => 0,
    }
  }
  group
    .image
    .iter()
    .filter(|img| !img.file_id().is_empty())
    .max_by_key(|img| rank(img))
    .map(|img| hex::encode(img.file_id()))
    .unwrap_or_default()
}

fn varint(field: u32, mut v: u64, out: &mut Vec<u8>) {
  out.push((field << 3) as u8); // wiretype 0
  loop {
    let b = (v & 0x7f) as u8;
    v >>= 7;
    if v != 0 {
      out.push(b | 0x80);
    } else {
      out.push(b);
      break;
    }
  }
}

fn len_prefixed(field: u32, data: &[u8], out: &mut Vec<u8>) {
  out.push(((field << 3) | 2) as u8); // wiretype 2
  let mut n = data.len() as u64;
  loop {
    let b = (n & 0x7f) as u8;
    n >>= 7;
    if n != 0 {
      out.push(b | 0x80);
    } else {
      out.push(b);
      break;
    }
  }
  out.extend_from_slice(data);
}

pub fn client_token_request(client_version: &str, client_id: &str, device_id: &str) -> Vec<u8> {
  let mut platform = Vec::new();
  len_prefixed(5, &[], &mut platform); // desktop_linux{} (empty message)

  let mut sdk = Vec::new();
  len_prefixed(1, &platform, &mut sdk);
  len_prefixed(2, device_id.as_bytes(), &mut sdk);

  let mut client_data = Vec::new();
  len_prefixed(1, client_version.as_bytes(), &mut client_data);
  len_prefixed(2, client_id.as_bytes(), &mut client_data);
  len_prefixed(3, &sdk, &mut client_data);

  let mut out = Vec::new();
  varint(1, 1, &mut out); // request_type = REQUEST_CLIENT_DATA_REQUEST
  len_prefixed(2, &client_data, &mut out);
  out
}

pub fn parse_client_token(data: &[u8]) -> Option<(String, u64)> {
  let granted = field_message(data, 2)?;
  let token_bytes = field_bytes(&granted, 1)?;
  let token = String::from_utf8(token_bytes).ok()?;
  if token.is_empty() {
    return None;
  }
  let ttl = field_varint(&granted, 2).unwrap_or(1_209_600); // ~14d default
  Some((token, ttl))
}

fn field_bytes(data: &[u8], want: u8) -> Option<Vec<u8>> {
  let mut i = 0usize;
  while i < data.len() {
    let tag = data[i];
    i += 1;
    let field = tag >> 3;
    match tag & 7 {
      2 => {
        let (len, adv) = read_varint(&data[i..]);
        i += adv;
        let end = (i + len as usize).min(data.len());
        if field == want {
          return Some(data[i..end].to_vec());
        }
        i = end;
      }
      0 => {
        let (_, adv) = read_varint(&data[i..]);
        i += adv;
      }
      5 => i += 4,
      1 => i += 8,
      _ => break,
    }
  }
  None
}

fn field_message(data: &[u8], want: u8) -> Option<Vec<u8>> {
  field_bytes(data, want)
}

fn field_varint(data: &[u8], want: u8) -> Option<u64> {
  let mut i = 0usize;
  while i < data.len() {
    let tag = data[i];
    i += 1;
    let field = tag >> 3;
    match tag & 7 {
      0 => {
        let (v, adv) = read_varint(&data[i..]);
        i += adv;
        if field == want {
          return Some(v);
        }
      }
      2 => {
        let (len, adv) = read_varint(&data[i..]);
        i += adv;
        i = (i + len as usize).min(data.len());
      }
      5 => i += 4,
      1 => i += 8,
      _ => break,
    }
  }
  None
}

fn read_varint(data: &[u8]) -> (u64, usize) {
  let mut v = 0u64;
  let mut shift = 0u32;
  let mut i = 0usize;
  while i < data.len() && shift < 64 {
    let b = data[i];
    i += 1;
    v |= ((b & 0x7f) as u64) << shift;
    if b & 0x80 == 0 {
      break;
    }
    shift += 7;
  }
  (v, i)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn read_varint_basic() {
    assert_eq!(read_varint(&[0x01]), (1, 1));
    assert_eq!(read_varint(&[0xac, 0x02]), (300, 2));
  }

  #[test]
  fn read_varint_overlong_does_not_panic() {
    let (_v, adv) = read_varint(&[0xff; 12]);
    assert!(adv <= 10);
  }

  #[test]
  fn gid_to_base62_known_vectors() {
    assert_eq!(gid_to_base62(&[0u8; 16]), "0".repeat(22));
    let mut g = [0u8; 16];
    g[15] = 1;
    assert_eq!(gid_to_base62(&g), format!("{}1", "0".repeat(21)));
  }
}

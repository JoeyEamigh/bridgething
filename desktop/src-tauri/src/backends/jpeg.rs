pub fn edge(bytes: &[u8]) -> Option<u32> {
  if bytes.get(..2)? != [0xff, 0xd8] {
    return None;
  }
  let mut at = 2;
  while at + 4 <= bytes.len() {
    if bytes[at] != 0xff || bytes[at + 1] == 0xff {
      at += 1;
      continue;
    }
    let marker = bytes[at + 1];
    if marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
      at += 2;
      continue;
    }
    let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
    if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
      let height = u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]);
      let width = u16::from_be_bytes([*bytes.get(at + 7)?, *bytes.get(at + 8)?]);
      return Some(u32::from(width.max(height)));
    }
    at += 2 + length.max(2);
  }
  None
}

#[cfg(test)]
pub fn sample(width: u16, height: u16) -> Vec<u8> {
  let mut bytes = vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08];
  bytes.extend_from_slice(&height.to_be_bytes());
  bytes.extend_from_slice(&width.to_be_bytes());
  bytes.extend_from_slice(&[0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xff, 0xd9]);
  bytes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn jpeg_dimensions_come_from_the_start_of_frame_marker() {
    assert_eq!(edge(&sample(512, 512)), Some(512));
    assert_eq!(edge(&sample(640, 480)), Some(640));
    assert_eq!(edge(&sample(480, 640)), Some(640));
    assert_eq!(edge(b"not a jpeg at all"), None);
  }
}

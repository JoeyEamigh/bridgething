use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

pub const LINK_HEADER_LEN: usize = 9;
pub const PAYLOAD_TRAILER: usize = 1;
pub const LINK_FRAME_OVERHEAD: usize = LINK_HEADER_LEN + PAYLOAD_TRAILER;
pub const LINK_MAGIC: [u8; 2] = [0xFF, 0x5A];
pub const DETECT_MARKER: [u8; 6] = [0xFF, 0x55, 0x02, 0x00, 0xEE, 0x10];

bitflags::bitflags! {
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
  pub struct ControlBits: u8 {
    const SYN = 0x80;
    const ACK = 0x40;
    const EAK = 0x20;
    const RST = 0x10;
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SessionType {
  Control = 0x00,
  FileTransfer = 0x01,
  ExternalAccessory = 0x02,
}

impl SessionType {
  pub fn from_u8(byte: u8) -> Option<Self> {
    match byte {
      0x00 => Some(Self::Control),
      0x01 => Some(Self::FileTransfer),
      0x02 => Some(Self::ExternalAccessory),
      _ => None,
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkHeader {
  pub length: u16,
  pub control: ControlBits,
  pub seq: u8,
  pub ack: u8,
  pub session_id: u8,
}

impl LinkHeader {
  pub fn header_only(control: ControlBits, seq: u8, ack: u8) -> Self {
    Self {
      length: LINK_HEADER_LEN as u16,
      control,
      seq,
      ack,
      session_id: 0,
    }
  }

  pub fn with_payload(control: ControlBits, seq: u8, ack: u8, session_id: u8, payload_len: usize) -> Self {
    Self {
      length: (LINK_HEADER_LEN + payload_len + 1) as u16,
      control,
      seq,
      ack,
      session_id,
    }
  }

  pub fn has_payload(&self) -> bool {
    self.length as usize > LINK_HEADER_LEN
  }

  fn encode_into(&self, buf: &mut BytesMut) {
    let start = buf.len();
    buf.put_u8(LINK_MAGIC[0]);
    buf.put_u8(LINK_MAGIC[1]);
    buf.put_u16(self.length);
    buf.put_u8(self.control.bits());
    buf.put_u8(self.seq);
    buf.put_u8(self.ack);
    buf.put_u8(self.session_id);
    let csum = modular_sum_checksum(&buf[start..start + 8]);
    buf.put_u8(csum);
  }

  fn decode(buf: &[u8]) -> std::result::Result<Self, FrameError> {
    if buf.len() < LINK_HEADER_LEN {
      return Err(FrameError::Incomplete);
    }
    if buf[0..2] != LINK_MAGIC {
      return Err(FrameError::BadMagic);
    }
    if !verify_modular_sum(&buf[..LINK_HEADER_LEN]) {
      return Err(FrameError::BadHeaderChecksum);
    }
    Ok(Self {
      length: u16::from_be_bytes([buf[2], buf[3]]),
      control: ControlBits::from_bits_truncate(buf[4]),
      seq: buf[5],
      ack: buf[6],
      session_id: buf[7],
    })
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPacket {
  pub header: LinkHeader,
  pub payload: Bytes,
}

impl LinkPacket {
  pub fn header_only(control: ControlBits, seq: u8, ack: u8) -> Self {
    Self {
      header: LinkHeader::header_only(control, seq, ack),
      payload: Bytes::new(),
    }
  }

  pub fn with_payload(control: ControlBits, seq: u8, ack: u8, session_id: u8, payload: Bytes) -> Self {
    Self {
      header: LinkHeader::with_payload(control, seq, ack, session_id, payload.len()),
      payload,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lsp {
  pub version: u8,
  pub max_outgoing: u8,
  pub max_len: u16,
  pub retransmission_timeout_ms: u16,
  pub ack_timeout_ms: u16,
  pub max_retransmissions: u8,
  pub max_ack: u8,
  pub sessions: Vec<SessionTriple>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTriple {
  pub id: u8,
  pub session_type: u8,
  pub version: u8,
}

impl Lsp {
  pub fn accessory_default() -> Self {
    Self {
      version: 1,
      max_outgoing: 32,
      max_len: 4096,
      retransmission_timeout_ms: 6000,
      ack_timeout_ms: 3000,
      max_retransmissions: 30,
      max_ack: 3,
      sessions: vec![
        SessionTriple {
          id: 1,
          session_type: 0,
          version: 1,
        },
        SessionTriple {
          id: 2,
          session_type: 1,
          version: 2,
        },
        SessionTriple {
          id: 3,
          session_type: 2,
          version: 1,
        },
      ],
    }
  }

  pub fn encode(&self) -> Bytes {
    let mut buf = BytesMut::with_capacity(10 + self.sessions.len() * 3);
    buf.put_u8(self.version);
    buf.put_u8(self.max_outgoing);
    buf.put_u16(self.max_len);
    buf.put_u16(self.retransmission_timeout_ms);
    buf.put_u16(self.ack_timeout_ms);
    buf.put_u8(self.max_retransmissions);
    buf.put_u8(self.max_ack);
    for s in &self.sessions {
      buf.put_u8(s.id);
      buf.put_u8(s.session_type);
      buf.put_u8(s.version);
    }
    buf.freeze()
  }

  pub fn decode(buf: &[u8]) -> std::result::Result<Self, FrameError> {
    if buf.len() < 10 {
      return Err(FrameError::ShortLsp);
    }
    let session_bytes = &buf[10..];
    if !session_bytes.len().is_multiple_of(3) {
      return Err(FrameError::BadLspSessionList);
    }
    let (triples, _) = session_bytes.as_chunks::<3>();
    let sessions = triples
      .iter()
      .map(|&[id, session_type, version]| SessionTriple {
        id,
        session_type,
        version,
      })
      .collect();
    Ok(Self {
      version: buf[0],
      max_outgoing: buf[1],
      max_len: u16::from_be_bytes([buf[2], buf[3]]),
      retransmission_timeout_ms: u16::from_be_bytes([buf[4], buf[5]]),
      ack_timeout_ms: u16::from_be_bytes([buf[6], buf[7]]),
      max_retransmissions: buf[8],
      max_ack: buf[9],
      sessions,
    })
  }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
  #[error("incomplete frame")]
  Incomplete,
  #[error("bad link magic (expected ff 5a)")]
  BadMagic,
  #[error("bad header checksum")]
  BadHeaderChecksum,
  #[error("bad payload checksum")]
  BadPayloadChecksum,
  #[error("declared length {0} smaller than header length {1}")]
  ImplausibleLength(u16, usize),
  #[error("LSP shorter than 10 bytes")]
  ShortLsp,
  #[error("LSP session list length not a multiple of 3")]
  BadLspSessionList,
}

fn modular_sum_checksum(buf: &[u8]) -> u8 {
  let sum: u32 = buf.iter().map(|&b| b as u32).sum();
  ((-(sum as i32)) & 0xFF) as u8
}

fn verify_modular_sum(buf: &[u8]) -> bool {
  let sum: u32 = buf.iter().map(|&b| b as u32).sum();
  sum & 0xFF == 0
}

#[derive(Debug, Default)]
pub struct LinkCodec;

impl Decoder for LinkCodec {
  type Item = LinkPacket;
  type Error = std::io::Error;

  fn decode(&mut self, src: &mut BytesMut) -> std::result::Result<Option<Self::Item>, Self::Error> {
    loop {
      if src.len() < LINK_HEADER_LEN {
        return Ok(None);
      }
      if src[0..2] != LINK_MAGIC {
        src.advance(1);
        continue;
      }
      let header = match LinkHeader::decode(&src[..LINK_HEADER_LEN]) {
        Ok(h) => h,
        Err(FrameError::Incomplete) => return Ok(None),
        Err(FrameError::BadHeaderChecksum) | Err(FrameError::BadMagic) => {
          tracing::debug!("iap2 decode: magic matched but header checksum failed, resyncing one byte");
          src.advance(1);
          continue;
        }
        Err(other) => return Err(std::io::Error::other(other)),
      };
      let total_len = header.length as usize;
      if total_len < LINK_HEADER_LEN {
        tracing::debug!(
          "iap2 decode: implausible length {} (< header) for seq {}, resyncing one byte",
          header.length,
          header.seq
        );
        src.advance(1);
        continue;
      }
      if src.len() < total_len {
        return Ok(None);
      }
      let trailer_len = total_len - LINK_HEADER_LEN;
      let payload = if trailer_len == 0 {
        Bytes::new()
      } else {
        let payload_with_csum = &src[LINK_HEADER_LEN..total_len];
        if !verify_modular_sum(payload_with_csum) {
          tracing::warn!(
            "iap2 decode: bad payload checksum for seq {} (len {}), skipping packet",
            header.seq,
            header.length
          );
          src.advance(total_len);
          continue;
        }
        let payload_len = trailer_len - 1;
        Bytes::copy_from_slice(&src[LINK_HEADER_LEN..LINK_HEADER_LEN + payload_len])
      };
      src.advance(total_len);
      return Ok(Some(LinkPacket { header, payload }));
    }
  }
}

impl Encoder<LinkPacket> for LinkCodec {
  type Error = std::io::Error;

  fn encode(&mut self, item: LinkPacket, dst: &mut BytesMut) -> std::result::Result<(), Self::Error> {
    item.header.encode_into(dst);
    if !item.payload.is_empty() {
      let payload_start = dst.len();
      dst.put_slice(&item.payload);
      let csum = modular_sum_checksum(&dst[payload_start..]);
      dst.put_u8(csum);
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn header_roundtrip_pure_ack() {
    let header = LinkHeader::header_only(ControlBits::ACK, 100, 42);
    let mut buf = BytesMut::new();
    header.encode_into(&mut buf);
    assert_eq!(buf.len(), LINK_HEADER_LEN);
    assert_eq!(&buf[0..2], &LINK_MAGIC);
    let parsed = LinkHeader::decode(&buf).unwrap();
    assert_eq!(parsed, header);
  }

  #[test]
  fn header_checksum_is_modular_sum_zero() {
    let header = LinkHeader::header_only(ControlBits::SYN | ControlBits::ACK, 99, 0);
    let mut buf = BytesMut::new();
    header.encode_into(&mut buf);
    let sum: u32 = buf.iter().map(|&b| b as u32).sum();
    assert_eq!(sum & 0xFF, 0, "header bytes must sum to 0 mod 256");
  }

  #[test]
  fn lsp_roundtrip() {
    let lsp = Lsp {
      version: 1,
      max_outgoing: 5,
      max_len: 2048,
      retransmission_timeout_ms: 6000,
      ack_timeout_ms: 3000,
      max_retransmissions: 30,
      max_ack: 3,
      sessions: vec![
        SessionTriple {
          id: 1,
          session_type: 0,
          version: 1,
        },
        SessionTriple {
          id: 3,
          session_type: 2,
          version: 1,
        },
        SessionTriple {
          id: 2,
          session_type: 1,
          version: 2,
        },
      ],
    };
    let bytes = lsp.encode();
    assert_eq!(bytes.len(), 10 + 3 * 3);
    let parsed = Lsp::decode(&bytes).unwrap();
    assert_eq!(parsed, lsp);
  }

  #[test]
  fn codec_decodes_single_pure_ack() {
    let packet = LinkPacket::header_only(ControlBits::ACK, 5, 6);
    let mut buf = BytesMut::new();
    let mut codec = LinkCodec;
    codec.encode(packet.clone(), &mut buf).unwrap();
    let decoded = codec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(decoded, packet);
    assert!(buf.is_empty());
  }

  #[test]
  fn codec_decodes_syn_with_lsp_payload() {
    let lsp = Lsp {
      version: 1,
      max_outgoing: 5,
      max_len: 2048,
      retransmission_timeout_ms: 6000,
      ack_timeout_ms: 3000,
      max_retransmissions: 30,
      max_ack: 3,
      sessions: vec![SessionTriple {
        id: 1,
        session_type: 0,
        version: 1,
      }],
    };
    let payload = lsp.encode();
    let packet = LinkPacket::with_payload(ControlBits::SYN, 99, 0, 0, payload.clone());
    let mut buf = BytesMut::new();
    let mut codec = LinkCodec;
    codec.encode(packet.clone(), &mut buf).unwrap();
    let decoded = codec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(decoded, packet);
    assert_eq!(Lsp::decode(&decoded.payload).unwrap(), lsp);
  }

  #[test]
  fn codec_resyncs_past_garbage_prefix() {
    let packet = LinkPacket::header_only(ControlBits::ACK, 1, 2);
    let mut buf = BytesMut::new();
    buf.put_slice(&[0x00, 0xAA, 0xFF, 0x00, 0xFF, 0x55]);
    let mut codec = LinkCodec;
    codec.encode(packet.clone(), &mut buf).unwrap();
    let decoded = codec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(decoded, packet);
  }

  #[test]
  fn codec_returns_none_on_partial_frame() {
    let packet = LinkPacket::header_only(ControlBits::ACK, 1, 2);
    let mut full = BytesMut::new();
    let mut codec = LinkCodec;
    codec.encode(packet, &mut full).unwrap();
    let mut partial = BytesMut::from(&full[..LINK_HEADER_LEN - 1]);
    assert!(codec.decode(&mut partial).unwrap().is_none());
  }

  #[test]
  fn codec_decodes_two_packets_back_to_back() {
    let p1 = LinkPacket::header_only(ControlBits::ACK, 1, 2);
    let p2 = LinkPacket::header_only(ControlBits::ACK, 3, 4);
    let mut buf = BytesMut::new();
    let mut codec = LinkCodec;
    codec.encode(p1.clone(), &mut buf).unwrap();
    codec.encode(p2.clone(), &mut buf).unwrap();
    let d1 = codec.decode(&mut buf).unwrap().unwrap();
    let d2 = codec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(d1, p1);
    assert_eq!(d2, p2);
    assert!(buf.is_empty());
  }

  #[test]
  fn codec_drops_packet_with_bad_payload_checksum_and_resyncs() {
    let payload = Bytes::from_static(&[0x01, 0x02, 0x03]);
    let packet = LinkPacket::with_payload(ControlBits::ACK, 1, 2, 0, payload);
    let mut buf = BytesMut::new();
    let mut codec = LinkCodec;
    codec.encode(packet, &mut buf).unwrap();
    let last = buf.len() - 1;
    buf[last] = buf[last].wrapping_add(1);

    let good = LinkPacket::header_only(ControlBits::ACK, 9, 10);
    codec.encode(good.clone(), &mut buf).unwrap();

    let decoded = codec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(decoded, good);
  }
}

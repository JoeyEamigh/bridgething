use std::{
  io::{Cursor, Read, Write},
  marker::PhantomData,
  ops::Deref,
};

use flate2::{read::GzDecoder, write::GzEncoder};
use serde::{Serialize, de::DeserializeOwned};
use tokio_util::{
  bytes::{Buf, BufMut, Bytes, BytesMut},
  codec::{Decoder, Encoder},
};

use super::{
  AUTO_GZIP_THRESHOLD_BYTES, COMPRESSION_GZIP, COMPRESSION_NONE, Compress, Compression, DecodedFrame, ENCODING_MSGPACK,
  Encoding, EndecError, EndecState, HEADER_LEN, MAGIC, MAX_FRAME_LEN, PrioritizedFrame, TypedDecodeError, VERSION,
  mbps, try_probe_envelope_json, try_probe_envelope_msgpack,
};
use crate::{
  Priority,
  gateway::{BridgeToGatewayMsg, GatewayToBridgeMsg},
};

pub type BridgeEndec = WireEndec<GatewayToBridgeMsg, BridgeToGatewayMsg>;

pub type GatewayEndec = WireEndec<BridgeToGatewayMsg, GatewayToBridgeMsg>;

trait WireBuf: Deref<Target = [u8]> {
  type Body: Deref<Target = [u8]>;

  fn skip(&mut self, count: usize);
  fn split_body(&mut self, len: usize) -> Self::Body;
}

impl WireBuf for BytesMut {
  type Body = BytesMut;

  fn skip(&mut self, count: usize) {
    self.advance(count);
  }

  fn split_body(&mut self, len: usize) -> BytesMut {
    self.split_to(len)
  }
}

impl WireBuf for Bytes {
  type Body = Bytes;

  fn skip(&mut self, count: usize) {
    self.advance(count);
  }

  fn split_body(&mut self, len: usize) -> Bytes {
    self.split_to(len)
  }
}

pub struct WireEndec<In, Out> {
  state: Option<EndecState>,
  _direction: PhantomData<fn() -> (In, Out)>,
}

impl<In, Out> Default for WireEndec<In, Out> {
  fn default() -> Self {
    Self {
      state: None,
      _direction: PhantomData,
    }
  }
}

impl<In, Out> std::fmt::Debug for WireEndec<In, Out> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("WireEndec").field("state", &self.state).finish()
  }
}

impl<In, Out> WireEndec<In, Out>
where
  In: DeserializeOwned,
{
  pub fn decode_bytes(&mut self, src: &mut Bytes) -> Result<Option<DecodedFrame<In>>, EndecError> {
    self.decode_item(src)
  }

  fn decode_item<B: WireBuf>(&mut self, src: &mut B) -> Result<Option<DecodedFrame<In>>, EndecError> {
    match self.decode_buf(src) {
      Ok(Some(frame)) => Ok(Some(DecodedFrame::Frame(frame))),
      Ok(None) => Ok(None),
      Err(err) if err.is_recoverable() => Ok(Some(DecodedFrame::Failed(err))),
      Err(err) => Err(err),
    }
  }

  fn decode_buf<B: WireBuf>(&mut self, src: &mut B) -> Result<Option<PrioritizedFrame<In>>, EndecError> {
    loop {
      if src.is_empty() {
        return Ok(None);
      }

      let state = self.state.get_or_insert_default();

      if !state.header_parsed {
        if src.len() < HEADER_LEN {
          tracing::trace!(target: "libbridgething::protocol::decode", "not enough bytes for header (need {}, have {})", HEADER_LEN, src.len());
          state.packet += 1;
          return Ok(None);
        }

        let magic = u16::from_be_bytes([src[0], src[1]]);
        if magic != MAGIC {
          self.state = None;
          if resync_to_magic(src) {
            tracing::warn!(target: "libbridgething::protocol::decode", "invalid magic {magic:#x}; resynced to next frame");
            continue;
          }
          return Ok(None);
        }

        let version = src[2];
        if version != VERSION {
          tracing::warn!(target: "libbridgething::protocol::decode", "unsupported version {version}; resyncing");
          self.state = None;
          src.skip(1);
          if resync_to_magic(src) {
            continue;
          }
          return Ok(None);
        }

        let length = u64::from_be_bytes(src[8..16].try_into().expect("8-byte slice"));
        if length > MAX_FRAME_LEN as u64 {
          tracing::warn!(target: "libbridgething::protocol::decode", "frame length {length} over cap; resyncing");
          self.state = None;
          src.skip(1);
          if resync_to_magic(src) {
            continue;
          }
          return Ok(None);
        }

        state.compression = src[3].into();
        state.encoding = src[4].into();
        state.priority = Priority::from_byte(src[5]);
        // src[6..8] reserved
        state.length = length;
        state.total_length = HEADER_LEN + length as usize;
        state.header_parsed = true;
        tracing::trace!(target: "libbridgething::protocol::decode", "message length {}, compression {:?}, encoding {:?}, priority {:?}", state.length, state.compression, state.encoding, state.priority);
      }

      if src.len() < state.total_length {
        tracing::trace!(target: "libbridgething::protocol::decode", "message not complete ({}/{} bytes)", src.len(), state.total_length);
        state.packet += 1;
        return Ok(None);
      }

      return self.finish_frame(src);
    }
  }

  fn finish_frame<B: WireBuf>(&mut self, src: &mut B) -> Result<Option<PrioritizedFrame<In>>, EndecError> {
    let state = self.state.take().expect("finish_frame runs with a parsed header");
    src.skip(HEADER_LEN);
    let body = src.split_body(state.length as usize);

    let mut decompressed: Vec<u8> = Vec::new();
    let payload: &[u8] = if state.compression == Compression::Gzip {
      tracing::trace!(target: "libbridgething::protocol::decode", "decompressing gzip data");
      let mut decoder = GzDecoder::new(Cursor::new(&body[..])).take(MAX_FRAME_LEN as u64 + 1);
      decoder.read_to_end(&mut decompressed).map_err(EndecError::Decompress)?;
      if decompressed.len() > MAX_FRAME_LEN {
        tracing::warn!(target: "libbridgething::protocol::decode", "decompressed payload over the {MAX_FRAME_LEN} byte cap");
        return Err(EndecError::DecompressTooLarge { limit: MAX_FRAME_LEN });
      }
      tracing::trace!(target: "libbridgething::protocol::decode", "decompressed {} bytes", decompressed.len());
      &decompressed
    } else {
      tracing::trace!(target: "libbridgething::protocol::decode", "using uncompressed data");
      &body
    };

    tracing::trace!(target: "libbridgething::protocol::decode", "deserializing message with {} bytes", payload.len());

    if state.packet != 0 {
      let elapsed_time = state.message_start.elapsed();
      tracing::debug!(target: "libbridgething::protocol::decode", "network bytes: {}, total bytes: {}, elapsed {:?}", state.length, payload.len(), elapsed_time);
      tracing::trace!(target: "libbridgething::protocol::decode", "transfer rate: {:.2}mbps, effective rate: {:.2}mbps", mbps(elapsed_time, state.total_length as f64), mbps(elapsed_time, (HEADER_LEN + payload.len()) as f64));
    }

    let msg: In = match state.encoding {
      Encoding::Msgpack => match rmp_serde::from_slice(payload) {
        Ok(m) => m,
        Err(err) => {
          return Err(EndecError::TypedDecode {
            error: TypedDecodeError::Rmp(err),
            probe: Box::new(try_probe_envelope_msgpack(payload)),
          });
        }
      },
      Encoding::Json => match serde_json::from_slice(payload) {
        Ok(m) => m,
        Err(err) => {
          return Err(EndecError::TypedDecode {
            error: TypedDecodeError::Json(err),
            probe: Box::new(try_probe_envelope_json(payload)),
          });
        }
      },
    };
    tracing::trace!(target: "libbridgething::protocol::decode", "successfully decoded message");

    Ok(Some(PrioritizedFrame::new(state.priority, msg)))
  }
}

impl<In, Out> Decoder for WireEndec<In, Out>
where
  In: DeserializeOwned,
{
  type Item = DecodedFrame<In>;
  type Error = EndecError;

  fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
    self.decode_item(src)
  }
}

fn resync_to_magic<B: WireBuf>(src: &mut B) -> bool {
  let magic = MAGIC.to_be_bytes();
  if let Some(pos) = src.windows(magic.len()).position(|w| w == magic) {
    src.skip(pos);
    true
  } else {
    let drop = src.len().saturating_sub(magic.len() - 1);
    src.skip(drop);
    false
  }
}

impl<In, Out> Encoder<Out> for WireEndec<In, Out>
where
  Out: Serialize,
{
  type Error = EndecError;

  fn encode(&mut self, item: Out, dst: &mut BytesMut) -> Result<(), Self::Error> {
    encode_frame(Priority::Normal, Compress::Auto, &item, dst)
  }
}

impl<In, Out> Encoder<PrioritizedFrame<Out>> for WireEndec<In, Out>
where
  Out: Serialize,
{
  type Error = EndecError;

  fn encode(&mut self, item: PrioritizedFrame<Out>, dst: &mut BytesMut) -> Result<(), Self::Error> {
    encode_frame(item.priority, item.compress, &item.msg, dst)
  }
}

pub fn encode_frame<T: Serialize>(
  priority: Priority,
  compress: Compress,
  msg: &T,
  dst: &mut BytesMut,
) -> Result<(), EndecError> {
  tracing::trace!(target: "libbridgething::protocol::encode", "serializing message");
  let packed = rmp_serde::to_vec_named(msg).map_err(EndecError::RmpSerialization)?;
  let (compression, body) = compress_body(priority, compress, packed)?;
  let len = body.len() as u64;
  tracing::trace!(target: "libbridgething::protocol::encode", "serialized to {len} bytes, priority {priority:?}, compression {compression:?}");

  dst.put_u16(MAGIC);
  dst.put_u8(VERSION);
  dst.put_u8(match compression {
    Compression::Gzip => COMPRESSION_GZIP,
    Compression::None => COMPRESSION_NONE,
  });
  dst.put_u8(ENCODING_MSGPACK);
  dst.put_u8(priority.as_byte());
  dst.put_bytes(0, 2); // reserved
  dst.put_u64(len);

  dst.extend_from_slice(&body);
  Ok(())
}

fn compress_body(
  priority: Priority,
  compress: Compress,
  packed: Vec<u8>,
) -> Result<(Compression, Vec<u8>), EndecError> {
  let wanted = match compress {
    Compress::Never => return Ok((Compression::None, packed)),
    Compress::Always | Compress::IfSmaller => true,
    Compress::Auto => priority == Priority::Normal && packed.len() > AUTO_GZIP_THRESHOLD_BYTES,
  };
  if !wanted {
    return Ok((Compression::None, packed));
  }

  let zipped = gzip(&packed)?;
  if compress == Compress::Always || zipped.len() < packed.len() {
    Ok((Compression::Gzip, zipped))
  } else {
    Ok((Compression::None, packed))
  }
}

fn gzip(payload: &[u8]) -> Result<Vec<u8>, EndecError> {
  let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::fast());
  encoder.write_all(payload).map_err(EndecError::Compression)?;
  encoder.finish().map_err(EndecError::Compression)
}

#[cfg(test)]
fn decoded_bytes<In: DeserializeOwned, Out>(
  codec: &mut WireEndec<In, Out>,
  src: &mut Bytes,
) -> Option<PrioritizedFrame<In>> {
  codec
    .decode_bytes(src)
    .expect("decode_bytes did not error")
    .map(|item| item.frame().expect("a good frame, not a decode failure"))
}

#[cfg(test)]
fn decoded_frame<M, D: Decoder<Item = DecodedFrame<M>, Error = EndecError>>(
  codec: &mut D,
  buf: &mut BytesMut,
) -> Option<PrioritizedFrame<M>> {
  codec
    .decode(buf)
    .expect("decode did not error")
    .map(|item| item.frame().expect("a good frame, not a decode failure"))
}

#[cfg(test)]
mod bridge_tests {
  use futures::StreamExt;
  use tokio_util::codec::FramedRead;
  use uuid::Uuid;

  use super::*;
  use crate::{
    gateway::{AssetNotFoundReply, GatewayToBridgeAssetMsg, GatewayToBridgeMsg, GatewayToBridgeMsgData},
    wire::MsgMeta,
  };

  fn sample(asset_id: &str) -> GatewayToBridgeMsg {
    GatewayToBridgeMsg {
      id: Uuid::now_v7(),
      meta: MsgMeta::Request,
      data: GatewayToBridgeMsgData::Asset(GatewayToBridgeAssetMsg::NotFound(AssetNotFoundReply {
        id: asset_id.into(),
      })),
    }
  }

  fn frame_bytes(msg: &GatewayToBridgeMsg) -> Vec<u8> {
    let body = rmp_serde::to_vec_named(msg).unwrap();
    let mut out = BytesMut::new();
    out.put_u16(MAGIC);
    out.put_u8(VERSION);
    out.put_u8(COMPRESSION_NONE);
    out.put_u8(ENCODING_MSGPACK);
    out.put_u8(Priority::Normal.as_byte());
    out.put_bytes(0, 2);
    out.put_u64(body.len() as u64);
    out.extend_from_slice(&body);
    out.to_vec()
  }

  #[test]
  fn decodes_back_to_back_frames() {
    let mut codec = BridgeEndec::default();
    let (a, b) = (sample("art/a"), sample("art/b"));
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&frame_bytes(&a));
    buf.extend_from_slice(&frame_bytes(&b));
    assert_eq!(decoded_frame(&mut codec, &mut buf).expect("first").msg.id, a.id);
    assert_eq!(decoded_frame(&mut codec, &mut buf).expect("second").msg.id, b.id);
    assert!(codec.decode(&mut buf).unwrap().is_none());
  }

  #[test]
  fn header_split_across_reads_decodes() {
    let msg = sample("art/split");
    let bytes = frame_bytes(&msg);
    for split in 1..HEADER_LEN {
      let mut codec = BridgeEndec::default();
      let mut buf = BytesMut::new();
      buf.extend_from_slice(&bytes[..split]);
      assert!(
        codec.decode(&mut buf).unwrap().is_none(),
        "split {split}: partial header must yield no frame"
      );
      buf.extend_from_slice(&bytes[split..]);
      let frame = decoded_frame(&mut codec, &mut buf).unwrap_or_else(|| panic!("split {split}: frame lost"));
      assert_eq!(frame.msg.id, msg.id, "split {split}");
      assert!(buf.is_empty(), "split {split}: no residue");
    }
  }

  #[test]
  fn byte_at_a_time_stream_decodes() {
    let mut codec = BridgeEndec::default();
    let (a, b) = (sample("art/one"), sample("art/two"));
    let mut stream = frame_bytes(&a);
    stream.extend_from_slice(&frame_bytes(&b));
    let mut buf = BytesMut::new();
    let mut decoded = Vec::new();
    for byte in stream {
      buf.extend_from_slice(&[byte]);
      while let Some(frame) = decoded_frame(&mut codec, &mut buf) {
        decoded.push(frame.msg.id);
      }
    }
    assert_eq!(decoded, vec![a.id, b.id]);
  }

  #[test]
  fn resyncs_past_leading_garbage() {
    let mut codec = BridgeEndec::default();
    let msg = sample("art/x");
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0x01, 0x02, 0x03, 0xde, 0x00, 0xff]); // junk, incl a lone magic-hi byte
    buf.extend_from_slice(&frame_bytes(&msg));
    assert_eq!(
      decoded_frame(&mut codec, &mut buf).expect("frame after resync").msg.id,
      msg.id
    );
  }

  #[test]
  fn corrupt_frame_does_not_kill_the_stream() {
    let mut codec = BridgeEndec::default();
    let good = sample("art/good");
    let mut buf = BytesMut::new();
    buf.put_u16(MAGIC);
    buf.put_u8(VERSION);
    buf.put_u8(COMPRESSION_NONE);
    buf.put_u8(ENCODING_MSGPACK);
    buf.put_u8(Priority::Normal.as_byte());
    buf.put_bytes(0, 2);
    buf.put_u64(3);
    buf.extend_from_slice(&[0xff, 0xff, 0xff]); // a 3-byte non-msgpack body
    buf.extend_from_slice(&frame_bytes(&good));

    let first = codec.decode(&mut buf);
    assert!(
      matches!(
        &first,
        Ok(Some(DecodedFrame::Failed(e))) if matches!(e, EndecError::TypedDecode { .. })
      ),
      "a bad body is a failed item, not an Err that would end a Framed stream: {first:?}"
    );
    assert_eq!(
      decoded_frame(&mut codec, &mut buf).expect("recovered frame").msg.id,
      good.id
    );
  }

  fn header(compression: u8, body_len: usize) -> BytesMut {
    let mut out = BytesMut::new();
    out.put_u16(MAGIC);
    out.put_u8(VERSION);
    out.put_u8(compression);
    out.put_u8(ENCODING_MSGPACK);
    out.put_u8(Priority::Normal.as_byte());
    out.put_bytes(0, 2);
    out.put_u64(body_len as u64);
    out
  }

  async fn next_item(reader: &mut FramedRead<Cursor<Vec<u8>>, BridgeEndec>) -> DecodedFrame<GatewayToBridgeMsg> {
    reader
      .next()
      .await
      .expect("the stream is still alive")
      .expect("a decoder Err would end the stream for good")
  }

  #[tokio::test]
  async fn a_typed_decode_failure_does_not_end_a_framed_stream() {
    let good = sample("art/after-a-bad-body");
    let mut wire = header(COMPRESSION_NONE, 3);
    wire.extend_from_slice(&[0xff, 0xff, 0xff]); // a 3-byte non-msgpack body
    wire.extend_from_slice(&frame_bytes(&good));

    let mut reader = FramedRead::new(Cursor::new(wire.to_vec()), BridgeEndec::default());

    let failed = next_item(&mut reader).await;
    assert!(
      matches!(&failed, DecodedFrame::Failed(EndecError::TypedDecode { .. })),
      "a bad body rides the item so the stream survives: {failed:?}"
    );
    assert_eq!(
      next_item(&mut reader).await.frame().expect("the next frame").msg.id,
      good.id,
      "a frame after the failure still decodes"
    );
    assert!(reader.next().await.is_none(), "and the stream ends only at eof");
  }

  #[tokio::test]
  async fn an_over_cap_gzip_frame_is_rejected_without_ending_the_stream() {
    let bomb = gzip(&vec![0u8; MAX_FRAME_LEN + 1]).expect("gzip");
    assert!(
      bomb.len() < MAX_FRAME_LEN,
      "the compressed bomb has to fit under the frame cap to reach decompression"
    );

    let good = sample("art/after-a-bomb");
    let mut wire = header(COMPRESSION_GZIP, bomb.len());
    wire.extend_from_slice(&bomb);
    wire.extend_from_slice(&frame_bytes(&good));

    let mut reader = FramedRead::new(Cursor::new(wire.to_vec()), BridgeEndec::default());

    let failed = next_item(&mut reader).await;
    assert!(
      matches!(&failed, DecodedFrame::Failed(EndecError::DecompressTooLarge { limit }) if *limit == MAX_FRAME_LEN),
      "expansion is capped rather than trusted: {failed:?}"
    );
    assert_eq!(
      next_item(&mut reader).await.frame().expect("the next frame").msg.id,
      good.id,
      "a frame after the bomb still decodes"
    );
  }

  #[test]
  fn resync_to_magic_finds_next_frame_start() {
    let mut buf = BytesMut::from(&[0x11, 0x22, 0xde, 0xad, 0x99][..]);
    assert!(resync_to_magic(&mut buf));
    assert_eq!(&buf[..], &[0xde, 0xad, 0x99]);
  }

  #[test]
  fn resync_to_magic_keeps_tail_when_absent() {
    let mut buf = BytesMut::from(&[0x11, 0x22, 0x33, 0xde][..]);
    assert!(!resync_to_magic(&mut buf));
    assert_eq!(
      &buf[..],
      &[0xde],
      "keeps a trailing byte for a magic that straddles reads"
    );
  }

  #[test]
  fn decode_bytes_peels_every_frame_out_of_one_message() {
    let mut codec = BridgeEndec::default();
    let (a, b) = (sample("art/ws-a"), sample("art/ws-b"));
    let mut wire = frame_bytes(&a);
    wire.extend_from_slice(&frame_bytes(&b));
    let mut chunk = Bytes::from(wire);

    assert_eq!(decoded_bytes(&mut codec, &mut chunk).expect("first").msg.id, a.id);
    assert_eq!(decoded_bytes(&mut codec, &mut chunk).expect("second").msg.id, b.id);
    assert!(chunk.is_empty());
  }

  #[test]
  fn decode_bytes_reports_a_typed_decode_failure_as_an_item_too() {
    let mut codec = BridgeEndec::default();
    let good = sample("art/ws-after-a-bad-body");
    let mut wire = header(COMPRESSION_NONE, 3).to_vec();
    wire.extend_from_slice(&[0xff, 0xff, 0xff]); // a 3-byte non-msgpack body
    wire.extend_from_slice(&frame_bytes(&good));
    let mut chunk = Bytes::from(wire);

    let failed = codec
      .decode_bytes(&mut chunk)
      .expect("a recoverable failure is not an Err")
      .expect("an item");
    assert!(
      matches!(&failed, DecodedFrame::Failed(EndecError::TypedDecode { .. })),
      "decode_bytes reports failures the same way the Decoder impl does: {failed:?}"
    );
    assert_eq!(
      decoded_bytes(&mut codec, &mut chunk).expect("recovered frame").msg.id,
      good.id
    );
  }

  #[test]
  fn decode_bytes_resyncs_past_garbage() {
    let mut codec = BridgeEndec::default();
    let msg = sample("art/ws-resync");
    let mut wire = vec![0x01, 0x02, 0x03];
    wire.extend_from_slice(&frame_bytes(&msg));
    let mut chunk = Bytes::from(wire);

    assert_eq!(
      decoded_bytes(&mut codec, &mut chunk)
        .expect("frame after resync")
        .msg
        .id,
      msg.id
    );
  }
}

#[cfg(test)]
mod compression_tests {
  use super::*;
  use crate::{
    ForwardMessage, ForwardRouted,
    gateway::{BridgeToGatewayForwardMsg, BridgeToGatewayMsgData},
    wire::MsgMeta,
  };

  fn routed(bytes: Vec<u8>) -> BridgeToGatewayMsgData {
    BridgeToGatewayMsgData::Forward(BridgeToGatewayForwardMsg::Routed(ForwardRouted {
      webapp: uuid::Uuid::nil(),
      message: ForwardMessage::Binary(bytes),
    }))
  }

  fn frame_of(payload_len: usize, priority: Priority, compress: Compress) -> BytesMut {
    let msg = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: routed(vec![0x5a; payload_len]),
    };
    let mut wire = BytesMut::new();
    BridgeEndec::default()
      .encode(PrioritizedFrame::new(priority, msg).compressed(compress), &mut wire)
      .expect("encode");
    wire
  }

  const ENVELOPE_SLACK: usize = 256;

  #[test]
  fn a_normal_payload_under_the_threshold_stays_uncompressed() {
    let wire = frame_of(
      AUTO_GZIP_THRESHOLD_BYTES - ENVELOPE_SLACK,
      Priority::Normal,
      Compress::Auto,
    );
    assert_eq!(wire[3], COMPRESSION_NONE);
  }

  #[test]
  fn a_normal_payload_over_the_threshold_is_gzipped_when_that_is_smaller() {
    let wire = frame_of(
      AUTO_GZIP_THRESHOLD_BYTES + ENVELOPE_SLACK,
      Priority::Normal,
      Compress::Auto,
    );
    assert_eq!(wire[3], COMPRESSION_GZIP);
    let length = u64::from_be_bytes(wire[8..16].try_into().expect("8-byte slice")) as usize;
    assert!(
      length < AUTO_GZIP_THRESHOLD_BYTES,
      "an all-one-byte payload must compress well below the threshold, got {length}"
    );
  }

  #[test]
  fn gzip_is_the_normal_lane_only() {
    for priority in [Priority::Bulk, Priority::Background] {
      let wire = frame_of(AUTO_GZIP_THRESHOLD_BYTES + ENVELOPE_SLACK, priority, Compress::Auto);
      assert_eq!(wire[3], COMPRESSION_NONE, "{priority:?} must not auto-gzip");
    }
  }

  #[test]
  fn an_incompressible_payload_over_the_threshold_ships_raw() {
    let mut noise = vec![0u8; AUTO_GZIP_THRESHOLD_BYTES + ENVELOPE_SLACK];
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    for byte in noise.iter_mut() {
      state ^= state << 13;
      state ^= state >> 7;
      state ^= state << 17;
      *byte = state as u8;
    }
    let msg = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: routed(noise),
    };
    let mut wire = BytesMut::new();
    BridgeEndec::default()
      .encode(PrioritizedFrame::normal(msg), &mut wire)
      .expect("encode");
    assert_eq!(wire[3], COMPRESSION_NONE, "gzip only wins when it is smaller");
  }

  #[test]
  fn if_smaller_gzips_a_small_bulk_frame() {
    let wire = frame_of(4 * 1024, Priority::Bulk, Compress::IfSmaller);
    assert_eq!(
      wire[3], COMPRESSION_GZIP,
      "a compressible bulk fragment must gzip whatever the lane and the threshold say"
    );
  }

  #[test]
  fn if_smaller_ships_incompressible_bytes_raw() {
    let mut noise = vec![0u8; 4 * 1024];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for byte in noise.iter_mut() {
      state ^= state << 13;
      state ^= state >> 7;
      state ^= state << 17;
      *byte = state as u8;
    }
    let msg = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: routed(noise),
    };
    let mut wire = BytesMut::new();
    BridgeEndec::default()
      .encode(
        PrioritizedFrame::new(Priority::Bulk, msg).compressed(Compress::IfSmaller),
        &mut wire,
      )
      .expect("encode");
    assert_eq!(
      wire[3], COMPRESSION_NONE,
      "an icon that is already compressed must not pay for gzip"
    );
  }

  #[test]
  fn an_explicit_directive_beats_both_the_lane_and_the_threshold() {
    let forced = frame_of(64, Priority::Bulk, Compress::Always);
    assert_eq!(
      forced[3], COMPRESSION_GZIP,
      "always gzips a bulk frame under the threshold"
    );

    let refused = frame_of(
      AUTO_GZIP_THRESHOLD_BYTES + ENVELOPE_SLACK,
      Priority::Normal,
      Compress::Never,
    );
    assert_eq!(refused[3], COMPRESSION_NONE, "never gzips whatever the size");
  }

  #[test]
  fn a_gzipped_frame_round_trips_through_the_peer_decoder() {
    let payload = vec![0x5a; AUTO_GZIP_THRESHOLD_BYTES + ENVELOPE_SLACK];
    let msg = BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: routed(payload),
    };
    let mut wire = BytesMut::new();
    BridgeEndec::default()
      .encode(PrioritizedFrame::normal(msg.clone()), &mut wire)
      .expect("encode");
    assert_eq!(wire[3], COMPRESSION_GZIP);

    let decoded = decoded_frame(&mut GatewayEndec::default(), &mut wire).expect("a complete frame");
    assert_eq!(decoded.msg, msg);
    assert_eq!(decoded.priority, Priority::Normal);
    assert!(wire.is_empty());
  }
}

#[cfg(test)]
mod gateway_tests {
  use super::*;
  use crate::{
    gateway::{BridgeToGatewayMsgData, BridgeToGatewayTransferMsg, TransferAck},
    wire::MsgMeta,
  };

  fn sample() -> BridgeToGatewayMsg {
    BridgeToGatewayMsg {
      id: uuid::Uuid::now_v7(),
      meta: MsgMeta::Event,
      data: BridgeToGatewayMsgData::Transfer(BridgeToGatewayTransferMsg::Ack(TransferAck {
        transfer_id: uuid::Uuid::now_v7(),
        received: 4096,
      })),
    }
  }

  fn frame_bytes(msg: &BridgeToGatewayMsg) -> Vec<u8> {
    let body = rmp_serde::to_vec_named(msg).unwrap();
    let mut out = BytesMut::new();
    out.put_u16(MAGIC);
    out.put_u8(VERSION);
    out.put_u8(COMPRESSION_NONE);
    out.put_u8(ENCODING_MSGPACK);
    out.put_u8(Priority::Normal.as_byte());
    out.put_bytes(0, 2);
    out.put_u64(body.len() as u64);
    out.extend_from_slice(&body);
    out.to_vec()
  }

  #[test]
  fn header_split_across_reads_decodes() {
    let msg = sample();
    let bytes = frame_bytes(&msg);
    for split in 1..HEADER_LEN {
      let mut codec = GatewayEndec::default();
      let mut buf = BytesMut::new();
      buf.extend_from_slice(&bytes[..split]);
      assert!(
        codec.decode(&mut buf).unwrap().is_none(),
        "split {split}: partial header must yield no frame"
      );
      buf.extend_from_slice(&bytes[split..]);
      let frame = decoded_frame(&mut codec, &mut buf).unwrap_or_else(|| panic!("split {split}: frame lost"));
      assert_eq!(frame.msg.id, msg.id, "split {split}");
      assert!(buf.is_empty(), "split {split}: no residue");
    }
  }

  #[test]
  fn bad_magic_resyncs_instead_of_killing_the_connection() {
    let mut codec = GatewayEndec::default();
    let msg = sample();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0xff, 0x00, 0x11]);
    buf.extend_from_slice(&frame_bytes(&msg));
    assert_eq!(
      decoded_frame(&mut codec, &mut buf).expect("frame after resync").msg.id,
      msg.id
    );
  }

  #[test]
  fn an_over_cap_length_resyncs_without_overflowing() {
    let mut codec = GatewayEndec::default();
    let msg = sample();
    let mut buf = BytesMut::new();
    buf.put_u16(MAGIC);
    buf.put_u8(VERSION);
    buf.put_u8(COMPRESSION_NONE);
    buf.put_u8(ENCODING_MSGPACK);
    buf.put_u8(Priority::Normal.as_byte());
    buf.put_bytes(0, 2);
    buf.put_u64(u64::MAX);
    buf.extend_from_slice(&frame_bytes(&msg));

    assert_eq!(
      decoded_frame(&mut codec, &mut buf).expect("frame after resync").msg.id,
      msg.id
    );
  }
}

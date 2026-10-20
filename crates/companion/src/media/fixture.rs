pub fn gif(frames: usize, padding: usize) -> Vec<u8> {
  let mut out = b"GIF89a".to_vec();
  out.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]);
  out.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
  out.extend_from_slice(&[0x21, 0xFF, 0x0B]);
  out.extend_from_slice(b"NETSCAPE2.0");
  out.extend_from_slice(&[3, 1, 0, 0, 0]);
  for frame in 0..frames {
    out.extend_from_slice(&[0x21, 0xF9, 4, 0, 10, 0, 0, 0]);
    out.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
    out.extend_from_slice(&[2, 2, 0x44, 0x01, 0]);
    if frame == 0 && padding > 0 {
      out.extend_from_slice(&[0x21, 0xFE]);
      for comment in vec![b'.'; padding].chunks(255) {
        out.push(comment.len() as u8);
        out.extend_from_slice(comment);
      }
      out.push(0);
    }
  }
  out.push(0x3B);
  out
}

pub fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
  let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
  out.extend_from_slice(kind);
  out.extend_from_slice(body);
  out
}

pub fn ftyp(major: &[u8; 4], compatible: &[&[u8; 4]]) -> Vec<u8> {
  let mut brands = major.to_vec();
  brands.extend_from_slice(&[0, 0, 0, 0]);
  for brand in compatible {
    brands.extend_from_slice(*brand);
  }
  [boxed(b"ftyp", &brands), boxed(b"free", &[])].concat()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
  Video,
  Audio,
  Text,
}

impl Track {
  fn handler(self) -> &'static [u8; 4] {
    match self {
      Track::Video => b"vide",
      Track::Audio => b"soun",
      Track::Text => b"text",
    }
  }

  fn matroska_type(self) -> u8 {
    match self {
      Track::Video => 1,
      Track::Audio => 2,
      Track::Text => 17,
    }
  }

  fn trak(self) -> Vec<u8> {
    let mut handler = vec![0; 8];
    handler.extend_from_slice(self.handler());
    handler.extend_from_slice(&[0; 13]);
    boxed(
      b"trak",
      &[boxed(b"tkhd", &[0; 84]), boxed(b"mdia", &boxed(b"hdlr", &handler))].concat(),
    )
  }
}

pub fn mvhd(version: u8, timescale: u32, duration: u64) -> Vec<u8> {
  let mut header = vec![version, 0, 0, 0];
  if version == 1 {
    header.extend_from_slice(&[0; 16]);
    header.extend_from_slice(&timescale.to_be_bytes());
    header.extend_from_slice(&duration.to_be_bytes());
  } else {
    header.extend_from_slice(&[0; 8]);
    header.extend_from_slice(&timescale.to_be_bytes());
    header.extend_from_slice(&(duration as u32).to_be_bytes());
  }
  header.extend_from_slice(&[0; 80]);
  boxed(b"mvhd", &header)
}

pub fn moov(header: Vec<u8>, tracks: &[Track]) -> Vec<u8> {
  let tracks: Vec<u8> = tracks.iter().flat_map(|track| track.trak()).collect();
  boxed(b"moov", &[header, tracks].concat())
}

pub fn movie(duration_ms: u32, tracks: &[Track]) -> Vec<u8> {
  [
    ftyp(b"isom", &[b"isom", b"mp42"]),
    moov(mvhd(0, 1000, u64::from(duration_ms)), tracks),
  ]
  .concat()
}

pub fn movie_at_the_end(duration_ms: u32, tracks: &[Track], media_bytes: usize) -> Vec<u8> {
  [
    ftyp(b"isom", &[b"isom", b"mp42"]),
    boxed(b"mdat", &vec![0xAB; media_bytes]),
    moov(mvhd(0, 1000, u64::from(duration_ms)), tracks),
  ]
  .concat()
}

pub fn ebml(id: u32, body: &[u8]) -> Vec<u8> {
  let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect();
  out.push(0x01);
  out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
  out.extend_from_slice(body);
  out
}

pub fn ebml_open(id: u32, body: &[u8]) -> Vec<u8> {
  let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect();
  out.push(0xFF);
  out.extend_from_slice(body);
  out
}

pub fn matroska_head(doctype: &str) -> Vec<u8> {
  ebml(0x1A45_DFA3, &ebml(0x4282, doctype.as_bytes()))
}

pub fn matroska_info(duration_ms: f64) -> Vec<u8> {
  ebml(
    0x1549_A966,
    &[
      ebml(0x2A_D7B1, &[0x0F, 0x42, 0x40]),
      ebml(0x4489, &duration_ms.to_be_bytes()),
    ]
    .concat(),
  )
}

pub fn matroska_tracks(tracks: &[Track]) -> Vec<u8> {
  let entries: Vec<u8> = tracks
    .iter()
    .flat_map(|track| ebml(0xAE, &ebml(0x83, &[track.matroska_type()])))
    .collect();
  ebml(0x1654_AE6B, &entries)
}

pub fn matroska(doctype: &str, duration_ms: u32, tracks: &[Track]) -> Vec<u8> {
  let segment = [
    matroska_info(f64::from(duration_ms)),
    matroska_tracks(tracks),
    ebml(0x1F43_B675, &[0xE7, 0x81, 0x00]),
  ]
  .concat();
  [matroska_head(doctype), ebml(0x1853_8067, &segment)].concat()
}

pub fn recorded_matroska(doctype: &str, tracks: &[Track]) -> Vec<u8> {
  let segment = [
    ebml(0x1549_A966, &ebml(0x2A_D7B1, &[0x0F, 0x42, 0x40])),
    matroska_tracks(tracks),
    ebml_open(0x1F43_B675, &[0xE7, 0x81, 0x00, 0xA3, 0x84, 0x81, 0, 0, 0x80]),
  ]
  .concat();
  [matroska_head(doctype), ebml_open(0x1853_8067, &segment)].concat()
}

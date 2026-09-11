const METADATA_UNIT: usize = 16;
const IMAGE_EXTENSIONS: [&str; 5] = [".jpg", ".jpeg", ".png", ".webp", ".gif"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IcyMetadata {
  pub title: Option<String>,
  pub url: Option<String>,
}

impl IcyMetadata {
  pub fn artwork_url(&self) -> Option<&str> {
    let url = self.url.as_deref()?;
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    let image = (lower.starts_with("http://") || lower.starts_with("https://"))
      && IMAGE_EXTENSIONS.iter().any(|extension| path.ends_with(extension));
    image.then_some(url)
  }
}

pub struct IcyDemux {
  metaint: usize,
  until_metadata: usize,
  block: Vec<u8>,
  block_len: Option<usize>,
}

impl IcyDemux {
  pub fn new(metaint: usize) -> Self {
    let metaint = metaint.max(1);
    IcyDemux {
      metaint,
      until_metadata: metaint,
      block: Vec::new(),
      block_len: None,
    }
  }

  pub fn push(&mut self, mut chunk: &[u8], mut audio: impl FnMut(&[u8]), mut metadata: impl FnMut(IcyMetadata)) {
    while !chunk.is_empty() {
      if self.until_metadata > 0 {
        let take = chunk.len().min(self.until_metadata);
        audio(&chunk[..take]);
        self.until_metadata -= take;
        chunk = &chunk[take..];
        continue;
      }
      match self.block_len {
        None => {
          let len = usize::from(chunk[0]) * METADATA_UNIT;
          chunk = &chunk[1..];
          if len == 0 {
            self.until_metadata = self.metaint;
          } else {
            self.block_len = Some(len);
          }
        }
        Some(len) => {
          let take = (len - self.block.len()).min(chunk.len());
          self.block.extend_from_slice(&chunk[..take]);
          chunk = &chunk[take..];
          if self.block.len() == len {
            if let Some(parsed) = parse_block(&self.block) {
              metadata(parsed);
            }
            self.block.clear();
            self.block_len = None;
            self.until_metadata = self.metaint;
          }
        }
      }
    }
  }
}

fn parse_block(block: &[u8]) -> Option<IcyMetadata> {
  let text = String::from_utf8_lossy(block);
  let text = text.trim_end_matches('\0');
  let parsed = IcyMetadata {
    title: field(text, "StreamTitle='"),
    url: field(text, "StreamUrl='"),
  };
  (parsed != IcyMetadata::default()).then_some(parsed)
}

fn field(text: &str, key: &str) -> Option<String> {
  let start = text.find(key)? + key.len();
  let rest = &text[start..];
  let end = rest.find("';").or_else(|| rest.rfind('\'')).unwrap_or(rest.len());
  let value = rest[..end].trim();
  (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn metadata(payload: &str) -> Vec<u8> {
    let mut bytes = payload.as_bytes().to_vec();
    let units = bytes.len().div_ceil(METADATA_UNIT);
    bytes.resize(units * METADATA_UNIT, 0);
    let mut block = vec![units as u8];
    block.extend_from_slice(&bytes);
    block
  }

  fn demuxed(body: &[u8], metaint: usize, chunk: usize) -> (Vec<u8>, Vec<IcyMetadata>) {
    let mut demux = IcyDemux::new(metaint);
    let mut audio = Vec::new();
    let mut heard = Vec::new();
    for piece in body.chunks(chunk) {
      demux.push(piece, |bytes| audio.extend_from_slice(bytes), |meta| heard.push(meta));
    }
    (audio, heard)
  }

  fn titles(heard: &[IcyMetadata]) -> Vec<String> {
    heard.iter().filter_map(|meta| meta.title.clone()).collect()
  }

  fn body() -> Vec<u8> {
    let mut body = vec![b'a'; 16];
    body.extend_from_slice(&metadata(
      "StreamTitle='Neu! - Hallogallo';StreamUrl='http://example/';",
    ));
    body.extend_from_slice(&[b'b'; 16]);
    body.extend_from_slice(&metadata(""));
    body.extend_from_slice(&[b'c'; 8]);
    body
  }

  fn audio() -> Vec<u8> {
    let mut audio = vec![b'a'; 16];
    audio.extend_from_slice(&[b'b'; 16]);
    audio.extend_from_slice(&[b'c'; 8]);
    audio
  }

  #[test]
  fn a_framed_body_comes_out_without_its_metadata_blocks() {
    let (heard, meta) = demuxed(&body(), 16, 8192);
    assert_eq!(heard, audio());
    assert_eq!(titles(&meta), vec!["Neu! - Hallogallo".to_owned()]);
    assert_eq!(meta[0].url.as_deref(), Some("http://example/"));
  }

  #[test]
  fn a_metadata_block_split_across_pushes_is_still_stripped_whole() {
    for chunk in [1, 2, 3, 7, 17] {
      let (heard, meta) = demuxed(&body(), 16, chunk);
      assert_eq!(heard, audio(), "pushes of {chunk} bytes");
      assert_eq!(
        titles(&meta),
        vec!["Neu! - Hallogallo".to_owned()],
        "pushes of {chunk} bytes"
      );
    }
  }

  #[test]
  fn every_title_in_a_stream_is_reported_in_order() {
    let mut body = vec![b'a'; 4];
    body.extend_from_slice(&metadata("StreamTitle='first';"));
    body.extend_from_slice(&[b'b'; 4]);
    body.extend_from_slice(&metadata("StreamTitle='second';"));
    body.extend_from_slice(&[b'c'; 4]);
    body.extend_from_slice(&metadata(""));
    body.extend_from_slice(&[b'd'; 4]);

    let (heard, meta) = demuxed(&body, 4, 8192);
    assert_eq!(heard, b"aaaabbbbccccdddd");
    assert_eq!(titles(&meta), vec!["first".to_owned(), "second".to_owned()]);
  }

  #[test]
  fn a_body_that_ends_inside_a_metadata_block_keeps_the_audio_before_it() {
    let mut body = vec![b'a'; 8];
    body.extend_from_slice(&[2, b'S', b't']);

    let (heard, meta) = demuxed(&body, 8, 8192);
    assert_eq!(heard, b"aaaaaaaa");
    assert!(meta.is_empty());
  }

  #[test]
  fn a_stream_title_is_read_out_of_its_padded_block() {
    let title = |block: &[u8]| parse_block(block).and_then(|meta| meta.title);
    assert_eq!(
      title(&metadata("StreamTitle='Cluster - Caramel';StreamUrl='';")[1..]),
      Some("Cluster - Caramel".to_owned())
    );
    assert_eq!(
      title(b"StreamTitle='Rock 'n' Roll';StreamUrl='http://example/';"),
      Some("Rock 'n' Roll".to_owned())
    );
    assert_eq!(title(b"StreamTitle='';"), None);
    assert_eq!(title(b"StreamTitle='   ';"), None);
    assert_eq!(title(b"StreamUrl='http://example/';"), None);
    assert_eq!(parse_block(&[0u8; 16]), None);
  }

  #[test]
  fn only_a_stream_url_that_points_at_an_image_counts_as_artwork() {
    let with = |url: &str| IcyMetadata {
      title: None,
      url: Some(url.to_owned()),
    };
    assert_eq!(
      with("https://cdn.example/art/now.JPG?x=1").artwork_url(),
      Some("https://cdn.example/art/now.JPG?x=1")
    );
    assert_eq!(
      with("http://cdn.example/a.png").artwork_url(),
      Some("http://cdn.example/a.png")
    );
    assert_eq!(with("https://station.example/").artwork_url(), None);
    assert_eq!(with("https://station.example/now-playing").artwork_url(), None);
    assert_eq!(with("ftp://cdn.example/a.png").artwork_url(), None);
    assert_eq!(IcyMetadata::default().artwork_url(), None);
  }
}

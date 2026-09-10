use std::io::{ErrorKind, Read, Result};

const METADATA_UNIT: usize = 16;
const TITLE_KEY: &str = "StreamTitle='";
const TITLE_END: &str = "';";

const HLS_TYPES: [&str; 6] = [
  "application/vnd.apple.mpegurl",
  "application/x-mpegurl",
  "application/mpegurl",
  "audio/mpegurl",
  "audio/x-mpegurl",
  "vnd.apple.mpegurl",
];

pub struct IcyReader<R> {
  inner: R,
  metaint: usize,
  until_metadata: usize,
  on_title: Box<dyn FnMut(String) + Send + Sync>,
}

impl<R: Read> IcyReader<R> {
  pub fn new(inner: R, metaint: usize, on_title: Box<dyn FnMut(String) + Send + Sync>) -> Self {
    IcyReader {
      inner,
      metaint,
      until_metadata: metaint,
      on_title,
    }
  }

  fn read_metadata(&mut self) -> Result<bool> {
    let mut length = [0u8; 1];
    if !self.read_exactly(&mut length)? {
      return Ok(false);
    }
    let mut block = vec![0u8; usize::from(length[0]) * METADATA_UNIT];
    if !self.read_exactly(&mut block)? {
      return Ok(false);
    }
    self.until_metadata = self.metaint;
    if let Some(title) = stream_title(&block) {
      (self.on_title)(title);
    }
    Ok(true)
  }

  fn read_exactly(&mut self, buf: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
      match self.inner.read(&mut buf[filled..]) {
        Ok(0) => return Ok(false),
        Ok(read) => filled += read,
        Err(error) if error.kind() == ErrorKind::Interrupted => {}
        Err(error) => return Err(error),
      }
    }
    Ok(true)
  }
}

impl<R: Read> Read for IcyReader<R> {
  fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
    if buf.is_empty() {
      return Ok(0);
    }
    if self.until_metadata == 0 && !self.read_metadata()? {
      return Ok(0);
    }
    let want = buf.len().min(self.until_metadata);
    let read = self.inner.read(&mut buf[..want])?;
    self.until_metadata -= read;
    Ok(read)
  }
}

fn stream_title(block: &[u8]) -> Option<String> {
  let text = String::from_utf8_lossy(block);
  let text = text.trim_end_matches('\0');
  let start = text.find(TITLE_KEY)? + TITLE_KEY.len();
  let rest = &text[start..];
  let end = rest.find(TITLE_END).or_else(|| rest.rfind('\'')).unwrap_or(rest.len());
  let title = rest[..end].trim();
  (!title.is_empty()).then(|| title.to_owned())
}

pub fn is_hls(url: &str, content_type: Option<&str>) -> bool {
  let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
  if path.ends_with(".m3u8") {
    return true;
  }
  let Some(content_type) = content_type else {
    return false;
  };
  let mime = content_type
    .split(';')
    .next()
    .unwrap_or(content_type)
    .trim()
    .to_ascii_lowercase();
  HLS_TYPES.contains(&mime.as_str())
}

#[cfg(test)]
mod tests {
  use std::{
    io::Cursor,
    sync::{Arc, Mutex},
  };

  use super::*;

  struct Trickle {
    inner: Cursor<Vec<u8>>,
    chunk: usize,
  }

  impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
      let len = buf.len().min(self.chunk);
      self.inner.read(&mut buf[..len])
    }
  }

  fn metadata(payload: &str) -> Vec<u8> {
    let mut bytes = payload.as_bytes().to_vec();
    let units = bytes.len().div_ceil(METADATA_UNIT);
    bytes.resize(units * METADATA_UNIT, 0);
    let mut block = vec![units as u8];
    block.extend_from_slice(&bytes);
    block
  }

  fn stripped(body: Vec<u8>, metaint: usize, chunk: usize) -> (Vec<u8>, Vec<String>) {
    let titles = Arc::new(Mutex::new(Vec::new()));
    let heard = Arc::clone(&titles);
    let mut reader = IcyReader::new(
      Trickle {
        inner: Cursor::new(body),
        chunk,
      },
      metaint,
      Box::new(move |title| heard.lock().unwrap().push(title)),
    );
    let mut audio = Vec::new();
    reader.read_to_end(&mut audio).expect("the framed body reads");
    let titles = titles.lock().unwrap().clone();
    (audio, titles)
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
  fn a_framed_body_reaches_the_decoder_without_its_metadata_blocks() {
    let (heard, titles) = stripped(body(), 16, 8192);
    assert_eq!(heard, audio());
    assert_eq!(titles, vec!["Neu! - Hallogallo".to_owned()]);
  }

  #[test]
  fn a_metadata_block_split_across_reads_is_still_stripped_whole() {
    for chunk in [1, 2, 3, 7, 17] {
      let (heard, titles) = stripped(body(), 16, chunk);
      assert_eq!(heard, audio(), "inner reads of {chunk} bytes");
      assert_eq!(
        titles,
        vec!["Neu! - Hallogallo".to_owned()],
        "inner reads of {chunk} bytes"
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

    let (heard, titles) = stripped(body, 4, 8192);
    assert_eq!(heard, b"aaaabbbbccccdddd");
    assert_eq!(titles, vec!["first".to_owned(), "second".to_owned()]);
  }

  #[test]
  fn a_body_that_ends_inside_a_metadata_block_ends_the_stream() {
    let mut body = vec![b'a'; 8];
    body.extend_from_slice(&[2, b'S', b't']);

    let (heard, titles) = stripped(body, 8, 8192);
    assert_eq!(heard, b"aaaaaaaa");
    assert!(titles.is_empty());
  }

  #[test]
  fn a_stream_title_is_read_out_of_its_padded_block() {
    assert_eq!(
      stream_title(&metadata("StreamTitle='Cluster - Caramel';StreamUrl='';")[1..]),
      Some("Cluster - Caramel".to_owned())
    );
    assert_eq!(
      stream_title(b"StreamTitle='Rock 'n' Roll';StreamUrl='http://example/';"),
      Some("Rock 'n' Roll".to_owned())
    );
    assert_eq!(stream_title(b"StreamTitle='';"), None);
    assert_eq!(stream_title(b"StreamTitle='   ';"), None);
    assert_eq!(stream_title(b"StreamUrl='http://example/';"), None);
    assert_eq!(stream_title(&[0u8; 16]), None);
  }

  #[test]
  fn a_playlist_url_or_content_type_is_recognised_as_hls() {
    assert!(is_hls("https://example/live/master.m3u8", None));
    assert!(is_hls("https://example/live/master.M3U8?token=1", None));
    assert!(is_hls(
      "https://example/live/stream",
      Some("application/vnd.apple.mpegurl")
    ));
    assert!(is_hls(
      "https://example/live/stream",
      Some("audio/x-mpegurl; charset=utf-8")
    ));
    assert!(!is_hls("https://example/live/stream.mp3", Some("audio/mpeg")));
    assert!(!is_hls("https://example/live/stream", None));
  }
}

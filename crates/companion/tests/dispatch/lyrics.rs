use std::sync::{
  Arc,
  atomic::{AtomicUsize, Ordering},
};

use bridgething_companion::{
  dispatch::lyrics::{LyricsDispatcher, LyricsResolver},
  provider::ProviderError,
};
use bridgething_gateway::{HandlerError, LyricsHandler};
use libbridgething::{
  LyricLine, Lyrics,
  gateway::{LyricsRequest, TrackIdentity},
};

use crate::{
  fakes::{FakeProvider, FakeRegistry},
  responses::Responses,
};

#[derive(Default)]
struct CannedResolver {
  lyrics: Option<Lyrics>,
  hits: AtomicUsize,
}

#[async_trait::async_trait]
impl LyricsResolver for CannedResolver {
  async fn lyrics(&self, _track: &TrackIdentity) -> Option<Lyrics> {
    self.hits.fetch_add(1, Ordering::SeqCst);
    self.lyrics.clone()
  }
}

fn track() -> TrackIdentity {
  TrackIdentity {
    artist: "Daft Punk".into(),
    track: "One More Time".into(),
    album: None,
    duration_ms: None,
    isrc: None,
  }
}

#[tokio::test]
async fn lyrics_fall_through_to_the_resolver_when_no_provider_has_them() {
  let resolver = Arc::new(CannedResolver {
    lyrics: Some(Lyrics {
      synced: Some(vec![LyricLine {
        start_ms: 0,
        text: "one more time".into(),
      }]),
      plain: None,
      source: "fake-resolver".into(),
    }),
    hits: AtomicUsize::new(0),
  });
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(
    FakeRegistry::with(FakeProvider::bare("spotify")),
    resolver.clone(),
    responses.cache(),
  );

  let reply = dispatch
    .get(LyricsRequest { track: track() })
    .await
    .expect("the lyrics resolved");

  let lyrics = reply.response.lyrics.expect("the resolver's lyrics");
  assert_eq!(lyrics.source, "fake-resolver");
  assert_eq!(lyrics.synced.as_ref().expect("synced")[0].text, "one more time");
  assert_eq!(resolver.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_provider_that_has_lyrics_wins_over_the_resolver() {
  let provider = Arc::new(FakeProvider {
    on_lyrics: Some(Box::new(|_| {
      Ok(Some(Lyrics {
        synced: None,
        plain: Some("from the provider".into()),
        source: "spotify".into(),
      }))
    })),
    ..FakeProvider::named("spotify")
  });
  let resolver = Arc::new(CannedResolver::default());
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(FakeRegistry::with(provider), resolver.clone(), responses.cache());

  let reply = dispatch
    .get(LyricsRequest { track: track() })
    .await
    .expect("the lyrics resolved");

  assert_eq!(reply.response.lyrics.expect("provider lyrics").source, "spotify");
  assert_eq!(
    resolver.hits.load(Ordering::SeqCst),
    0,
    "the resolver is the fallback, not a second opinion"
  );
}

#[tokio::test]
async fn no_hit_anywhere_is_an_empty_reply_rather_than_an_error() {
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(
    FakeRegistry::with(FakeProvider::bare("spotify")),
    Arc::new(CannedResolver::default()),
    responses.cache(),
  );

  let reply = dispatch
    .get(LyricsRequest { track: track() })
    .await
    .expect("a miss still answers");
  assert_eq!(reply.response.lyrics, None);
}

#[tokio::test]
async fn a_provider_lyrics_failure_answers_a_typed_error() {
  let provider = Arc::new(FakeProvider {
    on_lyrics: Some(Box::new(|_| Err(ProviderError::Failed("lrclib down".into())))),
    ..FakeProvider::named("spotify")
  });
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(
    FakeRegistry::with(provider),
    Arc::new(CannedResolver::default()),
    responses.cache(),
  );

  let refused = dispatch
    .get(LyricsRequest { track: track() })
    .await
    .expect_err("a failed resolve is a refusal");

  match refused {
    HandlerError::Domain(reply) => assert!(reply.message.contains("lrclib down"), "got {}", reply.message),
    other => panic!("a failed lyrics resolve is a domain error, not {other:?}"),
  }
}

#[tokio::test]
async fn a_provider_that_does_not_do_lyrics_falls_through_rather_than_failing() {
  let provider = Arc::new(FakeProvider {
    on_lyrics: Some(Box::new(|_| Err(ProviderError::NotImplemented))),
    ..FakeProvider::named("spotify")
  });
  let resolver = Arc::new(CannedResolver {
    lyrics: Some(Lyrics {
      synced: None,
      plain: Some("from the resolver".into()),
      source: "fake-resolver".into(),
    }),
    hits: AtomicUsize::new(0),
  });
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(FakeRegistry::with(provider), resolver.clone(), responses.cache());

  let reply = dispatch
    .get(LyricsRequest { track: track() })
    .await
    .expect("the resolver answered");

  assert_eq!(reply.response.lyrics.expect("resolver lyrics").source, "fake-resolver");
  assert_eq!(resolver.hits.load(Ordering::SeqCst), 1);
}

mod lrclib {
  use std::sync::{Arc, Mutex};

  use bridgething_companion::{dispatch::lyrics::LyricsResolver, lyrics::lrclib::LrclibResolver};
  use bridgething_io::{HttpDownloadSink, HttpExecutor, HttpRequest, HttpResponse, HttpSink, HttpTransport};
  use libbridgething::gateway::TrackIdentity;

  struct CannedHttp {
    status: u16,
    body: &'static str,
    urls: Mutex<Vec<String>>,
  }

  impl CannedHttp {
    fn new(status: u16, body: &'static str) -> Arc<Self> {
      Arc::new(Self {
        status,
        body,
        urls: Mutex::new(Vec::new()),
      })
    }
  }

  impl HttpTransport for CannedHttp {
    fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
      self.urls.lock().unwrap().push(request.url);
      sink.complete(HttpResponse {
        status: self.status,
        headers: Vec::new(),
        body: self.body.as_bytes().to_vec(),
      });
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("no downloads in these cases".into());
    }
  }

  fn track() -> TrackIdentity {
    TrackIdentity {
      artist: "almost monday".into(),
      track: "better late than never".into(),
      album: Some("better late than never".into()),
      duration_ms: Some(225_233),
      isrc: None,
    }
  }

  const HIT: &str = "{\"id\":1,\"syncedLyrics\":\"[00:12.34] the first line\\r\\n[00:15.67] the second line\\r\\n[00:19.00] the third line\",\"plainLyrics\":\"the first line\\nthe second line\\nthe third line\"}";

  #[tokio::test]
  async fn a_crlf_served_lrc_arrives_as_separate_synced_lines() {
    let http = CannedHttp::new(200, HIT);
    let resolver = LrclibResolver::new(HttpExecutor::new(http.clone()));

    let lyrics = resolver.lyrics(&track()).await.expect("the lookup hit");
    let synced = lyrics.synced.expect("synced lines survive");
    assert_eq!(synced.len(), 3, "a crlf-joined lrc must not collapse, got {synced:?}");
    assert_eq!(synced[0].text, "the first line");
    assert_eq!(synced[1].start_ms, 15_670);
    assert_eq!(lyrics.source, "lrclib");

    let urls = http.urls.lock().unwrap();
    assert_eq!(urls.len(), 1);
    assert!(
      urls[0].starts_with("https://lrclib.net/api/get?artist_name=almost+monday&track_name=better+late+than+never"),
      "the signature lookup carries the identity, got {}",
      urls[0]
    );
    assert!(
      urls[0].contains("duration=225"),
      "duration rounds to seconds, got {}",
      urls[0]
    );
  }

  #[tokio::test]
  async fn a_missing_track_is_a_quiet_none() {
    let http = CannedHttp::new(404, "{\"statusCode\":404,\"name\":\"TrackNotFound\"}");
    let resolver = LrclibResolver::new(HttpExecutor::new(http));

    assert!(resolver.lyrics(&track()).await.is_none());
  }
}

#[tokio::test]
async fn lyrics_found_once_are_served_from_the_phone_after_that() {
  let resolver = Arc::new(CannedResolver {
    lyrics: Some(Lyrics {
      synced: None,
      plain: Some("one more time".into()),
      source: "fake-resolver".into(),
    }),
    hits: AtomicUsize::new(0),
  });
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(
    FakeRegistry::with(FakeProvider::bare("spotify")),
    resolver.clone(),
    responses.cache(),
  );

  for _ in 0..3 {
    let reply = dispatch.get(LyricsRequest { track: track() }).await.unwrap();
    assert_eq!(reply.response.lyrics.unwrap().plain.as_deref(), Some("one more time"));
  }

  assert_eq!(resolver.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_lyrics_miss_is_asked_again_next_time() {
  let resolver = Arc::new(CannedResolver::default());
  let responses = Responses::new();
  let dispatch = LyricsDispatcher::new(
    FakeRegistry::with(FakeProvider::bare("spotify")),
    resolver.clone(),
    responses.cache(),
  );

  dispatch.get(LyricsRequest { track: track() }).await.unwrap();
  dispatch.get(LyricsRequest { track: track() }).await.unwrap();

  assert_eq!(resolver.hits.load(Ordering::SeqCst), 2);
}

use std::{collections::BTreeMap, error::Error, path::PathBuf, sync::Arc, time::Duration};

use bridgething_companion::{
  api::VoiceModelPaths,
  provider::spotify::{catalog_intent, names_nothing, voice_request},
  voice::{
    fast_path,
    inference::{BundleInference, NluInference},
    rejection::{RejectionOutcome, evaluate},
  },
};
use bridgething_desktop::{backends::Platform, shell::DesktopPaths};
use libbridgething::NluSlots;
use serde::Deserialize;
use spotify::{
  auth::{Auth, DEFAULT_WORKER_BASE},
  client::{Observer, SpotifyClient},
  http::random_hex,
  httpx,
  model::{AuthState, Device, LibraryScope, PlayerState, Queue},
  resolver::{VoiceResolved, VoiceTargetKind, made_for_you, norm},
  store::FileTokenStore,
};

type Boxed = Box<dyn Error>;

const USAGE: &str = "\
usage: resolver-eval [--rows <file>] [--bundle <dir>] [--lanes r,m] [--gen-library <n>] [--pace-ms <n>]

read-only voice resolver eval";

#[derive(Deserialize)]
struct Row {
  id: String,
  family: String,
  #[serde(default)]
  utterance: Option<String>,
  #[serde(default)]
  asr: Option<String>,
  #[serde(default)]
  slots: Option<NluSlots>,
  #[serde(default)]
  expect: Expect,
}

#[derive(Deserialize, Default, Clone)]
struct Expect {
  #[serde(default)]
  uri: Option<String>,
  #[serde(default)]
  kind: Option<String>,
  #[serde(default)]
  title: Option<String>,
  #[serde(default)]
  title_has: Option<String>,
  #[serde(default)]
  title_lacks: Option<String>,
  #[serde(default)]
  artist: Option<String>,
  #[serde(default)]
  year: Option<i32>,
  #[serde(default)]
  made_for_you: Option<bool>,
  #[serde(default)]
  dead_end: Option<bool>,
}

enum Verdict {
  Correct,
  Wrong(String),
  Dead(String),
}

struct Args {
  rows: PathBuf,
  bundle: Option<PathBuf>,
  lanes: Vec<char>,
  gen_library: usize,
  pace: Duration,
}

struct Silent;
impl Observer for Silent {
  fn on_player(&self, _state: PlayerState) {}
  fn on_queue(&self, _queue: Queue) {}
  fn on_devices(&self, _devices: Vec<Device>) {}
  fn on_auth(&self, _state: AuthState) {}
  fn on_library_changed(&self, _scope: LibraryScope) {}
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Boxed> {
  bridgething_io::install_crypto_provider();
  let args = parse(std::env::args().skip(1).collect())?;
  let mut rows = load_rows(&args.rows)?;

  let inference = if args.lanes.contains(&'m') {
    let bundle = match args.bundle.clone() {
      Some(dir) => dir,
      None => DesktopPaths::xdg()?
        .installed_nlu_bundle()
        .ok_or("no installed nlu bundle; open the desktop app to download one, or pass --bundle")?,
    };
    let platform = Platform::detect(&DesktopPaths::xdg()?.config_dir);
    let runner = platform.nlu.ok_or("this host has no nlu model runner")?;
    let armed = bundle.clone();
    platform.models.answered_by(move || VoiceModelPaths {
      nlu_bundle_dir: Some(armed.display().to_string()),
      asr_weights: None,
    });
    Some(BundleInference::load(&bundle, runner)?)
  } else {
    None
  };

  let client = connect().await?;
  if args.gen_library > 0 {
    let generated = library_rows(&client, args.gen_library, args.pace).await;
    println!("library rows generated: {}", generated.len());
    rows.extend(generated);
  }

  let mut tally: BTreeMap<(String, String), (u32, u32, u32)> = BTreeMap::new();
  for row in &rows {
    if args.lanes.contains(&'r')
      && let Some(slots) = &row.slots
    {
      let verdict = run_slots(&client, slots, &row.expect, args.pace).await;
      record(&mut tally, row, "R", "gold", &verdict);
    }
    if let Some(inference) = &inference {
      let policy = inference.rejection().unwrap_or_default();
      for (form, text) in [("clean", &row.utterance), ("asr", &row.asr)] {
        let Some(text) = text else { continue };
        let verdict = match model_slots(inference, policy, text).await {
          Ok((intent, slots)) if catalog_intent(&intent) => run_slots(&client, &slots, &row.expect, args.pace).await,
          Ok((intent, _)) => Verdict::Wrong(format!("intent {intent}")),
          Err(stage) => dead(&row.expect, stage),
        };
        record(&mut tally, row, "M", form, &verdict);
      }
    }
  }

  client.disconnect().await;
  println!("\nfamily                      lane   rows  correct  wrong  dead");
  for ((family, lane), (correct, wrong, deadends)) in &tally {
    let rows = correct + wrong + deadends;
    println!("{family:<28}{lane:<7}{rows:<6}{correct:<9}{wrong:<7}{deadends}");
  }
  Ok(())
}

fn parse(args: Vec<String>) -> Result<Args, Boxed> {
  let mut out = Args {
    rows: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("eval/resolver-rows.jsonl"),
    bundle: None,
    lanes: vec!['r', 'm'],
    gen_library: 0,
    pace: Duration::from_millis(400),
  };
  let mut rest = args.into_iter();
  while let Some(arg) = rest.next() {
    let mut value = || rest.next().ok_or(USAGE);
    match arg.as_str() {
      "--rows" => out.rows = PathBuf::from(value()?),
      "--bundle" => out.bundle = Some(PathBuf::from(value()?)),
      "--lanes" => out.lanes = value()?.chars().filter(|c| *c != ',').collect(),
      "--gen-library" => out.gen_library = value()?.parse()?,
      "--pace-ms" => out.pace = Duration::from_millis(value()?.parse()?),
      _ => return Err(USAGE.into()),
    }
  }
  Ok(out)
}

fn load_rows(path: &PathBuf) -> Result<Vec<Row>, Boxed> {
  let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
  text
    .lines()
    .filter(|l| !l.trim().is_empty())
    .map(|l| serde_json::from_str(l).map_err(|e| format!("bad row {l:?}: {e}").into()))
    .collect()
}

async fn connect() -> Result<SpotifyClient, Boxed> {
  let psk = std::env::var("SPOTIFY_AUTH_PSK").map_err(|_| "SPOTIFY_AUTH_PSK is required")?;
  let base = std::env::var("SPOTIFY_AUTH_BASE").unwrap_or_else(|_| DEFAULT_WORKER_BASE.to_string());
  let dir = std::env::var("SPOTIFY_PRIVATE_STATE").unwrap_or_else(|_| ".spotify-private".to_string());
  let store = FileTokenStore::new(&dir)?;
  let exec = httpx::executor();
  let auth = Arc::new(Auth::new(base, psk, Box::new(store), exec.clone()));
  if !auth.is_paired().await {
    return Err(format!("no paired refresh token in {dir}; run `sfp pair` first").into());
  }
  let client = SpotifyClient::new(auth, random_hex(20), exec, Arc::new(Silent));
  client.connect().await?;
  Ok(client)
}

async fn model_slots(
  inference: &BundleInference,
  policy: bridgething_companion::voice::rejection::RejectionPolicy,
  text: &str,
) -> Result<(String, NluSlots), String> {
  if let Some(hit) = fast_path::match_transcript(text) {
    return Ok((hit.intent.to_string(), hit.slots));
  }
  let output = inference.infer(text).await.map_err(|e| format!("infer: {e}"))?;
  match evaluate(&output, policy).map_err(|e| format!("rejection: {e}"))? {
    RejectionOutcome::Accept { intent } => Ok((intent.to_string(), output.slots)),
    RejectionOutcome::NoIntent => Err("NO_INTENT".to_string()),
    RejectionOutcome::Clarify { .. } => Err("CLARIFY".to_string()),
  }
}

async fn run_slots(client: &SpotifyClient, slots: &NluSlots, expect: &Expect, pace: Duration) -> Verdict {
  let request = voice_request(slots);
  if names_nothing(&request) {
    return dead(expect, "names nothing".to_string());
  }
  tokio::time::sleep(pace).await;
  match client.resolve_voice(request).await {
    Ok(out) if expect.dead_end == Some(true) => Verdict::Wrong(format!("resolved {} but expected a dead end", out.uri)),
    Ok(out) => match holds(expect, &out) {
      Ok(()) => Verdict::Correct,
      Err(why) => Verdict::Wrong(format!("{why}; got {} {:?} ({:?})", out.uri, out.display, out.kind)),
    },
    Err(e) => dead(expect, e.to_string()),
  }
}

fn dead(expect: &Expect, why: String) -> Verdict {
  match expect.dead_end == Some(true) {
    true => Verdict::Correct,
    false => Verdict::Dead(why),
  }
}

fn holds(expect: &Expect, out: &VoiceResolved) -> Result<(), String> {
  if let Some(uri) = &expect.uri
    && out.uri != *uri
  {
    return Err(format!("uri != {uri}"));
  }
  if let Some(kind) = &expect.kind
    && kind_name(out.kind) != kind.as_str()
  {
    return Err(format!("kind != {kind}"));
  }
  if let Some(title) = &expect.title
    && norm(&out.display) != norm(title)
  {
    return Err(format!("title != {title:?}"));
  }
  if let Some(part) = &expect.title_has
    && !format!(" {} ", norm(&out.display)).contains(&format!(" {} ", norm(part)))
  {
    return Err(format!("title lacks {part:?}"));
  }
  if let Some(part) = &expect.title_lacks
    && format!(" {} ", norm(&out.display)).contains(&format!(" {} ", norm(part)))
  {
    return Err(format!("title carries {part:?}"));
  }
  if let Some(artist) = &expect.artist {
    match &out.artist {
      Some(got) if norm(got) == norm(artist) => {}
      got => return Err(format!("artist {got:?} != {artist:?}")),
    }
  }
  if let Some(year) = expect.year
    && out.year != Some(year)
  {
    return Err(format!("year {:?} != {year}", out.year));
  }
  if let Some(want) = expect.made_for_you
    && made_for_you(&out.uri) != want
  {
    return Err(format!("made_for_you != {want}"));
  }
  Ok(())
}

fn kind_name(kind: VoiceTargetKind) -> &'static str {
  match kind {
    VoiceTargetKind::Track => "track",
    VoiceTargetKind::Album => "album",
    VoiceTargetKind::Artist => "artist",
    VoiceTargetKind::Playlist => "playlist",
    VoiceTargetKind::Show => "show",
    VoiceTargetKind::Episode => "episode",
    VoiceTargetKind::Station => "station",
  }
}

fn record(tally: &mut BTreeMap<(String, String), (u32, u32, u32)>, row: &Row, lane: &str, form: &str, v: &Verdict) {
  let cell = tally.entry((row.family.clone(), lane.to_string())).or_default();
  match v {
    Verdict::Correct => cell.0 += 1,
    Verdict::Wrong(why) => {
      cell.1 += 1;
      println!("MISS  {} [{lane}/{form}] wrong-action: {why}", row.id);
    }
    Verdict::Dead(why) => {
      cell.2 += 1;
      println!("MISS  {} [{lane}/{form}] dead-end: {why}", row.id);
    }
  }
}

async fn library_rows(client: &SpotifyClient, cap: usize, pace: Duration) -> Vec<Row> {
  let mut rows = Vec::new();
  for node in ["albums", "playlists", "artists"] {
    tokio::time::sleep(pace).await;
    let Ok(page) = client.browse(node, cap as u32, 0).await else {
      continue;
    };
    let node_start = rows.len();
    for item in &page.items {
      if rows.len() - node_start >= cap {
        break;
      }
      let title = item.title.trim().to_string();
      let artist = item.artists.first().map(|a| a.name.trim().to_string());
      if title.is_empty() {
        continue;
      }
      match item.uri.split(':').nth(1) {
        Some("album") => {
          rows.push(generated_row(
            format!("lib-album-{}", rows.len()),
            "lib-album",
            format!("play the album {title}"),
            slots(Some(&title), Some("album")),
            Expect {
              kind: Some("album".into()),
              title: Some(title.clone()),
              ..Default::default()
            },
          ));
          if let Some(artist) = artist {
            rows.push(generated_row(
              format!("lib-album-by-{}", rows.len()),
              "lib-album",
              format!("play {title} by {artist}"),
              slots(Some(&format!("{title} by {artist}")), Some("album")),
              Expect {
                kind: Some("album".into()),
                title: Some(title.clone()),
                ..Default::default()
              },
            ));
          }
        }
        Some("playlist") => rows.push(generated_row(
          format!("lib-playlist-{}", rows.len()),
          "lib-playlist",
          format!("play the playlist {title}"),
          slots(Some(&title), Some("playlist")),
          Expect {
            kind: Some("playlist".into()),
            title: Some(title.clone()),
            ..Default::default()
          },
        )),
        Some("artist") => rows.push(generated_row(
          format!("lib-artist-{}", rows.len()),
          "lib-artist",
          format!("play the artist {title}"),
          slots(Some(&title), Some("artist")),
          Expect {
            kind: Some("artist".into()),
            title: Some(title.clone()),
            ..Default::default()
          },
        )),
        _ => {}
      }
    }
  }
  rows
}

fn generated_row(id: String, family: &str, utterance: String, slots: NluSlots, expect: Expect) -> Row {
  Row {
    id,
    family: family.to_string(),
    asr: Some(norm(&utterance)),
    utterance: Some(utterance),
    slots: Some(slots),
    expect,
  }
}

fn slots(target: Option<&str>, kind: Option<&str>) -> NluSlots {
  let typed = kind.map(|k| serde_json::from_value(serde_json::Value::String(k.to_string())).expect("a wire kind"));
  NluSlots {
    target: target.map(str::to_string),
    target_type: typed,
    ..Default::default()
  }
}

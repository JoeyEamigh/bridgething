use std::{error::Error, path::PathBuf, sync::Arc, time::Duration};

use bridgething_io::{HttpExecutor, HttpMethod};
use librespot_protocol::{
  client_info::ClientInfo,
  connect::Cluster,
  credentials::OneTimeToken,
  login5::{LoginRequest, LoginResponse, login_request::Login_method, login_response::Response as Login5Response},
  player::ProvidedTrack,
};
use protobuf::{Message, MessageField};
use spotify::{
  auth::{Auth, DEFAULT_WORKER_BASE, TokenStore},
  client::{Observer, SpotifyClient},
  dealer::{Dealer, active_device, is_queued, provided_track_from_json, provided_track_json},
  http::{ANDROID_CLIENT_ID, SPCLIENT, SpHttp, random_hex},
  httpx,
  model::{AuthState, Device, LibraryScope, PlayerState, Queue, QueuePosition},
  resolver::{VoicePopularity, VoiceResolveRequest, VoiceResolved, VoiceTargetKind},
  spclient::SpClient,
  store::{FileTokenStore, load_or_make_device_id},
  util::image_hex,
};

type Boxed = Box<dyn Error>;

const USAGE: &str = "\
commands:
  probe | np | devices | watch [secs] | product | whoami | apwhoami | pair
  home | stations [filter] | search <query> | root | lib [node] | fav
  ctx <uri>        raw context-resolve of one spotify uri
  page <url>       follow a context page url
  autoplay <uri>   what spotify would autoplay after a context
  resolve <query...> [--type kind] [--position n] [--mood m] [--genre g] [--era e]
                     [--filter top5|top10|popular|recent|new|first|random] [--random]
                   voice slot resolution to a playable uri (read-only)
  pause | resume | next | prev | seek <ms> | play <uri>
  queue [show]     print queue_revision and the upcoming tracks
  queue dump <f>   save revision + next/prev tracks to a json file
  queue restore <f>  set_queue from a dump, re-stamped with the live revision
  queue add <uri>  append (lands after the tracks already queued)
  queue add-at <n> <uri>  insert at <n> of the upcoming list a webapp sees (delimiters skipped)";

#[tokio::main]
async fn main() -> Result<(), Boxed> {
  let args: Vec<String> = std::env::args().collect();
  let cmd = args.get(1).map(String::as_str).unwrap_or("probe");

  let psk =
    std::env::var("SPOTIFY_AUTH_PSK").map_err(|_| "SPOTIFY_AUTH_PSK is required (gates the private-auth worker)")?;
  let base = std::env::var("SPOTIFY_AUTH_BASE").unwrap_or_else(|_| DEFAULT_WORKER_BASE.to_string());
  let state_dir =
    PathBuf::from(std::env::var("SPOTIFY_PRIVATE_STATE").unwrap_or_else(|_| ".spotify-private".to_string()));
  let username = std::env::var("SPOTIFY_USERNAME").ok();

  let store = FileTokenStore::new(&state_dir)?;
  if let Ok(seed) = std::env::var("SPOTIFY_CARTHING_REFRESH_TOKEN")
    && !seed.is_empty()
    && store.load_refresh_token().is_none()
  {
    store.save_refresh_token(seed);
  }
  let device_id = load_or_make_device_id(&state_dir);
  let exec = httpx::executor();
  let auth = Arc::new(Auth::new(base, psk, Box::new(store), exec.clone()));

  if !auth.is_paired().await {
    eprintln!("not paired; starting device-code flow...");
    pair(&auth).await?;
  }

  let http = SpHttp::new(auth.clone(), exec.clone());
  let spc = SpClient::new(http.clone());
  let dealer = Dealer::new(http.clone(), device_id);

  let username = match username {
    Some(u) => Some(u),
    None => spotify::aplogin::resolve_and_cache(&auth, &http, dealer.device_id())
      .await
      .ok(),
  };

  match cmd {
    "pair" => println!("paired."),
    "probe" => probe(&spc, &dealer, username.as_deref()).await?,
    "np" => {
      let (_stream, writer) = dealer.open().await?;
      println!("{}", describe_np(&writer.cluster().await?));
    }
    "home" => {
      let home = spc.get_home("en").await?;
      println!("home: {} sections", home.body.sections.len());
      for s in home.body.sections.iter().take(20) {
        let car = pick_carousel(s);
        println!("  [{}] {:?}  {} items", section_kind(s), car.0, car.1);
      }
    }
    "stations" => {
      let filter = args.get(2).map(String::as_str).unwrap_or("station");
      let home = spc.get_home("en").await?;
      for s in &home.body.sections {
        let kind = section_kind(s);
        if !filter.is_empty() && !kind.contains(filter) {
          continue;
        }
        let (title, uris) = carousel_items(s);
        println!("[{kind}] {title:?}  {} items", uris.len());
        for u in &uris {
          println!("    {u}");
        }
      }
    }
    "ctx" => {
      let uri = args.get(2).map(String::as_str).ok_or("ctx needs a context uri")?;
      print_context(&spc.context_resolve(uri).await?);
    }
    "resolve" => {
      let req = parse_resolve(&args[2..])?;
      let client = SpotifyClient::new(
        auth.clone(),
        dealer.device_id().to_string(),
        exec.clone(),
        Arc::new(PrintObserver::default()),
      );
      client.connect().await?;
      print_resolved(&client.resolve_voice(req).await?);
      client.disconnect().await;
    }
    "page" => {
      let uri = args.get(2).map(String::as_str).ok_or("page needs a page url")?;
      print_page(&spc.context_page(uri).await?);
    }
    "autoplay" => {
      let uri = args.get(2).map(String::as_str).ok_or("autoplay needs a context uri")?;
      print_context(&spc.autoplay_context(uri, &[]).await?);
    }
    "search" => {
      let q = args.get(2).map(String::as_str).unwrap_or("daft punk");
      print_search(&spc, q).await?;
    }
    "devices" => {
      let (_stream, writer) = dealer.open().await?;
      let cluster = writer.cluster().await?;
      for (id, info) in &cluster.device {
        let active = if *id == cluster.active_device_id {
          " *active*"
        } else {
          ""
        };
        println!(
          "  {} [{:?}] vol={}{}",
          info.name,
          info.device_type.enum_value_or_default(),
          info.volume,
          active
        );
        println!(
          "    id={} client_id={} brand={} model={} sw={} public_ip={} can_play={}",
          id, info.client_id, info.brand, info.model, info.device_software_version, info.public_ip, info.can_play
        );
      }
    }
    "watch" => {
      let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
      let client = SpotifyClient::new(
        auth.clone(),
        dealer.device_id().to_string(),
        exec.clone(),
        Arc::new(PrintObserver::default()),
      );
      client.connect().await?;
      println!("watching {secs}s - play/pause/skip on Spotify to see deltas...");
      tokio::time::sleep(Duration::from_secs(secs)).await;
      client.disconnect().await;
    }
    "product" => {
      let client = SpotifyClient::new(
        auth.clone(),
        dealer.device_id().to_string(),
        exec.clone(),
        Arc::new(PrintObserver::default()),
      );
      let p = client.product().await?;
      println!(
        "product={} catalogue={} country={} premium={} can_use_superbird={}",
        p.product, p.catalogue, p.country, p.is_premium, p.can_use_superbird
      );
    }
    "root" => {
      let client = SpotifyClient::new(
        auth.clone(),
        dealer.device_id().to_string(),
        exec.clone(),
        Arc::new(PrintObserver::default()),
      );
      client.connect().await?;
      let shelves = client.root_browse(None, None).await?;
      println!("root: {} shelves", shelves.len());
      for s in &shelves {
        println!("  [{}] {:?}  {} items of {}", s.id, s.title, s.items.len(), s.total);
      }
      client.disconnect().await;
    }
    "lib" => {
      let node = args.get(2).map(String::as_str).unwrap_or("playlists");
      let client = SpotifyClient::new(
        auth.clone(),
        dealer.device_id().to_string(),
        exec.clone(),
        Arc::new(PrintObserver::default()),
      );
      client.connect().await?;
      let page = client.browse(node, 20, 0).await?;
      println!(
        "browse {node:?}: {} items (total={:?} more={})",
        page.items.len(),
        page.total,
        page.has_more
      );
      for it in page.items.iter().take(20) {
        println!(
          "  {} - {} [{}] art={}",
          it.title,
          it.subtitle,
          kind_of(&it.uri),
          it.image_id
        );
      }
      client.disconnect().await;
    }
    "fav" => {
      let client = SpotifyClient::new(
        auth.clone(),
        dealer.device_id().to_string(),
        exec.clone(),
        Arc::new(PrintObserver::default()),
      );
      client.connect().await?;
      let page = client.favorites_list(20, 0).await?;
      println!("favorites: {} items (total={:?})", page.items.len(), page.total);
      for it in page.items.iter().take(10) {
        println!("  {} - {} saved={}", it.title, it.subtitle, it.saved);
      }
      client.disconnect().await;
    }
    "whoami" => whoami(&http).await?,
    "apwhoami" => {
      let bearer = http.auth.bearer().await?;
      match spotify::aplogin::resolve_username(&http, &bearer, dealer.device_id()).await {
        Ok(u) => println!("canonical username = {u}"),
        Err(e) => println!("AP login failed: {e}"),
      }
    }
    "pause" | "resume" | "next" | "prev" | "seek" | "play" => {
      write_cmd(&dealer, cmd, args.get(2).map(String::as_str)).await?
    }
    "queue" => queue_cmd(&auth, &exec, &dealer, &args[2..]).await?,
    other => {
      eprintln!("unknown command: {other}\n{USAGE}");
      std::process::exit(2);
    }
  }
  Ok(())
}

#[derive(Default)]
struct PrintObserver {
  ready: Option<tokio::sync::mpsc::Sender<()>>,
}

impl Observer for PrintObserver {
  fn on_player(&self, s: PlayerState) {
    let track = match &s.track {
      Some(t) => format!(
        "{} - {}",
        t.name,
        t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
      ),
      None => "(nothing)".to_string(),
    };
    println!(
      "[player] {track} | {} | {}s/{}s | shuffle={} repeat={:?}",
      if s.is_paused { "paused" } else { "playing" },
      s.position_ms / 1000,
      s.duration_ms / 1000,
      s.shuffle,
      s.repeat,
    );
  }
  fn on_queue(&self, q: Queue) {
    println!("[queue] {} upcoming", q.next.len());
    if let Some(ready) = &self.ready {
      let _ = ready.try_send(());
    }
  }
  fn on_devices(&self, d: Vec<Device>) {
    let names: Vec<String> = d
      .iter()
      .map(|x| format!("{}{}", x.name, if x.is_active { "*" } else { "" }))
      .collect();
    println!("[devices] {}", names.join(", "));
  }
  fn on_auth(&self, a: AuthState) {
    println!("[auth] {a:?}");
  }
  fn on_library_changed(&self, scope: LibraryScope) {
    println!("[library] changed: {scope:?}");
  }
}

async fn whoami(http: &SpHttp) -> Result<(), Boxed> {
  let bearer = http.auth.bearer().await?;

  println!("== product_state (full) ==");
  let resp = http
    .send(
      HttpMethod::Get,
      format!("{SPCLIENT}/melody/v1/product_state"),
      http.headers(true).await?,
      Vec::new(),
      0,
    )
    .await?;
  println!("{}", String::from_utf8_lossy(&resp.body));

  println!("\n== login5 one_time_token = access_token ==");
  let mut ci = ClientInfo::new();
  ci.client_id = ANDROID_CLIENT_ID.to_string();
  ci.device_id = random_hex(20);
  let mut ott = OneTimeToken::new();
  ott.token = bearer.clone();
  let mut req = LoginRequest::new();
  req.client_info = MessageField::some(ci);
  req.login_method = Some(Login_method::OneTimeToken(ott));
  let mut headers = ::http::header::HeaderMap::new();
  headers.insert(
    ::http::header::ACCEPT,
    ::http::header::HeaderValue::from_static("application/x-protobuf"),
  );
  headers.insert(
    ::http::header::CONTENT_TYPE,
    ::http::header::HeaderValue::from_static("application/x-protobuf"),
  );
  let resp = http
    .send(
      HttpMethod::Post,
      "https://login5.spotify.com/v3/login".to_string(),
      headers,
      req.write_to_bytes()?,
      0,
    )
    .await?;
  let status = resp.status;
  let bytes = resp.body;
  println!("  login5 -> {status} ({} bytes)", bytes.len());
  match LoginResponse::parse_from_bytes(&bytes) {
    Ok(lr) => match lr.response {
      Some(Login5Response::Ok(ok)) => println!("  OK username = {}", ok.username),
      Some(Login5Response::Error(e)) => println!("  error = {e:?}"),
      Some(Login5Response::Challenges(_)) => println!("  challenges (would need client-token + hashcash)"),
      Some(_) => println!("  (other login5 response variant)"),
      None => println!("  no response variant; warnings={:?}", lr.warnings),
    },
    Err(_) => println!(
      "  unparseable: {}",
      String::from_utf8_lossy(&bytes).chars().take(160).collect::<String>()
    ),
  }
  Ok(())
}

async fn pair(auth: &Arc<Auth>) -> Result<(), Boxed> {
  let flow = auth.begin_device_flow().await?;
  println!("\n  open: {}\n  code: {}\n", flow.verification_uri, flow.user_code);
  println!("waiting for approval...");
  auth.complete_device_flow(&flow).await?;
  println!("paired ok.");
  Ok(())
}

async fn probe(spc: &SpClient, dealer: &Dealer, username: Option<&str>) -> Result<(), Boxed> {
  println!("== product_state ==");
  let product = spc.product_state().await?;
  let pick = |k: &str| product.get(k).and_then(|v| v.as_str()).unwrap_or("?").to_string();
  println!(
    "  product={} catalogue={} country={} on-demand={}",
    pick("product"),
    pick("catalogue"),
    pick("country"),
    pick("on-demand"),
  );

  println!("== dealer cluster ==");
  let (_stream, writer) = dealer.open().await?;
  let cid = writer.connection_id().to_string();
  println!("  connection-id: {}...", &cid[..cid.len().min(20)]);
  let cluster = writer.cluster().await?;
  println!(
    "  active_device_id: {}",
    if cluster.active_device_id.is_empty() {
      "(none)"
    } else {
      &cluster.active_device_id
    }
  );
  println!("  now-playing: {}", describe_np(&cluster));
  println!("  devices: {}", cluster.device.len());

  let np_uri = cluster.player_state.track.uri.clone();
  if np_uri.starts_with("spotify:track:") {
    println!("== hydrate now-playing track ==");
    let tracks = spc.get_tracks(std::slice::from_ref(&np_uri)).await?;
    if let Some(t) = tracks.get(&np_uri) {
      let artists: Vec<&str> = t.artist.iter().map(|a| a.name()).collect();
      println!(
        "  {} - {} [{}s] art={}",
        t.name(),
        artists.join(", "),
        t.duration() / 1000,
        image_hex(&t.album.cover_group)
      );
    }
  }

  println!("== casita home ==");
  let home = spc.get_home("en").await?;
  let populated = home.body.sections.iter().filter(|s| pick_carousel(s).1 > 0).count();
  println!("  {} sections ({} populated)", home.body.sections.len(), populated);
  for s in home.body.sections.iter().filter(|s| pick_carousel(s).1 > 0).take(8) {
    let (title, n) = pick_carousel(s);
    println!("    [{}] {:?}  {} items", section_kind(s), title, n);
  }

  println!("== search 'daft punk' ==");
  print_search(spc, "daft punk").await?;

  if let Some(user) = username {
    println!("== user library ({user}) ==");
    match spc.rootlist(user).await {
      Ok(rl) => println!("  rootlist: {} items", rl.contents.items.len()),
      Err(e) => println!("  rootlist FAILED: {e}"),
    }
    match spc.collection_paging(user, "collection", 50).await {
      Ok(items) => println!("  liked collection: {} items", items.len()),
      Err(e) => println!("  collection FAILED: {e}"),
    }
    match spc.recently_played(user, 20).await {
      Ok(rp) => println!("  recently-played: {} items", rp.items.len()),
      Err(e) => println!("  recently-played FAILED: {e}"),
    }
  } else {
    println!("(set SPOTIFY_USERNAME to probe rootlist/collection/recents)");
  }

  println!("\nprobe complete.");
  Ok(())
}

fn tags_of(it: &spotify::proto::custom::searchview::SearchItem) -> String {
  match it.playlist.as_ref().map(|p| p.tags.join(",")) {
    Some(tags) if !tags.is_empty() => format!(" tags={tags}"),
    _ => String::new(),
  }
}

async fn print_search(spc: &SpClient, q: &str) -> Result<(), Boxed> {
  let res = spc.search(q, 6).await?;
  let mut shown = 0;
  for it in &res.items {
    if shown >= 8 {
      break;
    }
    if !it.section.entries.is_empty() {
      for e in &it.section.entries {
        let ent = &e.item.entity;
        println!(
          "  [section] {} {} {}{}",
          kind_of(&ent.uri),
          ent.name,
          ent.uri,
          tags_of(ent)
        );
        shown += 1;
      }
    } else if !it.uri.is_empty() {
      println!("  {} {} {}{}", kind_of(&it.uri), it.name, it.uri, tags_of(it));
      shown += 1;
    }
  }
  Ok(())
}

async fn write_cmd(dealer: &Dealer, cmd: &str, arg: Option<&str>) -> Result<(), Boxed> {
  let (_stream, writer) = dealer.open().await?;
  let cluster = writer.cluster().await?;
  let target = active_device(&cluster, dealer.device_id(), None).ok_or("no reachable target device")?;
  let (status, body) = match cmd {
    "pause" => writer.pause(&target).await?,
    "resume" => writer.resume(&target).await?,
    "next" => writer.skip_next(&target).await?,
    "prev" => writer.skip_prev(&target).await?,
    "seek" => {
      let ms: i64 = arg.unwrap_or("0").parse().unwrap_or(0);
      writer.seek_to(&target, ms).await?
    }
    "play" => {
      let uri = arg.ok_or("play needs a context uri")?;
      writer.play(&target, play_envelope(uri)).await?
    }
    _ => unreachable!(),
  };
  println!("{cmd} -> {status} {}", body.chars().take(120).collect::<String>());
  Ok(())
}

async fn queue_cmd(auth: &Arc<Auth>, exec: &HttpExecutor, dealer: &Dealer, args: &[String]) -> Result<(), Boxed> {
  let sub = args.first().map(String::as_str).unwrap_or("show");
  if matches!(sub, "add" | "add-at") {
    return queue_write(auth, exec, dealer, sub, args).await;
  }

  let (_stream, writer) = dealer.open().await?;
  let cluster = writer.cluster().await?;
  let ps = &cluster.player_state;
  let encode = |ts: &[ProvidedTrack]| ts.iter().map(provided_track_json).collect::<Vec<_>>();

  match sub {
    "show" => {
      print_queue(&cluster);
      return Ok(());
    }
    "dump" => {
      let path = args.get(1).ok_or("queue dump needs a file path")?;
      let dump = serde_json::json!({
        "queue_revision": ps.queue_revision,
        "next_tracks": encode(&ps.next_tracks),
        "prev_tracks": encode(&ps.prev_tracks),
      });
      std::fs::write(path, serde_json::to_vec_pretty(&dump)?)?;
      println!(
        "dumped revision={} next={} prev={} -> {path}",
        ps.queue_revision,
        ps.next_tracks.len(),
        ps.prev_tracks.len()
      );
      return Ok(());
    }
    _ => {}
  }

  let target = active_device(&cluster, dealer.device_id(), None).ok_or("no reachable target device")?;
  let (status, body) = match sub {
    "restore" => {
      let path = args.get(1).ok_or("queue restore needs a file path")?;
      let dump: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
      let tracks = |key: &str| {
        dump
          .get(key)
          .and_then(|v| v.as_array())
          .map(|a| a.iter().map(provided_track_from_json).collect::<Vec<_>>())
          .unwrap_or_default()
      };
      writer
        .set_queue(
          &target,
          &tracks("next_tracks"),
          &tracks("prev_tracks"),
          &ps.queue_revision,
        )
        .await?
    }
    other => return Err(format!("unknown queue subcommand: {other}").into()),
  };
  println!("queue {sub} -> {status} {}", body.chars().take(200).collect::<String>());
  print_queue(&writer.cluster().await?);
  Ok(())
}

async fn queue_write(
  auth: &Arc<Auth>,
  exec: &HttpExecutor,
  dealer: &Dealer,
  sub: &str,
  args: &[String],
) -> Result<(), Boxed> {
  let (position, uri) = match sub {
    "add" => (QueuePosition::Append, args.get(1).ok_or("queue add needs a uri")?),
    _ => {
      let at: u32 = args.get(1).ok_or("queue add-at needs an index")?.parse()?;
      (
        QueuePosition::Index { at },
        args.get(2).ok_or("queue add-at needs a uri")?,
      )
    }
  };

  let (ready_tx, mut ready) = tokio::sync::mpsc::channel(1);
  let client = SpotifyClient::new(
    auth.clone(),
    dealer.device_id().to_string(),
    exec.clone(),
    Arc::new(PrintObserver { ready: Some(ready_tx) }),
  );
  client.connect().await?;
  tokio::time::timeout(Duration::from_secs(15), ready.recv())
    .await
    .map_err(|_| "no cluster arrived; is spotify playing anywhere?")?;
  let wrote = client.queue_uri(uri, position).await;
  client.disconnect().await;
  wrote?;
  println!("queue {sub} {position:?} {uri} -> ok");

  tokio::time::sleep(Duration::from_millis(750)).await;
  let (_stream, writer) = dealer.open().await?;
  print_queue(&writer.cluster().await?);
  Ok(())
}

fn print_queue(cluster: &Cluster) {
  let ps = &cluster.player_state;
  println!(
    "revision={} current={} next={} prev={}",
    ps.queue_revision,
    ps.track.uri,
    ps.next_tracks.len(),
    ps.prev_tracks.len()
  );
  for (i, t) in ps.next_tracks.iter().enumerate().take(20) {
    println!(
      "  [{i}] {} uid={} provider={} queued={} title={:?}",
      t.uri,
      t.uid,
      t.provider,
      is_queued(t),
      t.metadata.get("title")
    );
  }
}

fn parse_resolve(args: &[String]) -> Result<VoiceResolveRequest, Boxed> {
  let mut req = VoiceResolveRequest::default();
  let mut words: Vec<&str> = Vec::new();
  let mut rest = args.iter();
  while let Some(arg) = rest.next() {
    let mut value = || rest.next().cloned().ok_or_else(|| format!("{arg} needs a value"));
    match arg.as_str() {
      "--type" => req.target_type = Some(target_kind(&value()?)?),
      "--position" => req.position = Some(value()?.parse()?),
      "--mood" => req.mood = Some(value()?),
      "--genre" => req.genre = Some(value()?),
      "--era" => req.era = Some(value()?),
      "--random" => req.popularity_filter = Some(VoicePopularity::Random),
      "--filter" => req.popularity_filter = Some(popularity(&value()?)?),
      flag if flag.starts_with("--") => return Err(format!("unknown flag: {flag}").into()),
      word => words.push(word),
    }
  }
  if !words.is_empty() {
    req.target = Some(words.join(" "));
  }
  Ok(req)
}

fn popularity(name: &str) -> Result<VoicePopularity, Boxed> {
  match name {
    "top5" | "top_5" => Ok(VoicePopularity::Top5),
    "top10" | "top_10" => Ok(VoicePopularity::Top10),
    "popular" => Ok(VoicePopularity::Popular),
    "recent" => Ok(VoicePopularity::Recent),
    "new" => Ok(VoicePopularity::New),
    "first" => Ok(VoicePopularity::First),
    "random" => Ok(VoicePopularity::Random),
    other => Err(format!("unknown popularity filter: {other}").into()),
  }
}

fn target_kind(name: &str) -> Result<VoiceTargetKind, Boxed> {
  match name {
    "track" => Ok(VoiceTargetKind::Track),
    "album" => Ok(VoiceTargetKind::Album),
    "artist" => Ok(VoiceTargetKind::Artist),
    "playlist" => Ok(VoiceTargetKind::Playlist),
    "show" | "podcast" => Ok(VoiceTargetKind::Show),
    "episode" => Ok(VoiceTargetKind::Episode),
    "station" => Ok(VoiceTargetKind::Station),
    other => Err(format!("unknown target type: {other}").into()),
  }
}

fn print_resolved(out: &VoiceResolved) {
  println!("uri     = {}", out.uri);
  println!("context = {}", out.context_uri.as_deref().unwrap_or("(none)"));
  println!("display = {}", out.display);
  println!("kind    = {:?}", out.kind);
  println!("{} alternatives", out.alternatives.len());
  for alt in &out.alternatives {
    println!("  [{:?}] {} {}", alt.kind, alt.display, alt.uri);
  }
}

fn play_envelope(uri: &str) -> serde_json::Value {
  serde_json::json!({
      "endpoint": "play",
      "context": {"uri": uri, "url": format!("context://{uri}"), "metadata": {}},
      "play_origin": {"feature_identifier": "harmony", "feature_version": "9.1.52.1394", "referrer_identifier": "home"},
      "prepare_play_options": {"license": "premium"},
      "play_options": {"reason": "interactive", "operation": "replace", "trigger": "immediately"},
  })
}

fn describe_np(cluster: &Cluster) -> String {
  let ps = &cluster.player_state;
  let uri = ps.track.uri.clone();
  if uri.is_empty() {
    return "(nothing playing)".to_string();
  }
  let md = &ps.track.metadata;
  let title = md.get("title").cloned().unwrap_or_default();
  let artist = md
    .get("artist_name")
    .cloned()
    .unwrap_or_else(|| "(artist via hydration)".to_string());
  format!(
    "{title} - {artist} [{}] {uri}",
    if ps.is_paused { "paused" } else { "playing" }
  )
}

fn pick_carousel(s: &spotify::proto::custom::casita_home::Section) -> (String, usize) {
  for car in [&s.shortcuts, &s.carousel, &s.list_carousel] {
    if let Some(c) = car.as_ref() {
      let n = c.items.inner.items.len();
      if n > 0 || !c.header.title.text.is_empty() {
        return (c.header.title.text.clone(), n);
      }
    }
  }
  (String::new(), 0)
}

fn carousel_items(s: &spotify::proto::custom::casita_home::Section) -> (String, Vec<String>) {
  for car in [&s.shortcuts, &s.carousel, &s.list_carousel] {
    if let Some(c) = car.as_ref() {
      let uris: Vec<String> = c.items.inner.items.iter().map(|i| i.uri.clone()).collect();
      if !uris.is_empty() || !c.header.title.text.is_empty() {
        return (c.header.title.text.clone(), uris);
      }
    }
  }
  (String::new(), Vec::new())
}

fn print_context(ctx: &serde_json::Value) {
  let s = |k: &str| ctx.get(k).and_then(|v| v.as_str()).unwrap_or("(none)");
  println!("uri = {}", s("uri"));
  println!("url = {}", s("url"));
  if let Some(md) = ctx.get("metadata").and_then(|v| v.as_object()) {
    for (k, v) in md {
      println!("meta {k} = {v}");
    }
  }
  if let Some(r) = ctx.get("restrictions") {
    println!("restrictions = {r}");
  }
  let pages = ctx.get("pages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
  println!("{} pages", pages.len());
  for p in &pages {
    print_page(p);
  }
}

fn print_page(p: &serde_json::Value) {
  let tracks = p.get("tracks").and_then(|v| v.as_array()).cloned().unwrap_or_default();
  println!(
    "  page: {} tracks page_url={:?} next_page_url={:?} meta={:?}",
    tracks.len(),
    p.get("page_url").and_then(|v| v.as_str()),
    p.get("next_page_url").and_then(|v| v.as_str()),
    p.get("metadata"),
  );
  for t in tracks.iter().take(5) {
    println!(
      "    {} uid={:?} meta={:?}",
      t.get("uri").and_then(|v| v.as_str()).unwrap_or("(no uri)"),
      t.get("uid").and_then(|v| v.as_str()),
      t.get("metadata"),
    );
  }
}

fn section_kind(s: &spotify::proto::custom::casita_home::Section) -> String {
  let uri = &s.id.uri;
  uri.rsplit('|').next().unwrap_or(uri).to_string()
}

fn kind_of(uri: &str) -> &str {
  uri.split(':').nth(1).unwrap_or("?")
}

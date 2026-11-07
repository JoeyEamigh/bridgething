use std::{
  collections::HashMap,
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};

use tokio::sync::Mutex as AsyncMutex;

use super::client::{ItemType, Metadata, Page, PlexError, PlexServer};

const INDEX_LIFETIME: Duration = Duration::from_secs(600);
const MATCHED_GUID: &str = "plex://";

pub(super) type Found = (Arc<PlexServer>, Metadata);

#[derive(Clone)]
pub(super) struct Library {
  pub server: Arc<PlexServer>,
  pub section: String,
}

#[derive(Default)]
pub(super) struct Servers {
  pub servers: Vec<Arc<PlexServer>>,
  pub libraries: Vec<Library>,
}

impl Servers {
  fn rank(&self, server: &PlexServer) -> usize {
    self
      .servers
      .iter()
      .position(|known| known.machine == server.machine)
      .unwrap_or(usize::MAX)
  }
}

#[derive(Default)]
pub(super) struct Registry {
  current: Mutex<Arc<Servers>>,
}

impl Registry {
  pub fn snapshot(&self) -> Arc<Servers> {
    self.current.lock().unwrap().clone()
  }

  pub fn replace(&self, servers: Servers) {
    *self.current.lock().unwrap() = Arc::new(servers);
  }

  pub fn server(&self, machine: &str) -> Option<Arc<PlexServer>> {
    self
      .snapshot()
      .servers
      .iter()
      .find(|server| server.machine == machine)
      .cloned()
  }
}

#[derive(Clone, Copy)]
pub(super) enum Query<'a> {
  Items {
    kind: ItemType,
    sort: &'a str,
    filter: Option<(&'a str, &'a str)>,
  },
  Search {
    kind: ItemType,
    text: &'a str,
  },
}

impl Query<'_> {
  async fn fetch(&self, library: &Library, size: u32) -> Result<Page, PlexError> {
    let server = &library.server;
    match *self {
      Query::Items { kind, sort, filter } => {
        let filters: Vec<(&str, &str)> = filter.into_iter().collect();
        server.items(&library.section, kind, sort, &filters, 0, size).await
      }
      Query::Search { kind, text } => server.search(&library.section, kind, text, 0, size).await,
    }
  }
}

pub(super) struct Gathered {
  pub items: Vec<Found>,
  pub more: bool,
}

pub(super) async fn gather(servers: &Servers, query: Query<'_>, reach: u32) -> Result<Gathered, PlexError> {
  let pages = futures::future::join_all(servers.libraries.iter().map(|library| query.fetch(library, reach))).await;
  let mut lists = Vec::new();
  let mut more = false;
  for (library, page) in servers.libraries.iter().zip(pages) {
    let page = page?;
    more |= page.total > reach;
    lists.push(
      page
        .items
        .into_iter()
        .map(|item| (library.server.clone(), item))
        .collect(),
    );
  }
  Ok(Gathered {
    items: dedupe(servers, interleave(lists)),
    more,
  })
}

pub(super) fn interleave<T>(lists: Vec<Vec<T>>) -> Vec<T> {
  let mut iters: Vec<_> = lists.into_iter().map(Vec::into_iter).collect();
  let mut out = Vec::new();
  loop {
    let mut any = false;
    for iter in &mut iters {
      if let Some(item) = iter.next() {
        out.push(item);
        any = true;
      }
    }
    if !any {
      return out;
    }
  }
}

pub(super) fn identity(item: &Metadata) -> String {
  if let Some(guid) = item.guid.as_deref().filter(|guid| guid.starts_with(MATCHED_GUID)) {
    return guid.to_owned();
  }
  let low = |text: Option<&str>| text.unwrap_or_default().to_lowercase();
  let title = low(Some(&item.title));
  match item.kind.as_str() {
    "album" => format!("album|{title}|{}", low(item.parent_title.as_deref())),
    "artist" => format!("artist|{title}"),
    "track" => format!(
      "track|{title}|{}|{}|{}",
      low(item.grandparent_title.as_deref()),
      low(item.parent_title.as_deref()),
      item.index.unwrap_or(0)
    ),
    "playlist" => format!("playlist|{title}|{}", item.leaf_count.unwrap_or(0)),
    other => format!("{other}|{}", item.rating_key),
  }
}

struct Seen {
  winner: usize,
  viewed: Option<i64>,
  plays: Option<u32>,
}

pub(super) fn dedupe(servers: &Servers, found: Vec<Found>) -> Vec<Found> {
  let keyed: Vec<(String, usize, Found)> = found
    .into_iter()
    .map(|found| (identity(&found.1), servers.rank(&found.0), found))
    .collect();
  let mut seen: HashMap<&str, Seen> = HashMap::new();
  for (id, rank, (_, item)) in &keyed {
    let entry = seen.entry(id).or_insert(Seen {
      winner: *rank,
      viewed: None,
      plays: None,
    });
    entry.winner = entry.winner.min(*rank);
    entry.viewed = entry.viewed.max(item.last_viewed_at);
    entry.plays = match (entry.plays, item.view_count) {
      (Some(held), Some(more)) => Some(held + more),
      (held, more) => held.or(more),
    };
  }
  let folded: HashMap<String, (usize, Option<i64>, Option<u32>)> = seen
    .into_iter()
    .map(|(id, seen)| (id.to_owned(), (seen.winner, seen.viewed, seen.plays)))
    .collect();
  keyed
    .into_iter()
    .filter_map(|(id, rank, (server, mut item))| {
      let (winner, viewed, plays) = folded[&id];
      (winner == rank).then(|| {
        item.last_viewed_at = viewed;
        item.view_count = plays;
        (server, item)
      })
    })
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Indexed {
  Albums,
  Artists,
}

impl Indexed {
  fn kind(self) -> ItemType {
    match self {
      Indexed::Albums => ItemType::Album,
      Indexed::Artists => ItemType::Artist,
    }
  }
}

struct Built {
  items: Arc<Vec<Found>>,
  servers: Arc<Servers>,
  at: Instant,
}

#[derive(Default)]
pub(super) struct Indexes {
  albums: AsyncMutex<Option<Built>>,
  artists: AsyncMutex<Option<Built>>,
}

impl Indexes {
  fn slot(&self, which: Indexed) -> &AsyncMutex<Option<Built>> {
    match which {
      Indexed::Albums => &self.albums,
      Indexed::Artists => &self.artists,
    }
  }

  pub async fn get(&self, servers: Arc<Servers>, which: Indexed) -> Result<Arc<Vec<Found>>, PlexError> {
    let mut slot = self.slot(which).lock().await;
    if let Some(built) = slot.as_ref()
      && Arc::ptr_eq(&built.servers, &servers)
      && built.at.elapsed() < INDEX_LIFETIME
    {
      return Ok(built.items.clone());
    }
    let lists = futures::future::join_all(
      servers
        .libraries
        .iter()
        .map(|library| library.server.every(&library.section, which.kind())),
    )
    .await;
    let mut all = Vec::new();
    for (library, list) in servers.libraries.iter().zip(lists) {
      all.extend(list?.into_iter().map(|item| (library.server.clone(), item)));
    }
    let items = Arc::new(dedupe(&servers, all));
    *slot = Some(Built {
      items: items.clone(),
      servers,
      at: Instant::now(),
    });
    Ok(items)
  }

  pub async fn rated(&self, machine: &str, key: &str, rating: Option<f32>, at: i64) {
    for which in [Indexed::Albums, Indexed::Artists] {
      let mut slot = self.slot(which).lock().await;
      let Some(built) = slot.as_mut() else { continue };
      let items = Arc::make_mut(&mut built.items);
      for (server, item) in items.iter_mut() {
        if server.machine == machine && item.rating_key == key {
          item.user_rating = rating;
          item.last_rated_at = rating.map(|_| at);
        }
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use bridgething_io::{HttpDownloadSink, HttpExecutor, HttpRequest, HttpSink, HttpTransport};

  use super::*;

  struct NoHttp;

  impl HttpTransport for NoHttp {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      sink.fail("unused".into());
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      sink.on_failed("unused".into());
    }
  }

  fn server(machine: &str) -> Arc<PlexServer> {
    Arc::new(PlexServer::new(
      machine,
      machine,
      "https://plex.example",
      "tok",
      "cid",
      HttpExecutor::new(Arc::new(NoHttp)),
    ))
  }

  fn album(key: &str, guid: &str, title: &str, viewed: Option<i64>, plays: Option<u32>) -> Metadata {
    Metadata {
      rating_key: key.into(),
      kind: "album".into(),
      title: title.into(),
      parent_title: Some("Artist".into()),
      guid: Some(guid.into()),
      last_viewed_at: viewed,
      view_count: plays,
      ..Metadata::default()
    }
  }

  fn servers(machines: &[&str]) -> Servers {
    Servers {
      servers: machines.iter().map(|machine| server(machine)).collect(),
      libraries: Vec::new(),
    }
  }

  fn keys(found: &[Found]) -> Vec<String> {
    found
      .iter()
      .map(|(server, item)| format!("{}:{}", server.machine, item.rating_key))
      .collect()
  }

  #[test]
  fn the_preferred_server_keeps_a_shared_item_and_folds_the_others_listening_into_it() {
    let known = servers(&["near", "far"]);
    let (near, far) = (known.servers[0].clone(), known.servers[1].clone());
    let kept = dedupe(
      &known,
      vec![
        (far.clone(), album("9", "plex://album/a", "Away", Some(500), Some(3))),
        (near.clone(), album("1", "plex://album/a", "Away", Some(100), Some(2))),
        (far.clone(), album("8", "plex://album/b", "Only Far", None, None)),
      ],
    );
    assert_eq!(keys(&kept), ["near:1", "far:8"]);
    assert_eq!(kept[0].1.last_viewed_at, Some(500));
    assert_eq!(kept[0].1.view_count, Some(5));
  }

  #[test]
  fn unmatched_items_fall_back_to_their_titles_and_one_server_keeps_its_own_copies() {
    let known = servers(&["near", "far"]);
    let (near, far) = (known.servers[0].clone(), known.servers[1].clone());
    let kept = dedupe(
      &known,
      vec![
        (near.clone(), album("1", "local://11", "Demo Tape", None, None)),
        (far.clone(), album("2", "local://99", "demo tape", None, None)),
        (near.clone(), album("3", "plex://album/c", "Twice", None, None)),
        (near.clone(), album("4", "plex://album/c", "Twice", None, None)),
        (far.clone(), album("5", "plex://album/c", "Twice", None, None)),
      ],
    );
    assert_eq!(keys(&kept), ["near:1", "near:3", "near:4"]);
  }

  #[test]
  fn interleaving_takes_one_from_each_list_in_turn() {
    assert_eq!(
      interleave(vec![vec![1, 2, 3], vec![10], vec![20, 21]]),
      [1, 10, 20, 2, 21, 3]
    );
  }
}

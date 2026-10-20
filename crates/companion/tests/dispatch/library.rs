use std::sync::Arc;

use bridgething_companion::{dispatch::library::LibraryDispatcher, provider::ProviderError};
use bridgething_gateway::{HandlerError, LibraryHandler};
use libbridgething::{
  BrowseEntry, BrowseFolder, BrowseResult, FavoritesPage, ItemKind, ItemRef, LibraryError, SearchResult,
  gateway::{
    BrowseChanged, FavoritesSet, FavoritesToggle, GatewayToBridgeLibraryMsg, GatewayToBridgeMsg,
    GatewayToBridgeMsgData, LibraryBrowseRequest, LibraryFavoritesListRequest, LibrarySearchRequest,
  },
  wire::WireError,
};

use crate::{
  fakes::{FakeProvider, FakeRegistry},
  poll::eventually,
  responses::Responses,
  support::Peer,
};

fn browse_request() -> LibraryBrowseRequest {
  LibraryBrowseRequest {
    node_id: None,
    limit: 20,
    offset: 0,
    sections: None,
    preview: None,
  }
}

fn track(uri: &str) -> ItemRef {
  ItemRef {
    uri: uri.into(),
    kind: ItemKind::Track,
    persistent_id: None,
  }
}

fn library_error(msg: &GatewayToBridgeMsg) -> Option<LibraryError> {
  match &msg.data {
    GatewayToBridgeMsgData::Library(GatewayToBridgeLibraryMsg::ErrorEvent(reply)) => Some(reply.error.clone()),
    _ => None,
  }
}

#[tokio::test]
async fn browse_routes_to_the_active_provider_and_returns_its_result() {
  let provider = Arc::new(FakeProvider {
    on_browse: Some(Box::new(|_| {
      Ok(BrowseResult {
        entries: vec![],
        total: Some(7),
        has_more: false,
      })
    })),
    ..FakeProvider::named("spotify")
  });
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(provider.clone()),
    Arc::new(gateway),
    responses.cache(),
  );

  let reply = dispatch.browse(browse_request()).await.expect("the browse resolved");

  assert_eq!(reply.response.result.total, Some(7));
  assert!(provider.saw("browse"));
}

#[tokio::test]
async fn a_library_request_with_no_active_provider_is_a_domain_error() {
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(FakeRegistry::empty(), Arc::new(gateway), responses.cache());

  let refused = dispatch
    .browse(browse_request())
    .await
    .expect_err("no provider is a refusal");

  match refused {
    HandlerError::Domain(reply) => assert!(
      matches!(reply.error, LibraryError::NotSupported { .. }),
      "got {:?}",
      reply.error
    ),
    other => panic!("an absent provider is a library error, not {other:?}"),
  }
}

#[tokio::test]
async fn a_verb_the_provider_does_not_implement_is_a_protocol_refusal() {
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(FakeProvider::bare("spotify")),
    Arc::new(gateway),
    responses.cache(),
  );

  let refused = dispatch
    .browse(browse_request())
    .await
    .expect_err("an unimplemented verb is a refusal");

  assert_eq!(refused, HandlerError::Wire(WireError::Unimplemented));
}

#[tokio::test]
async fn an_unauthorized_provider_is_a_domain_error_rather_than_a_protocol_one() {
  let provider = Arc::new(FakeProvider {
    on_browse: Some(Box::new(|_| Err(ProviderError::NotAuthenticated))),
    ..FakeProvider::named("spotify")
  });
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(FakeRegistry::with(provider), Arc::new(gateway), responses.cache());

  let refused = dispatch.browse(browse_request()).await.expect_err("a refusal");

  match refused {
    HandlerError::Domain(reply) => assert_eq!(reply.error, LibraryError::Unauthorized),
    other => panic!("an expired session is a library error, not {other:?}"),
  }
}

#[tokio::test]
async fn search_and_favorites_list_route_to_the_active_provider() {
  let provider = Arc::new(FakeProvider {
    on_search: Some(Box::new(|request| {
      assert_eq!(request.query, "daft punk");
      Ok(SearchResult {
        items: vec![],
        kinds: vec![ItemKind::Track],
        total: Some(0),
        has_more: false,
      })
    })),
    on_favorites_list: Some(Box::new(|_| {
      Ok(FavoritesPage {
        items: vec![],
        total: Some(42),
        has_more: true,
      })
    })),
    ..FakeProvider::named("spotify")
  });
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(provider.clone()),
    Arc::new(gateway),
    responses.cache(),
  );

  let found = dispatch
    .search(LibrarySearchRequest {
      query: "daft punk".into(),
      kinds: None,
      limit: 10,
      offset: 0,
    })
    .await
    .expect("the search resolved");
  assert_eq!(found.response.result.kinds, vec![ItemKind::Track]);
  assert!(provider.saw("search:daft punk"));

  let page = dispatch
    .favorites_list(LibraryFavoritesListRequest { limit: 20, offset: 0 })
    .await
    .expect("the favorites list resolved");
  assert_eq!(page.response.page.total, Some(42));
  assert!(page.response.page.has_more);
  assert!(provider.saw("favoritesList"));
}

#[tokio::test]
async fn a_favorites_write_goes_to_the_provider_claiming_the_uri() {
  let owner = FakeProvider::bare("spotify");
  let other = FakeProvider::bare("applemusic");
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::of(vec![other.clone(), owner.clone()]),
    Arc::new(gateway),
    responses.cache(),
  );

  dispatch
    .favorites_toggle(FavoritesToggle {
      item: track("spotify:track:1"),
    })
    .await
    .expect("a command never refuses");

  assert!(owner.saw("favoritesToggle:spotify:track:1"));
  assert!(!other.saw("favoritesToggle:spotify:track:1"));
}

#[tokio::test]
async fn a_favorites_write_for_an_unclaimed_uri_falls_back_to_the_active_provider() {
  let active = FakeProvider::bare("spotify");
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(FakeRegistry::with(active.clone()), Arc::new(gateway), responses.cache());

  dispatch
    .favorites_set(FavoritesSet {
      item: track("unknown:track:9"),
      liked: true,
    })
    .await
    .expect("a command never refuses");

  assert!(active.saw("favoritesSet:unknown:track:9:true"));
}

#[tokio::test]
async fn a_failed_favorites_write_reports_a_library_error_event_rather_than_going_quiet() {
  let provider = Arc::new(FakeProvider {
    on_write: Some(Box::new(|| Err(ProviderError::NotAuthenticated))),
    ..FakeProvider::named("spotify")
  });
  let (gateway, peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(FakeRegistry::with(provider), Arc::new(gateway), responses.cache());

  dispatch
    .favorites_set(FavoritesSet {
      item: track("spotify:track:1"),
      liked: true,
    })
    .await
    .expect("a command never refuses");

  assert_eq!(
    peer.wait("a library error event", library_error).await,
    LibraryError::Unauthorized
  );
}

#[tokio::test]
async fn a_write_with_no_provider_at_all_reports_no_gateway() {
  let (gateway, peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(FakeRegistry::empty(), Arc::new(gateway), responses.cache());

  dispatch
    .favorites_toggle(FavoritesToggle {
      item: track("spotify:track:1"),
    })
    .await
    .expect("a command never refuses");

  assert_eq!(
    peer.wait("a library error event", library_error).await,
    LibraryError::NoGateway
  );
}

#[tokio::test]
async fn a_bulk_write_is_grouped_so_each_provider_is_called_once() {
  let spotify = FakeProvider::bare("spotify");
  let apple = FakeProvider::bare("applemusic");
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::of(vec![spotify.clone(), apple.clone()]),
    Arc::new(gateway),
    responses.cache(),
  );

  dispatch
    .favorites_set_many(libbridgething::gateway::FavoritesSetMany {
      entries: vec![
        FavoritesSet {
          item: track("spotify:track:1"),
          liked: true,
        },
        FavoritesSet {
          item: track("applemusic:track:2"),
          liked: false,
        },
        FavoritesSet {
          item: track("spotify:track:3"),
          liked: true,
        },
      ],
    })
    .await
    .expect("a command never refuses");

  assert!(spotify.saw("favoritesSetMany:2"), "two spotify uris in one call");
  assert!(apple.saw("favoritesSetMany:1"));
}

fn page(total: u32) -> BrowseResult {
  BrowseResult {
    entries: vec![BrowseEntry::Folder(BrowseFolder {
      node_id: "spotify:playlist:1".into(),
      title: "Liked".into(),
      subtitle: None,
      artwork_id: Some("spotify/img/248/ifolder".into()),
      total: None,
      preview_children: Some(vec![BrowseEntry::Folder(BrowseFolder {
        node_id: "spotify:album:2".into(),
        title: "Discovery".into(),
        subtitle: None,
        artwork_id: Some("spotify/img/96/ichild".into()),
        total: None,
        preview_children: None,
      })]),
    })],
    total: Some(total),
    has_more: false,
  }
}

type Live = Arc<std::sync::Mutex<Result<BrowseResult, ProviderError>>>;

fn browsing(name: &str, live: &Live) -> Arc<FakeProvider> {
  let live = live.clone();
  Arc::new(FakeProvider {
    on_browse: Some(Box::new(move |_| live.lock().unwrap().clone())),
    ..FakeProvider::named(name)
  })
}

fn browses(provider: &FakeProvider) -> usize {
  provider
    .calls
    .lock()
    .unwrap()
    .iter()
    .filter(|call| *call == "browse")
    .count()
}

#[tokio::test]
async fn a_page_seen_once_still_renders_when_the_network_fails_or_goes_away() {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let provider = browsing("spotify", &live);
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(provider.clone()),
    Arc::new(gateway),
    responses.cache(),
  );
  assert_eq!(
    dispatch.browse(browse_request()).await.unwrap().response.result.total,
    Some(7)
  );

  *live.lock().unwrap() = Err(ProviderError::Failed("no route to host".into()));
  assert_eq!(
    dispatch.browse(browse_request()).await.unwrap().response.result.total,
    Some(7)
  );

  responses.go_offline();
  let asked = browses(&provider);
  assert_eq!(
    dispatch.browse(browse_request()).await.unwrap().response.result.total,
    Some(7)
  );
  assert_eq!(browses(&provider), asked, "offline never waits on the network");
}

#[tokio::test]
async fn a_fresh_answer_replaces_the_cached_one_when_the_network_is_quick() {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(browsing("spotify", &live)),
    Arc::new(gateway),
    responses.cache(),
  );
  dispatch.browse(browse_request()).await.unwrap();

  *live.lock().unwrap() = Ok(page(8));

  assert_eq!(
    dispatch.browse(browse_request()).await.unwrap().response.result.total,
    Some(8)
  );
}

#[tokio::test]
async fn a_restarted_companion_serves_the_last_browse_with_no_network() {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let responses = Responses::new();
  {
    let (gateway, _peer) = Peer::link();
    let first = LibraryDispatcher::new(
      FakeRegistry::with(browsing("spotify", &live)),
      Arc::new(gateway),
      responses.cache(),
    );
    first.browse(browse_request()).await.unwrap();
  }

  *live.lock().unwrap() = Err(ProviderError::Failed("airplane mode".into()));
  responses.go_offline();
  let (gateway, _peer) = Peer::link();
  let restarted = LibraryDispatcher::new(
    FakeRegistry::with(browsing("spotify", &live)),
    Arc::new(gateway),
    responses.reopened(),
  );

  assert_eq!(
    restarted.browse(browse_request()).await.unwrap().response.result.total,
    Some(7)
  );
}

#[tokio::test]
async fn a_cached_page_never_answers_for_another_provider() {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let responses = Responses::new();
  let (gateway, _peer) = Peer::link();
  LibraryDispatcher::new(
    FakeRegistry::with(browsing("spotify", &live)),
    Arc::new(gateway),
    responses.cache(),
  )
  .browse(browse_request())
  .await
  .unwrap();

  let failing: Live = Arc::new(std::sync::Mutex::new(Err(ProviderError::Failed("offline".into()))));
  let (gateway, _peer) = Peer::link();
  let other = LibraryDispatcher::new(
    FakeRegistry::with(browsing("apple-music", &failing)),
    Arc::new(gateway),
    responses.cache(),
  );

  assert!(other.browse(browse_request()).await.is_err());
}

#[tokio::test]
async fn the_os_purging_the_cache_mid_session_is_a_live_fetch_not_an_error() {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(browsing("spotify", &live)),
    Arc::new(gateway),
    responses.cache(),
  );
  dispatch.browse(browse_request()).await.unwrap();

  std::fs::remove_dir_all(responses.dir.path()).unwrap();
  *live.lock().unwrap() = Ok(page(9));
  assert_eq!(
    dispatch.browse(browse_request()).await.unwrap().response.result.total,
    Some(9)
  );

  *live.lock().unwrap() = Err(ProviderError::Failed("offline".into()));
  assert_eq!(
    dispatch.browse(browse_request()).await.unwrap().response.result.total,
    Some(9)
  );
}

#[tokio::test]
async fn a_live_browse_warms_every_artwork_id_it_lists_on_the_phone() {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let provider = browsing("spotify", &live);
  let (gateway, _peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(provider.clone()),
    Arc::new(gateway),
    responses.cache(),
  );

  dispatch.browse(browse_request()).await.unwrap();

  assert!(eventually(|| provider.saw("asset:spotify/img/248/ifolder")).await);
  assert!(eventually(|| provider.saw("asset:spotify/img/96/ichild")).await);
}

fn browse_changed(msg: &GatewayToBridgeMsg) -> Option<BrowseChanged> {
  match &msg.data {
    GatewayToBridgeMsgData::Library(GatewayToBridgeLibraryMsg::BrowseChanged(changed)) => Some(changed.clone()),
    _ => None,
  }
}

async fn served_stale_then_refreshed(next: BrowseResult) -> (Peer, Option<u32>) {
  let live: Live = Arc::new(std::sync::Mutex::new(Ok(page(7))));
  let provider = browsing("spotify", &live);
  let (gateway, peer) = Peer::link();
  let responses = Responses::new();
  let dispatch = LibraryDispatcher::new(
    FakeRegistry::with(provider.clone()),
    Arc::new(gateway),
    responses.cache(),
  );
  dispatch.browse(browse_request()).await.unwrap();

  *live.lock().unwrap() = Ok(next);
  *provider.browse_delay.lock().unwrap() = Some(std::time::Duration::from_millis(600));
  let served = dispatch.browse(browse_request()).await.unwrap().response.result.total;
  (peer, served)
}

#[tokio::test]
async fn a_late_refresh_that_changed_the_page_tells_the_device_which_page() {
  let (peer, served) = served_stale_then_refreshed(page(8)).await;
  assert_eq!(served, Some(7), "the slow network served the phone's copy");

  let changed = peer.wait("a browse change", browse_changed).await;

  assert_eq!(changed.page, browse_request());
}

#[tokio::test]
async fn a_late_refresh_that_matched_the_served_page_says_nothing() {
  let (peer, served) = served_stale_then_refreshed(page(7)).await;
  assert_eq!(served, Some(7));

  tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;

  assert!(peer.scan(&browse_changed).is_none());
}

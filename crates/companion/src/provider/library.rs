use std::future::Future;

use libbridgething::{BrowseEntry, BrowseFolder, BrowseResult, LibraryItem};

use crate::provider::ProviderError;

const DEFAULT_ROOT_PREVIEW: u32 = 8;

pub const PLAYLISTS_NODE_ID: &str = "playlists";
pub const ALBUMS_NODE_ID: &str = "albums";
pub const ARTISTS_NODE_ID: &str = "artists";
pub const RECENTS_NODE_ID: &str = libbridgething::RECENTS_NODE_ID;
pub const NEWEST_NODE_ID: &str = "recently-added";
pub const RANDOM_NODE_ID: &str = "random";

pub const STAPLES: [(&str, &str); 6] = [
  (PLAYLISTS_NODE_ID, "Playlists"),
  (ALBUMS_NODE_ID, "Albums"),
  (ARTISTS_NODE_ID, "Artists"),
  (RECENTS_NODE_ID, "Recently played"),
  (NEWEST_NODE_ID, "Recently added"),
  (RANDOM_NODE_ID, "Random songs"),
];

pub fn page<T: Clone>(items: &[T], limit: u32, offset: u32) -> (Vec<T>, bool) {
  let start = (offset as usize).min(items.len());
  let end = (start + limit as usize).min(items.len());
  (items[start..end].to_vec(), end < items.len())
}

pub fn listing(items: Vec<LibraryItem>, total: Option<u32>, has_more: bool) -> BrowseResult {
  BrowseResult {
    entries: items.into_iter().map(BrowseEntry::Item).collect(),
    total,
    has_more,
  }
}

pub async fn staple_root<F, Fut>(sections: Option<u32>, preview: Option<u32>, node: F) -> BrowseResult
where
  F: Fn(&'static str, u32) -> Fut,
  Fut: Future<Output = Result<BrowseResult, ProviderError>>,
{
  let preview_count = preview.unwrap_or(DEFAULT_ROOT_PREVIEW);
  let mut folders = Vec::new();
  for (node_id, title) in STAPLES {
    let (children, total) = if preview_count == 0 {
      (Vec::new(), None)
    } else {
      match node(node_id, preview_count).await {
        Ok(page) => (page.entries, page.total),
        Err(error) => {
          tracing::debug!(%error, node_id, "a root preview did not load");
          (Vec::new(), None)
        }
      }
    };
    folders.push(BrowseFolder {
      node_id: node_id.into(),
      title: title.into(),
      subtitle: None,
      artwork_id: None,
      total,
      preview_children: (!children.is_empty()).then_some(children),
    });
  }
  if let Some(cap) = sections {
    folders.truncate(cap as usize);
  }
  BrowseResult {
    total: Some(folders.len() as u32),
    entries: folders.into_iter().map(BrowseEntry::Folder).collect(),
    has_more: false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn paging_reports_whether_more_follows() {
    let items = [1, 2, 3, 4, 5];
    assert_eq!(page(&items, 2, 0), (vec![1, 2], true));
    assert_eq!(page(&items, 2, 4), (vec![5], false));
    assert_eq!(page(&items, 2, 9), (vec![], false));
  }
}

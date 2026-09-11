pub const HERO_EDGE: u32 = 248;
pub const THUMBNAIL_EDGE: u32 = 96;

pub fn with_edge(id: &str, edge: u32) -> String {
  let mut parts = id.splitn(4, '/');
  let (Some(ns), Some("img"), Some(old_edge), Some(rest)) = (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return id.to_string();
  };
  if old_edge.parse::<u32>().is_err() {
    return id.to_string();
  }
  format!("{ns}/img/{edge}/{rest}")
}

pub fn hero(id: &str) -> String {
  with_edge(id, HERO_EDGE)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn thumbnail_rewrites_spotify_art_edge() {
    assert_eq!(with_edge("spotify/img/248/iabc123", 96), "spotify/img/96/iabc123");
    assert_eq!(
      with_edge("spotify/img/248/uhttps%3A%2F%2Fx", 96),
      "spotify/img/96/uhttps%3A%2F%2Fx"
    );
    assert_eq!(
      with_edge("applemusic/img/248/umusicKit%3A%2F%2Fartwork%2Fx", 96),
      "applemusic/img/96/umusicKit%3A%2F%2Fartwork%2Fx"
    );
  }

  #[test]
  fn hero_rewrites_queue_minted_thumb_edge_up() {
    assert_eq!(hero("spotify/img/96/iabc123"), "spotify/img/248/iabc123");
    assert_eq!(hero("spotify/img/248/iabc123"), "spotify/img/248/iabc123");
  }

  #[test]
  fn hero_and_thumbnail_edges_do_not_collide() {
    let queue_minted = "spotify/img/96/iabc123";
    assert_ne!(hero(queue_minted), with_edge(queue_minted, THUMBNAIL_EDGE));
  }

  #[test]
  fn non_image_ids_pass_through() {
    assert_eq!(with_edge("iap2/art/deadbeef/3", 96), "iap2/art/deadbeef/3");
    assert_eq!(with_edge("spotify/img/notanumber/x", 96), "spotify/img/notanumber/x");
    assert_eq!(hero(""), "");
  }
}

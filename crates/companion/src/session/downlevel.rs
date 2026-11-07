use std::borrow::Cow;

use libbridgething::{
  MusicProvider,
  gateway::{GatewayToBridgeCapabilitiesMsg, GatewayToBridgeMsgData},
};

pub(super) type Version = (u32, u32, u32);

const SUBSONIC: Version = (0, 12, 6);
const PLEX: Version = (0, 16, 1);

pub(super) fn version(raw: &str) -> Option<Version> {
  let mut parts = raw.trim().trim_start_matches('v').split(['.', '-', '+']);
  Some((
    parts.next()?.parse().ok()?,
    parts.next()?.parse().ok()?,
    parts.next()?.parse().ok()?,
  ))
}

fn understood(provider: MusicProvider, peer: Option<Version>) -> bool {
  let since = match provider {
    MusicProvider::None | MusicProvider::Spotify | MusicProvider::AppleMusic => return true,
    MusicProvider::Subsonic => SUBSONIC,
    MusicProvider::Plex => PLEX,
  };
  peer.is_some_and(|peer| peer >= since)
}

pub(super) fn waits_on_version(provider: MusicProvider) -> bool {
  !understood(provider, None)
}

pub(super) fn for_peer(data: &GatewayToBridgeMsgData, peer: Option<Version>) -> Cow<'_, GatewayToBridgeMsgData> {
  match data {
    GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(caps))
      if !understood(caps.music_provider, peer) =>
    {
      let mut caps = caps.clone();
      caps.music_provider = MusicProvider::None;
      Cow::Owned(GatewayToBridgeMsgData::Capabilities(
        GatewayToBridgeCapabilitiesMsg::Announce(caps),
      ))
    }
    _ => Cow::Borrowed(data),
  }
}

#[cfg(test)]
mod tests {
  use libbridgething::GatewayCapabilities;

  use super::*;

  fn announce(provider: MusicProvider) -> GatewayToBridgeMsgData {
    GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(GatewayCapabilities {
      uri_schemes: vec!["plex".into()],
      music_provider: provider,
      ..GatewayCapabilities::default()
    }))
  }

  fn provider_sent(data: &GatewayToBridgeMsgData) -> MusicProvider {
    match data {
      GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(caps)) => caps.music_provider,
      _ => panic!("not an announce"),
    }
  }

  #[test]
  fn daemon_versions_parse_with_or_without_a_v_and_a_suffix() {
    assert_eq!(version("v0.16.0"), Some((0, 16, 0)));
    assert_eq!(version("0.16.1-dev.3"), Some((0, 16, 1)));
    assert_eq!(version("v12.0.1"), Some((12, 0, 1)));
    assert_eq!(version(""), None);
    assert_eq!(version("unknown"), None);
  }

  #[test]
  fn a_daemon_that_predates_a_provider_hears_none_and_keeps_everything_else() {
    let data = announce(MusicProvider::Plex);
    let older = for_peer(&data, Some((0, 16, 0)));
    assert_eq!(provider_sent(&older), MusicProvider::None);
    let GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(caps)) = older.as_ref() else {
      panic!("not an announce");
    };
    assert_eq!(caps.uri_schemes, ["plex"]);
    assert_eq!(provider_sent(&for_peer(&data, Some((0, 16, 1)))), MusicProvider::Plex);
    assert_eq!(
      provider_sent(&for_peer(&announce(MusicProvider::Subsonic), Some((0, 12, 5)))),
      MusicProvider::None
    );
    assert_eq!(
      provider_sent(&for_peer(&announce(MusicProvider::Subsonic), Some((0, 12, 6)))),
      MusicProvider::Subsonic
    );
  }

  #[test]
  fn an_unidentified_daemon_only_hears_providers_every_daemon_knows() {
    assert_eq!(
      provider_sent(&for_peer(&announce(MusicProvider::Plex), None)),
      MusicProvider::None
    );
    assert_eq!(
      provider_sent(&for_peer(&announce(MusicProvider::Spotify), None)),
      MusicProvider::Spotify
    );
  }

  #[test]
  fn other_frames_pass_through_untouched() {
    let data = announce(MusicProvider::Spotify);
    assert!(matches!(for_peer(&data, Some((0, 1, 0))), Cow::Borrowed(_)));
  }
}

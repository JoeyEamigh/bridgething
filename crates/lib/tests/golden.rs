use std::path::PathBuf;

use libbridgething::{
  gateway::*,
  wire::{MsgMeta as GatewayMsgMeta, ResponseMeta, WireError as GatewayError},
  *,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const HEADER_LEN: usize = 16;
const MAGIC: u16 = 0xdead;
const VERSION: u8 = 2;
const COMPRESSION_NONE: u8 = 0x00;
const ENCODING_MSGPACK: u8 = 0x00;
const PRIORITY_NORMAL: u8 = 0x00;
const PRIORITY_BULK: u8 = 0x01;
const PRIORITY_BACKGROUND: u8 = 0x02;

const FIXED_ID: &str = "0192f2a0-bbb0-7c00-a000-000000000001";
const FIXED_REQUEST_ID: &str = "0192f2a0-bbb0-7c00-a000-000000000099";
const FIXED_STOCK_WEBAPP: &str = "0192f2a0-bbb0-7c00-a000-000000000100";
const FIXED_DEMO_WEBAPP: &str = "0192f2a0-bbb0-7c00-a000-000000000101";

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct GoldenFile {
  version: u8,
  magic: String,
  header: HeaderSpec,
  fixtures: Vec<GoldenFixture>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct HeaderSpec {
  size_bytes: usize,
  fields: Vec<HeaderField>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct HeaderField {
  name: String,
  offset: usize,
  size: usize,
  description: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct GoldenFixture {
  name: String,
  description: String,
  direction: Direction,
  priority: String,
  decoded_json: serde_json::Value,
  msgpack_hex: String,
  framed_hex: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Direction {
  BridgeToGateway,
  GatewayToBridge,
}

fn fixture_path() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/golden.json")
}

fn header_spec() -> HeaderSpec {
  HeaderSpec {
    size_bytes: HEADER_LEN,
    fields: vec![
      HeaderField {
        name: "magic".into(),
        offset: 0,
        size: 2,
        description: "u16 BE, always 0xdead".into(),
      },
      HeaderField {
        name: "version".into(),
        offset: 2,
        size: 1,
        description: "u8 wire-format version".into(),
      },
      HeaderField {
        name: "compression".into(),
        offset: 3,
        size: 1,
        description: "u8: 0x00 none, 0x01 gzip".into(),
      },
      HeaderField {
        name: "encoding".into(),
        offset: 4,
        size: 1,
        description: "u8: 0x00 msgpack-named, 0x01 json".into(),
      },
      HeaderField {
        name: "priority".into(),
        offset: 5,
        size: 1,
        description: "u8: 0x00 normal, 0x01 bulk, 0x02 background - sender hint for transport-level scheduling. Unknown bytes decode as normal.".into(),
      },
      HeaderField {
        name: "reserved".into(),
        offset: 6,
        size: 2,
        description: "must be zero on encode, ignored on decode".into(),
      },
      HeaderField {
        name: "length".into(),
        offset: 8,
        size: 8,
        description: "u64 BE byte length of payload (post-compression)".into(),
      },
    ],
  }
}

fn frame(payload: &[u8], priority: u8) -> Vec<u8> {
  let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
  out.extend_from_slice(&MAGIC.to_be_bytes());
  out.push(VERSION);
  out.push(COMPRESSION_NONE);
  out.push(ENCODING_MSGPACK);
  out.push(priority);
  out.extend_from_slice(&[0, 0]); // reserved
  out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
  out.extend_from_slice(payload);
  out
}

fn priority_label(byte: u8) -> &'static str {
  match byte {
    PRIORITY_BULK => "bulk",
    PRIORITY_BACKGROUND => "background",
    _ => "normal",
  }
}

fn hex(bytes: &[u8]) -> String {
  let mut s = String::with_capacity(bytes.len() * 2);
  for b in bytes {
    s.push_str(&format!("{b:02x}"));
  }
  s
}

fn id() -> Uuid {
  FIXED_ID.parse().unwrap()
}

fn req_id() -> Uuid {
  FIXED_REQUEST_ID.parse().unwrap()
}

fn bridge_meta() -> BridgeThingMeta {
  BridgeThingMeta {
    bridgething_version: "0.1.0".into(),
    libbridgething_version: "v0.1.0".into(),
    app_name: "bridgething".into(),
    nickname: Some("Joey's kitchen".into()),
    app_version: "0.1.0".into(),
    daemon_sha256: Some("2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae".into()),
    wakeword_model_version: Some("1.0.0".into()),
    os_name: "linux".into(),
    os_version: "6.19".into(),
    os_description: "bridgething wrynose".into(),
    bt_mac: "aa:bb:cc:dd:ee:ff".into(),
    serial_number: "GOLDEN-0001".into(),
    fcc_id: "fcc-test".into(),
    ic_id: "ic-test".into(),
    model_name: "Car Thing".into(),
    channel: "stable".into(),
    image_variant: "prod".into(),
    image_version: "2026.05.0".into(),
    image_build_id: "golden-build-id".into(),
    image_build_date: "2026-04-27T00:00:00Z".into(),
    image_distro: "bridgething".into(),
    image_machine: "superbird".into(),
    discord: "https://discord.example".into(),
    credits: "the car thing scene".into(),
  }
}

fn gateway_capabilities() -> GatewayCapabilities {
  GatewayCapabilities {
    gateway: GatewayInfo {
      address: "00:11:22:33:44:55".into(),
      name: "Joey's iPhone".into(),
      os_name: "iOS".into(),
      app_name: "bridgething-mobile".into(),
      app_version: "1.0.0".into(),
      adapter_version: "1.0.0".into(),
      lib_version: "1.0.0".into(),
      libbridgething_version: "v0.1.0".into(),
    },
    uri_schemes: vec!["spotify".into()],
    network: NetworkInfo {
      kind: NetworkKind::Wifi,
      metered: false,
    },
    available: SurfaceAvailability {
      geo: false,
      notifications: false,
      net_fetch: true,
      net_ws: true,
      audio_tts: false,
      lyrics: true,
      playback_targets: true,
      forward: false,
    },
    audio: AudioCapabilities {
      earcons: vec![],
      voices: vec![],
    },
    music_provider: MusicProvider::Spotify,
  }
}

fn fingerprint_bytes() -> Vec<u8> {
  vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
}

fn build_fixtures() -> Vec<(GoldenFixture, Vec<u8>)> {
  let mut out = Vec::new();

  out.push(bridge_fixture(
    "bridge_to_gateway/version-event",
    "daemon announcing its version + hardware metadata as an event",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: bridge_meta().into(),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/ack-response",
    "ack to a request - meta carries requestId, data is the unit Ack variant",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Ack,
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/done-response",
    "completion response to a long-running command",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Done,
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/asset-request-request",
    "daemon asks the companion for an asset id the cache missed",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Request,
      data: BridgeToGatewayMsgData::Asset(BridgeToGatewayAssetMsg::Request(AssetRequest {
        id: "spotify/track/abc/image".into(),
        request_id: req_id(),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/webapp-list-response",
    "response to a webapp List request - bundles + their source",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::Webapps(WebappList {
        webapps: vec![
          WebappInfo {
            id: FIXED_STOCK_WEBAPP.parse().unwrap(),
            name: "Stock".into(),
            source: WebappSource::Builtin,
            role: WebappRole::Standard,
            version: "8.9.2".into(),
            description: Some("Spotify Car Thing stock UI".into()),
            icon_hash: None,
            settings_hash: None,
            overlay_hash: None,
            config: vec![],
            permissions: vec![],
            renders_voice_display: false,
            art: None,
            provenance: None,
            extension: None,
          },
          WebappInfo {
            id: FIXED_DEMO_WEBAPP.parse().unwrap(),
            name: "Demo".into(),
            source: WebappSource::Installed,
            role: WebappRole::Standard,
            version: "0.1.0".into(),
            description: None,
            icon_hash: Some("2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae".into()),
            settings_hash: Some("fcde2b2edba56bf408601fb721fe9b5c338d10ee429ea04fae5511b68fbf8fb9".into()),
            overlay_hash: None,
            config: vec![],
            permissions: vec![],
            renders_voice_display: false,
            art: None,
            provenance: Some("https://apps.bridgething.com/catalog.json".into()),
            extension: Some(ExtensionInfo {
              permissions: vec![
                "all".parse().expect("descriptor parses"),
                "net:example.com:443".parse().expect("descriptor parses"),
              ],
              api: 1,
            }),
          },
        ],
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/webapp-active-response",
    "response to a webapp GetActive request - currently active app id + display name",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::Active(WebappActive {
        id: Some(FIXED_STOCK_WEBAPP.parse().unwrap()),
        name: Some("Stock".into()),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/webapp-switched-response",
    "broadcast event after the kiosk switches to a new webapp",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::Switched(WebappActive {
        id: Some(FIXED_DEMO_WEBAPP.parse().unwrap()),
        name: Some("Demo".into()),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/webapp-installed-event",
    "broadcast event after a chunked install completed - full metadata of the freshly installed app",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::WebappInstalled(WebappInfo {
        id: FIXED_DEMO_WEBAPP.parse().unwrap(),
        name: "Demo".into(),
        source: WebappSource::Installed,
        role: WebappRole::Standard,
        version: "0.1.0".into(),
        description: None,
        icon_hash: None,
        settings_hash: None,
        overlay_hash: None,
        config: vec![],
        permissions: vec![],
        renders_voice_display: false,
        art: None,
        provenance: Some("https://apps.bridgething.com/catalog.json".into()),
        extension: None,
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/webapp-uninstalled-response",
    "response to an Uninstall request - active webapp after the uninstall settled",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::Uninstalled(WebappActive {
        id: Some(FIXED_STOCK_WEBAPP.parse().unwrap()),
        name: Some("Stock".into()),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/webapp-error-unknown",
    "domain error: requested webapp id is not installed",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Webapp(BridgeToGatewayWebappMsg::WebappError(WebappError::WebappNotFound {
        id: FIXED_DEMO_WEBAPP.into(),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/error-unsupported",
    "protocol error: bridge does not implement this request variant",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Error(GatewayError::Unsupported),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/error-handler-failed",
    "protocol error: handler hit an unexpected internal failure",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: BridgeToGatewayMsgData::Error(GatewayError::HandlerFailed {
        reason: "disk write failed".into(),
      }),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/forward-text-event",
    "arbitrary text payload over the Forward escape hatch",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: BridgeToGatewayMsgData::Forward(BridgeToGatewayForwardMsg::Routed(ForwardRouted {
        webapp: FIXED_DEMO_WEBAPP.parse().unwrap(),
        message: ForwardMessage::Text("hello, gateway".into()),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/forward-json-event",
    "arbitrary JSON payload over the Forward escape hatch",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: BridgeToGatewayMsgData::Forward(BridgeToGatewayForwardMsg::Routed(ForwardRouted {
        webapp: FIXED_DEMO_WEBAPP.parse().unwrap(),
        message: ForwardMessage::Json(serde_json::json!({
          "kind": "playback-changed",
          "payload": { "playing": true, "positionMs": 12345 }
        })),
      })),
    },
  ));

  out.push(bridge_fixture(
    "bridge_to_gateway/forward-binary-event",
    "raw bytes over the Forward escape hatch - verifies msgpack bin tag, not base64 string",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: BridgeToGatewayMsgData::Forward(BridgeToGatewayForwardMsg::Routed(ForwardRouted {
        webapp: FIXED_DEMO_WEBAPP.parse().unwrap(),
        message: ForwardMessage::Binary(fingerprint_bytes()),
      })),
    },
  ));

  out.push(bridge_fixture_with(
    "bridge_to_gateway/forward-binary-bulk-event",
    "same Forward.Binary payload but framed on the Bulk priority lane - exercises the priority byte at header offset 5",
    BridgeToGatewayMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: BridgeToGatewayMsgData::Forward(BridgeToGatewayForwardMsg::Routed(ForwardRouted {
        webapp: FIXED_DEMO_WEBAPP.parse().unwrap(),
        message: ForwardMessage::Binary(fingerprint_bytes()),
      })),
    },
    PRIORITY_BULK,
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/capabilities-announce-event",
    "phone announcing its gateway capabilities at session-up",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(gateway_capabilities())),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/asset-clear-event",
    "companion drops a previously pushed asset",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Asset(GatewayToBridgeAssetMsg::Clear(AssetClear {
        id: "spotify/track/abc/image".into(),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/asset-got-inline-response",
    "asset reply small enough to ride inline - TransferBody.inline arm",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: GatewayToBridgeMsgData::Asset(GatewayToBridgeAssetMsg::Got(AssetGotReply {
        id: "spotify/img/248/cover".into(),
        mime: Some("image/jpeg".into()),
        body: TransferBody::Inline(fingerprint_bytes()),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/asset-got-stream-response",
    "asset reply declaring a fragment stream - TransferBody.stream arm, ref id = request id",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: GatewayToBridgeMsgData::Asset(GatewayToBridgeAssetMsg::Got(AssetGotReply {
        id: "spotify/img/248/cover".into(),
        mime: Some("image/jpeg".into()),
        body: TransferBody::Stream(TransferRef {
          id: req_id(),
          total_size: 81920,
          sha256: None,
        }),
      })),
    },
  ));

  out.push(gateway_fixture_with(
    "gateway_to_bridge/transfer-fragment-event",
    "one offset-addressed slice of a fragment stream, framed on the Bulk lane",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(TransferFragment {
        transfer_id: req_id(),
        offset: 4096,
        bytes: fingerprint_bytes().into(),
      })),
    },
    PRIORITY_BULK,
  ));

  out.push(gateway_fixture_with(
    "gateway_to_bridge/transfer-fragment-background-event",
    "same fragment shape framed on the Background lane - exercises priority byte 0x02",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Fragment(TransferFragment {
        transfer_id: req_id(),
        offset: 0,
        bytes: fingerprint_bytes().into(),
      })),
    },
    PRIORITY_BACKGROUND,
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/transfer-abandon-event",
    "sender aborts an in-flight fragment stream",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Transfer(GatewayToBridgeTransferMsg::Abandon(TransferAbandon {
        transfer_id: req_id(),
        reason: "source asset evicted mid-stream".into(),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/authority-claim-now-playing-metadata",
    "companion claims authority over the now-playing metadata scope",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Authority(GatewayToBridgeAuthorityMsg::Claim(AuthorityClaim {
        scope: CompanionAuthorityScope::NowPlayingMetadata,
        app_bundle: Some("com.spotify.client".to_string()),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/authority-release-now-playing-playback",
    "companion releases authority over the playback scope (e.g. user switched apps)",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Event,
      data: GatewayToBridgeMsgData::Authority(GatewayToBridgeAuthorityMsg::Release(AuthorityRelease {
        scope: CompanionAuthorityScope::NowPlayingPlayback,
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/webapp-list-request",
    "gateway asks the daemon to enumerate installed and built-in webapps",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Request,
      data: GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::List),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/webapp-get-active-request",
    "gateway asks the daemon which webapp is currently active",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Request,
      data: GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::GetActive),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/webapp-switch-to-command",
    "gateway tells the daemon to swap the kiosk to a different webapp",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Command,
      data: GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::SwitchTo(WebappSwitchTo {
        id: FIXED_DEMO_WEBAPP.parse().unwrap(),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/webapp-uninstall-command",
    "gateway tells the daemon to remove a previously-installed webapp",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Command,
      data: GatewayToBridgeMsgData::Webapp(GatewayToBridgeWebappMsg::Uninstall(WebappUninstall {
        id: FIXED_DEMO_WEBAPP.parse().unwrap(),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/chrome-navigate-command",
    "gateway driving the Car Thing chromium kiosk to a new URL",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Command,
      data: GatewayToBridgeMsgData::Chrome(GatewayToBridgeChromeMsg::Navigate(ChromeNavigate {
        url: "https://example.com".into(),
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/library-browse-reply",
    "companion answering a browse with one track and one album - pins the field casing of the nested Track / Album / Artist shapes",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: GatewayToBridgeMsgData::Library(GatewayToBridgeLibraryMsg::BrowseReply(BrowseReply {
        result: BrowseResult {
          entries: vec![
            BrowseEntry::Item(LibraryItem::Track(Track {
              id: "spotify:track:golden".into(),
              name: "Golden".into(),
              album: Album {
                id: "spotify:album:golden".into(),
                name: "Golden Album".into(),
                artwork_id: Some("spotify/img/640/album".into()),
              },
              artist: Artist {
                id: "spotify:artist:golden".into(),
                name: "Golden Artist".into(),
                artwork_id: Some("spotify/img/640/artist".into()),
              },
              artists: vec![Artist {
                id: "spotify:artist:golden".into(),
                name: "Golden Artist".into(),
                artwork_id: None,
              }],
              duration_ms: 185_741,
              image_id: "spotify/img/640/track".into(),
              saved: true,
            })),
            BrowseEntry::Folder(BrowseFolder {
              node_id: "spotify:playlist:golden".into(),
              title: "Golden Playlist".into(),
              subtitle: Some("12 songs".into()),
              artwork_id: Some("spotify/img/640/playlist".into()),
              total: Some(12),
              preview_children: None,
            }),
          ],
          total: Some(2),
          has_more: false,
        },
      })),
    },
  ));

  out.push(gateway_fixture(
    "gateway_to_bridge/error-malformed",
    "protocol error: gateway could not decode the request the daemon sent",
    GatewayToBridgeMsg {
      id: id(),
      meta: GatewayMsgMeta::Response(ResponseMeta { request_id: req_id() }),
      data: GatewayToBridgeMsgData::Error(GatewayError::Malformed {
        reason: "missing required field".into(),
      }),
    },
  ));

  out
}

fn bridge_fixture(name: &str, description: &str, msg: BridgeToGatewayMsg) -> (GoldenFixture, Vec<u8>) {
  bridge_fixture_with(name, description, msg, PRIORITY_NORMAL)
}

fn bridge_fixture_with(
  name: &str,
  description: &str,
  msg: BridgeToGatewayMsg,
  priority: u8,
) -> (GoldenFixture, Vec<u8>) {
  let packed = rmp_serde::to_vec_named(&msg).expect("encode bridge msg");
  let framed = frame(&packed, priority);
  let decoded_json = serde_json::to_value(&msg).expect("re-encode as json");
  let fix = GoldenFixture {
    name: name.into(),
    description: description.into(),
    direction: Direction::BridgeToGateway,
    priority: priority_label(priority).into(),
    decoded_json,
    msgpack_hex: hex(&packed),
    framed_hex: hex(&framed),
  };
  (fix, packed)
}

fn gateway_fixture(name: &str, description: &str, msg: GatewayToBridgeMsg) -> (GoldenFixture, Vec<u8>) {
  gateway_fixture_with(name, description, msg, PRIORITY_NORMAL)
}

fn gateway_fixture_with(
  name: &str,
  description: &str,
  msg: GatewayToBridgeMsg,
  priority: u8,
) -> (GoldenFixture, Vec<u8>) {
  let packed = rmp_serde::to_vec_named(&msg).expect("encode gateway msg");
  let framed = frame(&packed, priority);
  let decoded_json = serde_json::to_value(&msg).expect("re-encode as json");
  let fix = GoldenFixture {
    name: name.into(),
    description: description.into(),
    direction: Direction::GatewayToBridge,
    priority: priority_label(priority).into(),
    decoded_json,
    msgpack_hex: hex(&packed),
    framed_hex: hex(&framed),
  };
  (fix, packed)
}

fn current() -> GoldenFile {
  GoldenFile {
    version: VERSION,
    magic: format!("0x{:04x}", MAGIC),
    header: header_spec(),
    fixtures: build_fixtures().into_iter().map(|(f, _)| f).collect(),
  }
}

#[test]
fn golden_vectors_match_fixture_file() {
  let current = current();

  if std::env::var("UPDATE_GOLDEN").is_ok() {
    let json = serde_json::to_string_pretty(&current).expect("serialize golden file");
    std::fs::write(fixture_path(), format!("{json}\n")).expect("write fixture file");
    eprintln!(
      "wrote {} fixtures to {}",
      current.fixtures.len(),
      fixture_path().display()
    );
    return;
  }

  let on_disk = std::fs::read_to_string(fixture_path()).unwrap_or_else(|err| {
    panic!(
      "failed to read {}: {err}\nrun `just goldens` (or `UPDATE_GOLDEN=1 cargo test -p libbridgething --test golden`) to generate it",
      fixture_path().display()
    )
  });
  let parsed: GoldenFile = serde_json::from_str(&on_disk).expect("parse golden file");

  assert_eq!(
    parsed, current,
    "golden fixtures drifted from Rust source - run `just goldens` to regenerate"
  );
}

#[test]
fn golden_fixtures_round_trip_through_rust_codec() {
  for (fix, packed) in build_fixtures() {
    match fix.direction {
      Direction::BridgeToGateway => {
        let decoded: BridgeToGatewayMsg =
          rmp_serde::from_slice(&packed).unwrap_or_else(|err| panic!("decode {}: {err}", fix.name));
        let re_encoded =
          rmp_serde::to_vec_named(&decoded).unwrap_or_else(|err| panic!("re-encode {}: {err}", fix.name));
        assert_eq!(packed, re_encoded, "{} did not round-trip", fix.name);
      }
      Direction::GatewayToBridge => {
        let decoded: GatewayToBridgeMsg =
          rmp_serde::from_slice(&packed).unwrap_or_else(|err| panic!("decode {}: {err}", fix.name));
        let re_encoded =
          rmp_serde::to_vec_named(&decoded).unwrap_or_else(|err| panic!("re-encode {}: {err}", fix.name));
        assert_eq!(packed, re_encoded, "{} did not round-trip", fix.name);
      }
    }
  }
}

#[test]
fn priority_round_trips_through_codec_on_all_lanes() {
  use libbridgething::{
    Priority,
    protocol::{BridgeEndec, GatewayEndec, PrioritizedFrame},
  };
  use tokio_util::{
    bytes::BytesMut,
    codec::{Decoder, Encoder},
  };

  let bridge_msg = BridgeToGatewayMsg {
    id: id(),
    meta: GatewayMsgMeta::Event,
    data: BridgeToGatewayMsgData::Forward(BridgeToGatewayForwardMsg::Routed(ForwardRouted {
      webapp: FIXED_DEMO_WEBAPP.parse().unwrap(),
      message: ForwardMessage::Binary(fingerprint_bytes()),
    })),
  };
  let gateway_msg = GatewayToBridgeMsg {
    id: id(),
    meta: GatewayMsgMeta::Event,
    data: GatewayToBridgeMsgData::Capabilities(GatewayToBridgeCapabilitiesMsg::Announce(gateway_capabilities())),
  };

  for priority in [Priority::Normal, Priority::Bulk, Priority::Background] {
    let mut wire = BytesMut::new();
    BridgeEndec::default()
      .encode(PrioritizedFrame::new(priority, bridge_msg.clone()), &mut wire)
      .expect("encode bridge");
    assert_eq!(wire[5], priority.as_byte(), "priority byte at offset 5");
    let decoded = GatewayEndec::default()
      .decode(&mut wire)
      .expect("decode bridge")
      .expect("frame ready")
      .frame()
      .expect("a decoded frame");
    assert_eq!(decoded.priority, priority, "decoded priority preserved");
    assert_eq!(decoded.msg, bridge_msg, "decoded payload matches");

    let mut wire = BytesMut::new();
    GatewayEndec::default()
      .encode(PrioritizedFrame::new(priority, gateway_msg.clone()), &mut wire)
      .expect("encode gateway");
    assert_eq!(wire[5], priority.as_byte());
    let decoded = BridgeEndec::default()
      .decode(&mut wire)
      .expect("decode gateway")
      .expect("frame ready")
      .frame()
      .expect("a decoded frame");
    assert_eq!(decoded.priority, priority);
    assert_eq!(decoded.msg, gateway_msg);
  }
}

#[test]
fn legacy_zero_priority_byte_decodes_as_normal() {
  use libbridgething::{
    Priority,
    protocol::{BridgeEndec, GatewayEndec},
  };
  use tokio_util::{
    bytes::BytesMut,
    codec::{Decoder, Encoder},
  };

  let msg = BridgeToGatewayMsg {
    id: id(),
    meta: GatewayMsgMeta::Event,
    data: BridgeToGatewayMsgData::Ack,
  };

  let mut wire = BytesMut::new();
  BridgeEndec::default()
    .encode(msg.clone(), &mut wire)
    .expect("encode bare msg");
  assert_eq!(wire[5], 0x00, "bare-msg encoder defaults to Normal");

  let decoded = GatewayEndec::default()
    .decode(&mut wire)
    .expect("decode")
    .expect("frame ready")
    .frame()
    .expect("a decoded frame");
  assert_eq!(decoded.priority, Priority::Normal);
  assert_eq!(decoded.msg, msg);
}

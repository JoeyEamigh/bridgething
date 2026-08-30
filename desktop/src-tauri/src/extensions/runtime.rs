use std::{
  fs,
  io::Write as _,
  path::{Path, PathBuf},
};

use bridgething_delivery::bundle::fetch::sha256_file;
use bridgething_io::{DownloadBody, HttpExecutor, HttpHeader, HttpMethod, HttpRequest};

pub const DENO_VERSION: &str = "2.9.6";

const DOWNLOAD_TIMEOUT_MS: u32 = 600_000;
const RUNTIME_DIR: &str = "runtime";
const CACHE_DIR: &str = "deno-cache";

pub struct DenoRelease {
  pub asset: &'static str,
  pub sha256: &'static str,
}

const RELEASES: &[(&str, &str, DenoRelease)] = &[
  (
    "macos",
    "aarch64",
    DenoRelease {
      asset: "deno-aarch64-apple-darwin.zip",
      sha256: "213a2f304f04d3c9cb5220669afad138f60a5aab1fe80962abdeb8f35807a472",
    },
  ),
  (
    "macos",
    "x86_64",
    DenoRelease {
      asset: "deno-x86_64-apple-darwin.zip",
      sha256: "7d4524b82bcc557fe020a1a5b56956ed42b992ae5b28026e8ad5d17329533f5f",
    },
  ),
  (
    "linux",
    "x86_64",
    DenoRelease {
      asset: "deno-x86_64-unknown-linux-gnu.zip",
      sha256: "394f07f4da2bebe6ce6f1e7ce0fa16429b29b08c35e3fac3fe25972676dff4b2",
    },
  ),
  (
    "linux",
    "aarch64",
    DenoRelease {
      asset: "deno-aarch64-unknown-linux-gnu.zip",
      sha256: "9a46afc6c392c7cd2ff71a31558935545b46408d0e87f7a86908c712721c046e",
    },
  ),
  (
    "windows",
    "x86_64",
    DenoRelease {
      asset: "deno-x86_64-pc-windows-msvc.zip",
      sha256: "15e5300b0ba3c3695a7621d90160a746ec9e710228cee639afa9d580f6e3cd11",
    },
  ),
  (
    "windows",
    "aarch64",
    DenoRelease {
      asset: "deno-aarch64-pc-windows-msvc.zip",
      sha256: "acb014afe2299847764e232b4993e162e3946cdeec36603e3f1a0b548cd1ea55",
    },
  ),
];

pub fn release(os: &str, arch: &str) -> Option<&'static DenoRelease> {
  RELEASES
    .iter()
    .find(|(candidate_os, candidate_arch, _)| *candidate_os == os && *candidate_arch == arch)
    .map(|(_, _, release)| release)
}

pub fn asset_url(asset: &str) -> String {
  format!("https://github.com/denoland/deno/releases/download/v{DENO_VERSION}/{asset}")
}

fn binary_name() -> &'static str {
  if cfg!(target_os = "windows") {
    "deno.exe"
  } else {
    "deno"
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenoRuntime {
  pub binary: PathBuf,
  pub cache: PathBuf,
}

fn layout(state_dir: &Path) -> DenoRuntime {
  let root = state_dir.join(RUNTIME_DIR);
  DenoRuntime {
    binary: root.join(format!("deno-{DENO_VERSION}")).join(binary_name()),
    cache: root.join(CACHE_DIR),
  }
}

pub async fn acquire(state_dir: &Path, http: &HttpExecutor) -> Result<DenoRuntime, String> {
  let os = std::env::consts::OS;
  let arch = std::env::consts::ARCH;
  let release = release(os, arch).ok_or_else(|| format!("no pinned deno build for {os} {arch}"))?;
  install(state_dir, http, release.asset, release.sha256).await
}

async fn install(state_dir: &Path, http: &HttpExecutor, asset: &str, sha256: &str) -> Result<DenoRuntime, String> {
  let runtime = layout(state_dir);
  if runtime.binary.is_file() {
    return Ok(runtime);
  }

  let root = state_dir.join(RUNTIME_DIR);
  fs::create_dir_all(&root).map_err(|error| format!("{}: {error}", root.display()))?;
  fs::create_dir_all(&runtime.cache).map_err(|error| format!("{}: {error}", runtime.cache.display()))?;

  let archive = root.join(format!("{asset}.part"));
  let url = asset_url(asset);
  tracing::info!(%url, "pulling the pinned deno runtime");
  download(http, &url, &archive).await.inspect_err(|_| {
    let _ = fs::remove_file(&archive);
  })?;

  let seen = sha256_file(&archive).map_err(|error| error.to_string())?;
  if seen != sha256 {
    let _ = fs::remove_file(&archive);
    return Err(format!("{asset} hashes {seen}, not the pinned {sha256}"));
  }

  let staging = root.join(format!("deno-{DENO_VERSION}.staging"));
  let _ = fs::remove_dir_all(&staging);
  unzip(&archive, &staging)?;
  let _ = fs::remove_file(&archive);

  let staged = staging.join(binary_name());
  if !staged.is_file() {
    let _ = fs::remove_dir_all(&staging);
    return Err(format!("{asset} holds no {}", binary_name()));
  }
  executable(&staged)?;

  let home = root.join(format!("deno-{DENO_VERSION}"));
  let _ = fs::remove_dir_all(&home);
  fs::rename(&staging, &home).map_err(|error| format!("{}: {error}", home.display()))?;
  tracing::info!(version = DENO_VERSION, path = %runtime.binary.display(), "the deno runtime is installed");

  Ok(runtime)
}

struct Spool {
  file: fs::File,
  status: u16,
}

impl DownloadBody for Spool {
  fn on_response(&mut self, status: u16, _headers: &[HttpHeader], _content_length: Option<u64>) -> bool {
    self.status = status;
    (200..300).contains(&status)
  }

  fn write(&mut self, chunk: &[u8]) -> Result<(), String> {
    self.file.write_all(chunk).map_err(|error| error.to_string())
  }
}

async fn download(http: &HttpExecutor, url: &str, into: &Path) -> Result<(), String> {
  let file = fs::File::create(into).map_err(|error| format!("{}: {error}", into.display()))?;
  let outcome = http
    .download(
      HttpRequest {
        method: HttpMethod::Get,
        url: url.to_owned(),
        headers: Vec::new(),
        body: Vec::new(),
        timeout_ms: DOWNLOAD_TIMEOUT_MS,
      },
      Box::new(Spool { file, status: 0 }),
    )
    .await
    .map_err(|error| error.to_string())?;

  if !outcome.ok() {
    return Err(format!("{url} answered {}", outcome.status));
  }
  Ok(())
}

fn unzip(archive: &Path, into: &Path) -> Result<(), String> {
  let file = fs::File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
  let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())?;
  for index in 0..zip.len() {
    let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
    let name = entry.name().to_owned();
    let out = super::store::contained(into, &name).ok_or_else(|| format!("{name} escapes the runtime directory"))?;
    if entry.is_dir() {
      fs::create_dir_all(&out).map_err(|error| error.to_string())?;
      continue;
    }
    if let Some(parent) = out.parent() {
      fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut sink = fs::File::create(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    std::io::copy(&mut entry, &mut sink).map_err(|error| error.to_string())?;
  }
  Ok(())
}

#[cfg(unix)]
fn executable(path: &Path) -> Result<(), String> {
  use std::os::unix::fs::PermissionsExt as _;
  fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> Result<(), String> {
  Ok(())
}

#[cfg(test)]
mod tests {
  use std::{io::Write as _, sync::Arc};

  use bridgething_io::{HttpDownloadSink, HttpSink, HttpTransport};

  use super::*;

  struct Serving(Option<Vec<u8>>);

  impl HttpTransport for Serving {
    fn execute(&self, _request: HttpRequest, sink: Arc<HttpSink>) {
      sink.fail("the runtime is only ever streamed".to_owned());
    }

    fn download(&self, _request: HttpRequest, sink: Arc<HttpDownloadSink>) {
      let Some(body) = self.0.clone() else {
        sink.on_response(404, Vec::new(), Some(0));
        sink.on_finished();
        return;
      };
      sink.on_response(200, Vec::new(), Some(body.len() as u64));
      sink.on_chunk(body);
      sink.on_finished();
    }
  }

  fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
    for (name, body) in entries {
      zip.start_file(*name, options).expect("an entry");
      zip.write_all(body).expect("the entry body");
    }
    zip.finish().expect("a finished archive").into_inner()
  }

  const ASSET: &str = "deno-test-target.zip";

  fn digest(bytes: &[u8], dir: &Path) -> String {
    let path = dir.join("hash-me");
    fs::write(&path, bytes).expect("the scratch copy");
    let seen = sha256_file(&path).expect("a digest");
    fs::remove_file(&path).expect("the scratch copy is cleaned up");
    seen
  }

  fn leftovers(state_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(state_dir.join(RUNTIME_DIR)) else {
      return Vec::new();
    };
    let mut held: Vec<String> = entries
      .flatten()
      .map(|entry| entry.file_name().to_string_lossy().into_owned())
      .filter(|name| name != CACHE_DIR)
      .collect();
    held.sort();
    held
  }

  #[tokio::test]
  async fn a_release_that_hashes_to_the_pinned_value_lands_runnable() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let bytes = archive(&[(binary_name(), b"#!/bin/sh\nexit 0\n")]);
    let http = HttpExecutor::new(Arc::new(Serving(Some(bytes.clone()))));

    let runtime = install(dir.path(), &http, ASSET, &digest(&bytes, dir.path()))
      .await
      .expect("the pinned hash matches, so the runtime installs");

    assert_eq!(runtime, layout(dir.path()));
    assert!(runtime.binary.is_file(), "the spawn looks for the binary at this path");
    assert!(runtime.cache.is_dir(), "npm: downloads need DENO_DIR to exist");
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt as _;
      let mode = fs::metadata(&runtime.binary).expect("the binary").permissions().mode();
      assert_eq!(mode & 0o111, 0o111, "an unexecutable deno spawns nothing");
    }
    assert_eq!(
      leftovers(dir.path()),
      vec![format!("deno-{DENO_VERSION}")],
      "the archive and the staging directory are cleaned up"
    );
  }

  #[tokio::test]
  async fn a_release_that_does_not_hash_to_the_pinned_value_leaves_nothing_behind() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let bytes = archive(&[(binary_name(), b"not the pinned build")]);
    let http = HttpExecutor::new(Arc::new(Serving(Some(bytes))));

    let failure = install(dir.path(), &http, ASSET, &"0".repeat(64))
      .await
      .expect_err("a build that is not the pinned one must never run");

    assert!(failure.contains("hashes"), "the failure names the mismatch: {failure}");
    assert!(
      leftovers(dir.path()).is_empty(),
      "an unverified archive must not be left where the next attempt could adopt it"
    );
    assert!(
      !layout(dir.path()).binary.is_file(),
      "nothing runnable is left where the next attempt could adopt it"
    );
  }

  #[tokio::test]
  async fn an_archive_without_the_binary_is_refused_after_it_verifies() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let bytes = archive(&[("LICENSE", b"not a binary")]);
    let http = HttpExecutor::new(Arc::new(Serving(Some(bytes.clone()))));

    let failure = install(dir.path(), &http, ASSET, &digest(&bytes, dir.path()))
      .await
      .expect_err("a verified archive that holds no deno is still no runtime");

    assert!(failure.contains(binary_name()), "{failure}");
    assert!(leftovers(dir.path()).is_empty());
  }

  #[tokio::test]
  async fn a_download_that_does_not_answer_leaves_no_part_file() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let http = HttpExecutor::new(Arc::new(Serving(None)));

    let failure = install(dir.path(), &http, ASSET, &"0".repeat(64))
      .await
      .expect_err("offline is the visible failure the app row shows");

    assert!(failure.contains("404"), "{failure}");
    assert!(
      leftovers(dir.path()).is_empty(),
      "a half-written .part must not be mistaken for an archive on the retry"
    );
    assert!(!layout(dir.path()).binary.is_file());
  }

  #[test]
  fn every_target_the_desktop_ships_on_has_a_pinned_build() {
    for (os, arch) in [
      ("macos", "aarch64"),
      ("macos", "x86_64"),
      ("linux", "x86_64"),
      ("linux", "aarch64"),
      ("windows", "x86_64"),
      ("windows", "aarch64"),
    ] {
      let release = release(os, arch).unwrap_or_else(|| panic!("{os} {arch} has no pinned deno"));
      assert_eq!(release.sha256.len(), 64, "{} has no sha256", release.asset);
      assert!(
        release
          .sha256
          .chars()
          .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "{} records its hash in a form the comparison will never match",
        release.asset
      );
    }
  }

  #[test]
  fn the_host_this_binary_runs_on_is_one_of_them() {
    assert!(
      release(std::env::consts::OS, std::env::consts::ARCH).is_some(),
      "a desktop build with no runtime can never start an extension"
    );
  }

  #[test]
  fn no_two_targets_share_an_asset() {
    let mut assets: Vec<&str> = RELEASES.iter().map(|(_, _, release)| release.asset).collect();
    assets.sort_unstable();
    let count = assets.len();
    assets.dedup();
    assert_eq!(
      assets.len(),
      count,
      "a copy-pasted row would silently run the wrong build"
    );
  }

  #[test]
  fn the_download_url_points_at_the_pinned_tag() {
    assert_eq!(
      asset_url("deno-aarch64-apple-darwin.zip"),
      format!("https://github.com/denoland/deno/releases/download/v{DENO_VERSION}/deno-aarch64-apple-darwin.zip")
    );
  }
}

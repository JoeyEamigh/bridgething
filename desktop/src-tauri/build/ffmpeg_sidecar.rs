use std::{
  env, fs,
  io::{self, Read, Write},
  path::{Path, PathBuf},
  process,
  time::Duration,
};

use sha2::{Digest, Sha256};

const OVERRIDE: &str = "BRIDGETHING_FFMPEG_SIDECAR";
const OFFLINE: &str = "CARGO_NET_OFFLINE";
const RECIPE: [&str; 2] = ["scripts/ffmpeg-sidecar.Dockerfile", "scripts/ffmpeg-sidecar.sh"];
const MIRROR: &str = "https://ota.bridgething.com/ffmpeg-sidecar";
const NOTICE: &str = "bridgething-ffmpeg-NOTICE.txt";
const TARGETS: [&str; 4] = [
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
  "aarch64-pc-windows-msvc",
];
const MAX_DOWNLOAD: u64 = 256 << 20;

pub fn place_on_linux_and_windows() {
  if !matches!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("linux" | "windows")) {
    return;
  }
  let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"));
  let target = env::var("TARGET").expect("TARGET is set for build scripts");
  let sidecar = Sidecar::new(&manifest, target);

  let adopted = env::var_os(OVERRIDE).map(PathBuf::from);
  let provenance = adopted.clone().unwrap_or_else(|| sidecar.stamp());

  println!("cargo:rerun-if-env-changed={OVERRIDE}");
  println!("cargo:rerun-if-env-changed={OFFLINE}");
  for file in RECIPE {
    println!("cargo:rerun-if-changed={}", sidecar.root.join(file).display());
  }
  for file in [sidecar.binary(), provenance, sidecar.notice()] {
    println!("cargo:rerun-if-changed={}", file.display());
  }

  let placed = match adopted {
    Some(path) => sidecar.adopt(&path),
    None => sidecar.ensure(),
  };
  if let Err(reason) = placed {
    panic!("{}", sidecar.failure(&reason));
  }
}

struct Sidecar {
  root: PathBuf,
  dir: PathBuf,
  target: String,
  name: String,
  recipe: String,
}

impl Sidecar {
  fn new(manifest: &Path, target: String) -> Self {
    let root = manifest.ancestors().nth(2).unwrap_or(manifest).to_path_buf();
    let exe = if target.contains("-windows-") { ".exe" } else { "" };
    let recipe = recipe(&root).unwrap_or_else(|reason| panic!("the ffmpeg sidecar recipe is unreadable: {reason}"));
    Self {
      dir: manifest.join("binaries"),
      name: format!("bridgething-ffmpeg-{target}{exe}"),
      root,
      target,
      recipe,
    }
  }

  fn binary(&self) -> PathBuf {
    self.dir.join(&self.name)
  }

  fn stamp(&self) -> PathBuf {
    self.dir.join(format!("{}.recipe", self.name))
  }

  fn notice(&self) -> PathBuf {
    self.dir.join(NOTICE)
  }

  fn url(&self, file: &str) -> String {
    format!("{MIRROR}/{}/{file}", self.recipe)
  }

  fn built_from(&self) -> Option<String> {
    fs::read_to_string(self.stamp())
      .ok()
      .map(|stamp| stamp.trim().to_owned())
  }

  fn ensure(&self) -> Result<(), String> {
    let recipe = self.recipe.as_str();
    let present = self.binary().is_file() && self.notice().is_file();
    if present && self.built_from().as_deref() == Some(recipe) {
      return mark_executable(&self.binary());
    }
    if !TARGETS.contains(&self.target.as_str()) {
      return Err(format!("no sidecar recipe builds {}", self.target));
    }
    if offline() {
      if present {
        println!(
          "cargo:warning=bundling the local ffmpeg sidecar built from recipe {}, not the current recipe {recipe}, because {OFFLINE} forbids fetching it",
          self.built_from().unwrap_or_else(|| "unknown".to_owned())
        );
        return mark_executable(&self.binary());
      }
      return Err(format!("{OFFLINE} is set, so it cannot be fetched"));
    }
    install(&self.fetch_binary()?, &self.binary())?;
    install(&self.fetch_notice()?, &self.notice())?;
    write(&self.stamp(), format!("{recipe}\n").as_bytes())?;
    mark_executable(&self.binary())
  }

  fn adopt(&self, path: &Path) -> Result<(), String> {
    if !path.is_file() {
      return Err(format!("{OVERRIDE} points at {}, which is not a file", path.display()));
    }
    install(path, &self.binary())?;
    remove(&self.stamp())?;
    let sibling = path.with_file_name(NOTICE);
    if sibling.is_file() {
      install(&sibling, &self.notice())?;
    } else if !self.notice().is_file() {
      if offline() {
        return Err(format!(
          "no {NOTICE} sits beside {}, and {OFFLINE} forbids fetching one",
          path.display()
        ));
      }
      install(&self.fetch_notice()?, &self.notice())?;
    }
    mark_executable(&self.binary())
  }

  fn cache(&self) -> Result<PathBuf, String> {
    let dir = cache_root().join(&self.recipe);
    fs::create_dir_all(&dir).map_err(|error| format!("creating {} failed: {error}", dir.display()))?;
    Ok(dir)
  }

  fn fetch_binary(&self) -> Result<PathBuf, String> {
    let cached = self.cache()?.join(&self.name);
    if cached.is_file() {
      return Ok(cached);
    }
    let checksum_url = self.url(&format!("{}.sha256", self.name));
    let checksum = download(&checksum_url)?;
    let expected = std::str::from_utf8(&checksum)
      .ok()
      .and_then(|text| text.split_whitespace().next())
      .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
      .ok_or_else(|| format!("{checksum_url} holds no sha256"))?
      .to_ascii_lowercase();
    let url = self.url(&self.name);
    let body = download(&url)?;
    let actual = hex(&Sha256::digest(&body));
    if actual != expected {
      return Err(format!("{url} hashed to {actual}, but its .sha256 says {expected}"));
    }
    write(&cached, &body)?;
    Ok(cached)
  }

  fn fetch_notice(&self) -> Result<PathBuf, String> {
    let cached = self.cache()?.join(NOTICE);
    if !cached.is_file() {
      write(&cached, &download(&self.url(NOTICE))?)?;
    }
    Ok(cached)
  }

  fn failure(&self, reason: &str) -> String {
    let state = match (self.binary().is_file(), self.built_from()) {
      (false, _) => "it is missing".to_owned(),
      (true, Some(built)) => format!("the copy there was built from recipe {built}"),
      (true, None) => "the copy there carries no recipe stamp".to_owned(),
    };
    format!(
      "\n\nbridgething-desktop bundles the ffmpeg sidecar for {target} at\n  {binary}\nand {state}.\n\n\
       recipe:  {recipe} (sha256 of {dockerfile} then {script}, first 16 hex)\n\
       mirror:  {url}\n\
       failed:  {reason}\n\n\
       build it locally:  just ffmpeg-sidecar {target}\n\
       or bring your own: {OVERRIDE}=/path/to/{name} cargo build\n\
       a stale local copy is kept when {OFFLINE}=true\n\n",
      target = self.target,
      recipe = self.recipe,
      binary = self.binary().display(),
      dockerfile = RECIPE[0],
      script = RECIPE[1],
      url = self.url(&self.name),
      name = self.name,
    )
  }
}

fn recipe(root: &Path) -> Result<String, String> {
  let mut digest = Sha256::new();
  for file in RECIPE {
    let path = root.join(file);
    digest.update(fs::read(&path).map_err(|error| format!("reading {} failed: {error}", path.display()))?);
  }
  let mut recipe = hex(&digest.finalize());
  recipe.truncate(16);
  Ok(recipe)
}

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn offline() -> bool {
  env::var(OFFLINE).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn cache_root() -> PathBuf {
  dirs::cache_dir()
    .map(|dir| dir.join("bridgething"))
    .unwrap_or_else(|| PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set for build scripts")))
    .join("ffmpeg-sidecar")
}

fn download(url: &str) -> Result<Vec<u8>, String> {
  let agent: ureq::Agent = ureq::Agent::config_builder()
    .timeout_global(Some(Duration::from_secs(600)))
    .https_only(true)
    .build()
    .into();
  let response = agent.get(url).call().map_err(|error| match error {
    ureq::Error::StatusCode(404) => format!("{url} is not published (404)"),
    error => format!("fetching {url} failed: {error}"),
  })?;
  let mut body = Vec::new();
  response
    .into_body()
    .into_with_config()
    .limit(MAX_DOWNLOAD)
    .reader()
    .read_to_end(&mut body)
    .map_err(|error| format!("reading {url} failed: {error}"))?;
  Ok(body)
}

fn staging(dest: &Path) -> PathBuf {
  let mut name = dest.file_name().unwrap_or_default().to_os_string();
  name.push(format!(".{}.part", process::id()));
  dest.with_file_name(name)
}

fn write(dest: &Path, body: &[u8]) -> Result<(), String> {
  let part = staging(dest);
  let written = fs::File::create(&part)
    .and_then(|mut file| file.write_all(body).and_then(|()| file.sync_all()))
    .and_then(|()| fs::rename(&part, dest));
  written.map_err(|error| {
    let _ = fs::remove_file(&part);
    format!("writing {} failed: {error}", dest.display())
  })
}

fn install(source: &Path, dest: &Path) -> Result<(), String> {
  if identical(source, dest) {
    return Ok(());
  }
  let parent = dest.parent().expect("sidecar paths have a parent directory");
  fs::create_dir_all(parent).map_err(|error| format!("creating {} failed: {error}", parent.display()))?;
  let part = staging(dest);
  fs::copy(source, &part)
    .and_then(|_| fs::rename(&part, dest))
    .map_err(|error| {
      let _ = fs::remove_file(&part);
      format!("copying {} to {} failed: {error}", source.display(), dest.display())
    })
}

fn identical(source: &Path, dest: &Path) -> bool {
  matches!((fs::read(source), fs::read(dest)), (Ok(wanted), Ok(held)) if wanted == held)
}

fn remove(path: &Path) -> Result<(), String> {
  match fs::remove_file(path) {
    Err(error) if error.kind() != io::ErrorKind::NotFound => {
      Err(format!("removing {} failed: {error}", path.display()))
    }
    _ => Ok(()),
  }
}

#[cfg(unix)]
fn mark_executable(path: &Path) -> Result<(), String> {
  use std::os::unix::fs::PermissionsExt;
  let mut permissions = fs::metadata(path)
    .map_err(|error| format!("reading {} failed: {error}", path.display()))?
    .permissions();
  if permissions.mode() & 0o111 != 0o111 {
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)
      .map_err(|error| format!("marking {} executable failed: {error}", path.display()))?;
  }
  Ok(())
}

#[cfg(not(unix))]
fn mark_executable(_: &Path) -> Result<(), String> {
  Ok(())
}

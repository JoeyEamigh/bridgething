use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use ts_rs::TS;

pub const EXTENSION_API_VERSION: u32 = 1;

/// `kind` or `kind:scope`, from `all`, `net`, `read`, `write`, `run`, `env`, `sys`, `ffi`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, SerializeDisplay, DeserializeFromStr)]
pub enum ExtensionPermission {
  All,
  Net(Option<String>),
  Read(Option<String>),
  Write(Option<String>),
  Run(Option<String>),
  Env(Option<String>),
  Sys(Option<String>),
  Ffi(Option<String>),
}

/// A rejected descriptor. A scope must be non-empty and comma-free, and `all` takes no scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPermissionParseError {
  pub descriptor: String,
  pub reason: String,
}

impl fmt::Display for ExtensionPermissionParseError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "invalid permission {:?}: {}", self.descriptor, self.reason)
  }
}

impl std::error::Error for ExtensionPermissionParseError {}

impl ExtensionPermission {
  fn kind(&self) -> &'static str {
    match self {
      Self::All => "all",
      Self::Net(_) => "net",
      Self::Read(_) => "read",
      Self::Write(_) => "write",
      Self::Run(_) => "run",
      Self::Env(_) => "env",
      Self::Sys(_) => "sys",
      Self::Ffi(_) => "ffi",
    }
  }

  fn scope(&self) -> Option<&str> {
    match self {
      Self::All => None,
      Self::Net(s) | Self::Read(s) | Self::Write(s) | Self::Run(s) | Self::Env(s) | Self::Sys(s) | Self::Ffi(s) => {
        s.as_deref()
      }
    }
  }

  pub fn deno_flags(permissions: &[Self]) -> Vec<String> {
    if permissions.iter().any(|p| matches!(p, Self::All)) {
      return vec!["--allow-all".to_string()];
    }

    const KINDS: &[&str] = &["net", "read", "write", "run", "env", "sys", "ffi"];
    let mut flags = Vec::new();
    for kind in KINDS {
      let mut scopes: Vec<&str> = Vec::new();
      let mut bare = false;
      let mut present = false;
      for permission in permissions.iter().filter(|p| p.kind() == *kind) {
        present = true;
        match permission.scope() {
          None => bare = true,
          Some(scope) if !scopes.contains(&scope) => scopes.push(scope),
          Some(_) => {}
        }
      }
      if !present {
        continue;
      }
      if bare || scopes.is_empty() {
        flags.push(format!("--allow-{kind}"));
      } else {
        flags.push(format!("--allow-{kind}={}", scopes.join(",")));
      }
    }
    flags
  }
}

impl fmt::Display for ExtensionPermission {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self.scope() {
      Some(scope) => write!(f, "{}:{scope}", self.kind()),
      None => f.write_str(self.kind()),
    }
  }
}

impl FromStr for ExtensionPermission {
  type Err = ExtensionPermissionParseError;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    let reject = |reason: &str| ExtensionPermissionParseError {
      descriptor: s.to_string(),
      reason: reason.to_string(),
    };
    let (kind, scope) = match s.split_once(':') {
      Some((_, "")) => return Err(reject("scope is empty")),
      Some((_, scope)) if scope.contains(',') => return Err(reject("scope contains a comma")),
      Some((kind, scope)) => (kind, Some(scope.to_string())),
      None => (s, None),
    };
    match (kind, scope) {
      ("all", None) => Ok(Self::All),
      ("all", Some(_)) => Err(reject("`all` takes no scope")),
      ("net", scope) => Ok(Self::Net(scope)),
      ("read", scope) => Ok(Self::Read(scope)),
      ("write", scope) => Ok(Self::Write(scope)),
      ("run", scope) => Ok(Self::Run(scope)),
      ("env", scope) => Ok(Self::Env(scope)),
      ("sys", scope) => Ok(Self::Sys(scope)),
      ("ffi", scope) => Ok(Self::Ffi(scope)),
      _ => Err(reject("unknown permission kind")),
    }
  }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct ExtensionManifest {
  pub entry: String,
  #[serde(default)]
  #[ts(type = "string[]")]
  pub permissions: Vec<ExtensionPermission>,
  pub api: u32,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "shared.ts")]
pub struct ExtensionInfo {
  #[ts(type = "string[]")]
  pub permissions: Vec<ExtensionPermission>,
  pub api: u32,
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(s: &str) -> ExtensionPermission {
    s.parse().unwrap_or_else(|e| panic!("{s} should parse: {e}"))
  }

  #[test]
  fn every_bare_descriptor_round_trips() {
    for descriptor in ["all", "net", "read", "write", "run", "env", "sys", "ffi"] {
      assert_eq!(parse(descriptor).to_string(), descriptor);
    }
  }

  #[test]
  fn scoped_descriptors_keep_their_scope_verbatim() {
    assert_eq!(
      parse("net:example.com"),
      ExtensionPermission::Net(Some("example.com".into()))
    );
    assert_eq!(
      parse("net:example.com:443"),
      ExtensionPermission::Net(Some("example.com:443".into())),
      "a host:port scope must not be split on its own colon"
    );
    assert_eq!(parse("read:~/Music").to_string(), "read:~/Music");
    assert_eq!(parse("env:HOME"), ExtensionPermission::Env(Some("HOME".into())));
    assert_eq!(parse("ffi:/usr/lib/libfoo.so").to_string(), "ffi:/usr/lib/libfoo.so");
  }

  #[test]
  fn malformed_descriptors_are_rejected() {
    for descriptor in [
      "",
      "network",
      "all:everything",
      "net:",
      "READ",
      "read :x",
      "net:api.example.com,exfil.example.net",
      "read:,",
    ] {
      assert!(
        descriptor.parse::<ExtensionPermission>().is_err(),
        "{descriptor:?} must not parse"
      );
    }
  }

  #[test]
  fn descriptors_are_plain_strings_on_the_wire() {
    let permissions = vec![parse("all")];
    assert_eq!(serde_json::to_string(&permissions).expect("serialize"), r#"["all"]"#);
    let back: Vec<ExtensionPermission> = serde_json::from_str(r#"["net:example.com","read"]"#).expect("deserialize");
    assert_eq!(back, vec![parse("net:example.com"), parse("read")]);
  }

  #[test]
  fn all_collapses_the_whole_argv() {
    let perms = vec![parse("all"), parse("net:example.com")];
    assert_eq!(ExtensionPermission::deno_flags(&perms), vec!["--allow-all"]);
  }

  #[test]
  fn scopes_of_one_kind_fold_into_one_flag_in_declaration_order() {
    let perms = vec![parse("read:~/Music"), parse("net:a.example"), parse("read:/tmp")];
    assert_eq!(
      ExtensionPermission::deno_flags(&perms),
      vec!["--allow-net=a.example", "--allow-read=~/Music,/tmp"],
      "flags are grouped per kind in a stable order, never repeated"
    );
  }

  #[test]
  fn a_bare_descriptor_beats_its_scoped_siblings() {
    let perms = vec![parse("run:ffmpeg"), parse("run")];
    assert_eq!(ExtensionPermission::deno_flags(&perms), vec!["--allow-run"]);
  }

  #[test]
  fn an_empty_declaration_grants_nothing() {
    assert!(ExtensionPermission::deno_flags(&[]).is_empty());
  }

  #[test]
  fn a_manifest_block_parses_with_defaulted_permissions() {
    let manifest: ExtensionManifest =
      serde_json::from_str(r#"{"entry":"extension/desktop.mjs","api":1}"#).expect("manifest parses");
    assert!(manifest.permissions.is_empty());
    assert_eq!(manifest.api, EXTENSION_API_VERSION);
  }
}

use std::{
  collections::{BTreeMap, BTreeSet},
  path::{Path, PathBuf},
  process::Command,
};

use anyhow::{Context, Result, bail};

mod dispatch;

const TS_BINDINGS_DIR: &str = "crates/lib/ts/bindings";
const TS_COMPANION_DIR: &str = "crates/companion/ts";
const TS_COMPANION_OUTPUT: &str = "crates/companion/ts/companion.ts";
const TS_UUID_FIELDS_OUTPUT: &str = "crates/lib/ts/uuid-fields.generated.ts";
const LIB_SRC: &str = "crates/lib/src";
const RUST_CLIENT_OUTPUT: &str = "crates/client-rs/src/surface.generated.rs";
const RUST_GATEWAY_OUTPUT: &str = "crates/gateway-rs/src/surface.generated.rs";
const DOCS_OUTPUT: &str = "crates/lib/docs/surfaces.json";

fn workspace_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("../..")
    .canonicalize()
    .expect("workspace root resolves")
}

fn main() -> Result<()> {
  let args: Vec<String> = std::env::args().skip(1).collect();
  let target = args.first().map(String::as_str).unwrap_or("all");

  std::env::set_current_dir(workspace_root()).context("cd to workspace root")?;

  match target {
    "ts" | "typescript" => gen_typescript()?,
    "companion" => gen_companion_typescript()?,
    "rust" => gen_rust()?,
    "docs" => gen_docs()?,
    "all" => {
      gen_rust()?;
      gen_typescript()?;
      gen_docs()?;
    }
    other => bail!("unknown target {other:?}; expected one of: ts, companion, rust, docs, all"),
  }
  Ok(())
}

fn gen_docs() -> Result<()> {
  println!("==> docs");
  let inv = dispatch::inventory(LIB_SRC).context("dispatch inventory")?;
  let plan = dispatch::build_plan_for(&inv, dispatch::Protocol::Client).context("dispatch plan")?;
  dispatch::emit_docs_json(&plan, &inv, DOCS_OUTPUT).context("emit docs json")?;
  println!("    emitted {DOCS_OUTPUT}");
  Ok(())
}

fn gen_rust() -> Result<()> {
  println!("==> rust");
  let inv = dispatch::inventory(LIB_SRC).context("dispatch inventory")?;
  let plans = dispatch::build_plans(&inv).context("dispatch plans")?;
  for plan in &plans {
    let target = match plan.protocol {
      dispatch::Protocol::Client => dispatch::RustTarget {
        out_path: RUST_CLIENT_OUTPUT,
        sdk_type: "Client",
        wire_mod: "client",
        protocol_type: None,
      },
      dispatch::Protocol::Gateway => dispatch::RustTarget {
        out_path: RUST_GATEWAY_OUTPUT,
        sdk_type: "Gateway",
        wire_mod: "gateway",
        protocol_type: Some("GatewayProtocol"),
      },
    };
    dispatch::emit_rust(plan, &inv, &target).with_context(|| format!("emit rust dispatch for {:?}", plan.protocol))?;
    println!("    emitted {}", target.out_path);
  }
  run(
    "rustfmt",
    &["--edition", "2024", RUST_CLIENT_OUTPUT, RUST_GATEWAY_OUTPUT],
  )?;
  Ok(())
}

fn gen_typescript() -> Result<()> {
  println!("==> typescript");
  let staging = stage_dir(TS_BINDINGS_DIR)?;
  let staged = staging.display().to_string();
  let generated = (|| -> Result<()> {
    run_with(
      "cargo",
      &["test", "-p", "libbridgething", "--quiet"],
      &[("TS_RS_EXPORT_DIR", staged.as_str())],
    )?;
    prettier(&[&staged])?;
    add_ts_import_extensions(&staged).context("add .js extensions to ts bindings")
  })();
  commit_stage(generated, &staging, TS_BINDINGS_DIR)?;

  println!("    emitting ts dispatch helpers");
  let inv = dispatch::inventory(LIB_SRC).context("dispatch inventory")?;
  println!(
    "    dispatch inventory: {} marker-tagged types, {} typed-request decls, {} uuid fields",
    inv.markers.len(),
    inv.typed_requests.len(),
    inv.uuid_field_names.len(),
  );
  let plan = dispatch::build_plan_for(&inv, dispatch::Protocol::Client).context("dispatch plan")?;
  let binding_locations = scan_binding_type_locations(TS_BINDINGS_DIR).context("scan ts bindings")?;
  dispatch::emit_typescript_client(&plan, &binding_locations).context("emit client typescript dispatch")?;
  emit_ts_uuid_fields(&inv.uuid_field_names).context("emit ts uuid fields")?;
  prettier(&["packages/client-ts/src/dispatch.generated.ts", TS_UUID_FIELDS_OUTPUT])?;
  gen_companion_typescript()?;
  Ok(())
}

fn gen_companion_typescript() -> Result<()> {
  println!("==> companion typescript");
  let staging = stage_dir(TS_COMPANION_DIR)?;
  let staged = staging.display().to_string();
  let staged_output = staging.join("companion.ts").display().to_string();
  let generated = (|| -> Result<()> {
    run_with(
      "cargo",
      &["test", "-p", "bridgething-companion", "--quiet"],
      &[("TS_RS_EXPORT_DIR", staged.as_str()), ("TS_RS_LARGE_INT", "number")],
    )?;
    prettier(&[&staged_output])
  })();
  commit_stage(generated, &staging, TS_COMPANION_DIR)?;
  println!("    emitted {TS_COMPANION_OUTPUT}");
  Ok(())
}

fn stage_dir(final_dir: &str) -> Result<PathBuf> {
  let root = workspace_root();
  let target = root.join(final_dir);
  let parent = target.parent().map(Path::to_path_buf).unwrap_or(root);
  let name = target
    .file_name()
    .and_then(|name| name.to_str())
    .context("staged output needs a directory name")?;
  let staging = parent.join(format!(".{name}.staging.{}", std::process::id()));
  if staging.exists() {
    std::fs::remove_dir_all(&staging).with_context(|| format!("clear {}", staging.display()))?;
  }
  std::fs::create_dir_all(&staging).with_context(|| format!("create {}", staging.display()))?;
  Ok(staging)
}

fn commit_stage(generated: Result<()>, staging: &Path, final_dir: &str) -> Result<()> {
  if let Err(err) = generated {
    let _ = std::fs::remove_dir_all(staging);
    return Err(err);
  }
  let target = workspace_root().join(final_dir);
  if target.exists() {
    std::fs::remove_dir_all(&target).with_context(|| format!("clear {}", target.display()))?;
  }
  std::fs::rename(staging, &target).with_context(|| format!("move staged output into {}", target.display()))
}

fn add_ts_import_extensions(dir: &str) -> Result<()> {
  let specifier = regex::Regex::new(r#"(from\s+|import\s*\(\s*)(['"])(\.{1,2}/[^'"]+)(['"])"#)?;
  for entry in std::fs::read_dir(dir).with_context(|| format!("read {dir}"))? {
    let path = entry?.path();
    if path.extension().and_then(|e| e.to_str()) != Some("ts") {
      continue;
    }
    let input = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let output = specifier.replace_all(&input, |caps: &regex::Captures| {
      let spec = &caps[3];
      let suffix = if spec.ends_with(".js") { "" } else { ".js" };
      format!("{}{}{}{}{}", &caps[1], &caps[2], spec, suffix, &caps[4])
    });
    if output != input {
      std::fs::write(&path, output.as_ref()).with_context(|| format!("write {}", path.display()))?;
    }
  }
  Ok(())
}

fn scan_binding_type_locations(dir: &str) -> Result<BTreeMap<String, BTreeSet<String>>> {
  let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
  for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {dir}"))? {
    let entry = entry?;
    let path = entry.path();
    if path.extension().and_then(|s| s.to_str()) != Some("ts") {
      continue;
    }
    let file_name = path
      .file_name()
      .and_then(|s| s.to_str())
      .unwrap_or_default()
      .to_string();
    let content = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    for line in content.lines() {
      let trimmed = line.trim_start();
      let prefix = "export type ";
      if let Some(rest) = trimmed.strip_prefix(prefix) {
        let name: String = rest
          .chars()
          .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
          .collect();
        if !name.is_empty() {
          map.entry(name).or_default().insert(file_name.clone());
        }
      }
    }
  }
  Ok(map)
}

fn emit_ts_uuid_fields(names: &BTreeSet<String>) -> Result<()> {
  let mut out = String::new();
  out.push_str("// @generated by tools/codegen - do not edit. Run `just codegen` to regenerate.\n\n");
  out.push_str("export const UUID_FIELD_NAMES: ReadonlySet<string> = new Set([\n");
  for name in names {
    out.push_str(&format!("  '{name}',\n"));
  }
  out.push_str("]);\n");
  std::fs::write(TS_UUID_FIELDS_OUTPUT, out).context("write uuid-fields.generated.ts")?;
  Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<()> {
  run_with(program, args, &[])
}

fn prettier(paths: &[&str]) -> Result<()> {
  let mut args = vec!["prettier"];
  args.extend_from_slice(paths);
  args.extend_from_slice(&["--ignore-path", "/dev/null", "--write", "--log-level", "warn"]);
  run("bunx", &args)
}

fn run_with(program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<()> {
  let mut command = Command::new(program);
  command.args(args).current_dir(workspace_root());
  for (key, value) in env {
    command.env(key, value);
  }
  let status = command.status().with_context(|| format!("spawn {program}"))?;
  if !status.success() {
    bail!("{program} exited with {status}");
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn staging_is_per_invocation_and_sits_beside_its_target() {
    let staging = stage_dir(TS_COMPANION_DIR).expect("stage");
    let target = workspace_root().join(TS_COMPANION_DIR);

    assert_eq!(
      staging.parent(),
      target.parent(),
      "staging must share a filesystem with its target so the commit is a rename"
    );
    let name = staging.file_name().and_then(|n| n.to_str()).expect("staging name");
    assert!(
      name.ends_with(&format!(".{}", std::process::id())),
      "concurrent invocations must not share a staging path, got {name}"
    );
    assert!(staging.is_dir());

    std::fs::remove_dir_all(&staging).expect("clean up");
  }

  #[test]
  fn prettier_formats_generated_output_inside_the_staging_dir() {
    let staging = stage_dir(TS_BINDINGS_DIR).expect("stage");
    let probe = staging.join("probe.ts");
    std::fs::write(&probe, "export type Probe = { a: string,   b: number, };\n").expect("write probe");

    let ran = prettier(&[staging.display().to_string().as_str()]);
    let formatted = std::fs::read_to_string(&probe).expect("read probe");
    std::fs::remove_dir_all(&staging).expect("clean up");

    ran.expect("prettier run");
    assert_eq!(
      formatted, "export type Probe = { a: string; b: number };\n",
      "staged output must be formatted before it is committed, not left raw for a later repo-wide pass"
    );
  }
}

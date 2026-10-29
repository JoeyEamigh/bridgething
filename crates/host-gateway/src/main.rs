use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};

use bridgething_host_gateway::{
  chaos::ChaosConfig,
  init_logging, install,
  ota::{self, PushRequest, PushShape},
  presence, run_open, webapp,
};
use clap::{Parser, Subcommand};
use libbridgething::OtaKind;
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(version, about = "bridgething host gateway (dev iteration tool)", long_about = None)]
struct Cli {
  #[arg(long, default_value = "ws://127.0.0.1:8892/")]
  url: String,

  #[arg(long, default_value_t = 0.0)]
  inject_loss: f32,

  #[arg(long)]
  inject_disconnect: Option<u64>,

  #[arg(long)]
  throttle: Option<u64>,

  #[arg(long)]
  fixture: Option<PathBuf>,

  #[command(subcommand)]
  cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
  Open,
  PushUpdate {
    swu: PathBuf,
    #[arg(long)]
    update_url_base: Option<String>,
    #[arg(long)]
    zck: Option<PathBuf>,
    #[arg(long)]
    boot_zck: Option<PathBuf>,
  },
  PushDaemon {
    binary: PathBuf,
    #[arg(long)]
    patch_from: Option<PathBuf>,
    #[arg(long)]
    compress: bool,
  },
  PushBuiltinWebapp {
    bundle: PathBuf,
  },
  PushWakeword {
    model: PathBuf,
    #[arg(long)]
    version: Option<String>,
  },
  SwitchWebapp {
    id: Uuid,
  },
  /// Report a host display state (active, dim, off). `--hold` keeps heartbeating for that many seconds.
  Presence {
    state: String,
    #[arg(long, default_value_t = 0)]
    hold: u64,
  },
  Install {
    bundle: PathBuf,
    #[arg(long)]
    provenance: Option<String>,
  },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  init_logging();
  let cli = Cli::parse();

  let chaos = ChaosConfig {
    inject_loss: cli.inject_loss,
    inject_disconnect: cli.inject_disconnect.map(std::time::Duration::from_secs),
    throttle_bytes_per_sec: cli.throttle,
  };
  let fixture = cli.fixture.as_deref();

  match cli.cmd {
    Command::Open => run_open(&cli.url, chaos).await,
    Command::PushUpdate {
      swu,
      update_url_base,
      zck,
      boot_zck,
    } => {
      let mut zcks = BTreeMap::new();
      if let Some(path) = zck {
        zcks.insert("system.img.zck".to_owned(), resolve(fixture, &path));
      }
      if let Some(path) = boot_zck {
        zcks.insert("boot.vfat.zck".to_owned(), resolve(fixture, &path));
      }
      ota::run_push(
        &cli.url,
        chaos,
        PushRequest {
          kind: OtaKind::Image,
          artifact: resolve(fixture, &swu),
          shape: PushShape::Whole,
          update_url_base,
          zcks,
          version: None,
        },
      )
      .await
    }
    Command::PushDaemon {
      binary,
      patch_from,
      compress,
    } => {
      let shape = match (patch_from, compress) {
        (Some(source), _) => PushShape::PatchFrom(resolve(fixture, &source)),
        (None, true) => PushShape::Compressed,
        (None, false) => PushShape::Whole,
      };
      ota::run_push(
        &cli.url,
        chaos,
        PushRequest {
          kind: OtaKind::Daemon,
          artifact: resolve(fixture, &binary),
          shape,
          update_url_base: None,
          zcks: BTreeMap::new(),
          version: None,
        },
      )
      .await
    }
    Command::PushBuiltinWebapp { bundle } => {
      ota::run_push(
        &cli.url,
        chaos,
        PushRequest {
          kind: OtaKind::BuiltinWebapp,
          artifact: resolve(fixture, &bundle),
          shape: PushShape::Whole,
          update_url_base: None,
          zcks: BTreeMap::new(),
          version: None,
        },
      )
      .await
    }
    Command::PushWakeword { model, version } => {
      ota::run_push(
        &cli.url,
        chaos,
        PushRequest {
          kind: OtaKind::WakewordModel,
          artifact: resolve(fixture, &model),
          shape: PushShape::Whole,
          update_url_base: None,
          zcks: BTreeMap::new(),
          version,
        },
      )
      .await
    }
    Command::SwitchWebapp { id } => webapp::run_switch(&cli.url, chaos, id).await,
    Command::Presence { state, hold } => presence::run_presence(&cli.url, chaos, &state, hold).await,
    Command::Install { bundle, provenance } => {
      install::run_install(&cli.url, chaos, &resolve(fixture, &bundle), provenance.as_deref()).await
    }
  }
}

fn resolve(fixture: Option<&Path>, path: &Path) -> PathBuf {
  match fixture {
    Some(dir) if path.is_relative() => dir.join(path),
    _ => path.to_path_buf(),
  }
}

use std::{error::Error, path::PathBuf};

use bridgething_companion::{
  api::VoiceModelPaths,
  voice::{
    fast_path,
    inference::{BundleInference, NluInference},
    rejection::{RejectionOutcome, evaluate},
  },
};
use bridgething_desktop::{backends::Platform, shell::DesktopPaths};
use libbridgething::NluSlots;

type Boxed = Box<dyn Error>;

const USAGE: &str = "\
usage: nlu [--bundle <dir>] <utterance>...

runs each utterance through both lanes of the voice pipeline for debugging";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Boxed> {
  let (bundle, utterances) = parse(std::env::args().skip(1).collect())?;
  let bundle = match bundle {
    Some(dir) => dir,
    None => installed_bundle()?,
  };

  let platform = Platform::detect();
  let runner = platform.nlu.ok_or("this host has no nlu model runner")?;
  let armed = bundle.clone();
  platform.models.answered_by(move || VoiceModelPaths {
    nlu_bundle_dir: Some(armed.display().to_string()),
    asr_weights: None,
  });

  let inference = BundleInference::load(&bundle, runner)?;
  let policy = inference.rejection().unwrap_or_default();
  println!("bundle  {}", bundle.display());
  println!(
    "policy  in_domain>={} clarify_margin={}",
    policy.in_domain_threshold, policy.clarify_margin
  );

  for text in &utterances {
    println!("\n{text:?}");
    match fast_path::match_transcript(text) {
      Some(hit) => println!("  fast   {} {}", hit.intent, slots_json(&hit.slots)?),
      None => println!("  fast   (no hit)"),
    }
    let output = inference.infer(text).await?;
    let outcome = match evaluate(&output, policy)? {
      RejectionOutcome::Accept { intent } => intent.to_string(),
      RejectionOutcome::NoIntent => "NO_INTENT".to_string(),
      RejectionOutcome::Clarify { alternates } => format!("CLARIFY {alternates:?}"),
    };
    println!(
      "  model  {outcome} {} in_domain={:.3}",
      slots_json(&output.slots)?,
      1.0 / (1.0 + (-output.in_domain_logit).exp())
    );
  }
  Ok(())
}

fn parse(args: Vec<String>) -> Result<(Option<PathBuf>, Vec<String>), Boxed> {
  let mut bundle = None;
  let mut utterances = Vec::new();
  let mut rest = args.into_iter();
  while let Some(arg) = rest.next() {
    match arg.as_str() {
      "--bundle" => bundle = Some(PathBuf::from(rest.next().ok_or(USAGE)?)),
      "-h" | "--help" => return Err(USAGE.into()),
      _ => utterances.push(arg),
    }
  }
  if utterances.is_empty() {
    return Err(USAGE.into());
  }
  Ok((bundle, utterances))
}

fn installed_bundle() -> Result<PathBuf, Boxed> {
  let root = DesktopPaths::xdg()?.state_dir.join("bridgething-nlu");
  let current = std::fs::read_to_string(root.join("current"))
    .map_err(|_| "no installed nlu bundle; open the desktop app to download one, or pass --bundle")?;
  Ok(root.join(current.trim()))
}

fn slots_json(slots: &NluSlots) -> Result<String, Boxed> {
  Ok(serde_json::to_string(slots)?)
}

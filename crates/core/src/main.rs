use std::time::Duration;

use bridgething::DaemonConfig;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

const USAGE: &str = "usage: bridgething [--dev]

  --dev   host development mode: no bluetooth radio, loopback binds
  -h      print this message
";

fn main() {
  let mut config = DaemonConfig::real();

  for arg in std::env::args().skip(1) {
    match arg.as_str() {
      "--dev" => config = DaemonConfig::dev(),
      "-h" | "--help" => return print!("{USAGE}"),
      other => {
        eprint!("bridgething: unrecognized argument {other:?}\n\n{USAGE}");
        std::process::exit(2);
      }
    }
  }

  let runtime = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("the tokio runtime builds");
  runtime.block_on(bridgething::run_daemon(config));
  runtime.shutdown_timeout(SHUTDOWN_GRACE);
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
  bridgething_io::install_crypto_provider();
  bridgething_desktop::run();
}

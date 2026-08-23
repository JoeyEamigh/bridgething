fn main() {
  let _ = rustls::crypto::ring::default_provider().install_default();
  bridgething_desktop::run();
}

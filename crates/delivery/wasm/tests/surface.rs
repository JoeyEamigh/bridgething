#![cfg(target_arch = "wasm32")]

use bridgething_delivery_wasm::{WasmByteLink, artifact_urls, parse_composite_version};
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use wasm_bindgen_test::wasm_bindgen_test;

fn field(value: &JsValue, key: &str) -> Option<String> {
  js_sys::Reflect::get(value, &JsValue::from_str(key))
    .expect("the object answers the key")
    .as_string()
}

#[wasm_bindgen_test]
fn a_composite_version_splits_into_its_two_halves() {
  let parsed = parse_composite_version("0.8.0+image.2026.04".to_owned()).expect("a composite parses");
  assert_eq!(field(&parsed, "daemon").as_deref(), Some("0.8.0"));
  assert_eq!(field(&parsed, "image").as_deref(), Some("2026.04"));
}

#[wasm_bindgen_test]
fn a_version_without_an_image_half_is_null_rather_than_an_error() {
  let parsed = parse_composite_version("0.8.0".to_owned()).expect("a bad composite still answers");
  assert!(parsed.is_null());
}

#[wasm_bindgen_test]
fn artifact_urls_reach_javascript_camel_cased() {
  let urls = artifact_urls(
    "https://ota.test/".to_owned(),
    "dev".to_owned(),
    "0.8.0".to_owned(),
    "2026.04".to_owned(),
    "dev".to_owned(),
  )
  .expect("the urls build");

  assert_eq!(
    field(&urls, "imageSwu").as_deref(),
    Some("https://ota.test/images/dev/2026.04/bridgething-dev-image.swu"),
  );
  assert_eq!(
    field(&urls, "daemonBinary").as_deref(),
    Some("https://ota.test/daemon/dev/0.8.0/bridgething"),
  );
}

#[wasm_bindgen_test]
fn a_byte_link_takes_bytes_before_and_after_it_closes() {
  let writes = js_sys::Array::new();
  let sink = writes.clone();
  let write = Closure::<dyn FnMut(js_sys::Uint8Array)>::new(move |chunk: js_sys::Uint8Array| {
    sink.push(&chunk);
  });
  let link = WasmByteLink::new(write.as_ref().unchecked_ref::<js_sys::Function>().clone());

  link.push(&[1, 2, 3]);
  link.close();
  link.push(&[4, 5, 6]);
}

#[wasm_bindgen_test]
fn the_system_clock_reads_wall_time_in_the_browser() {
  use bridgething_delivery::seam::{Clock, SystemClock};

  assert!(
    SystemClock.unix_millis() > 1_767_225_600_000,
    "a 2026 or later wall clock"
  );
}

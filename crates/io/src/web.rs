use std::sync::Arc;

use js_sys::Uint8Array;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{AbortController, Headers, ReadableStreamDefaultReader, Request, RequestInit, Response};

use crate::http::{HttpDownloadSink, HttpHeader, HttpRequest, HttpResponse, HttpSink, HttpTransport};

#[derive(Debug, Default, Clone, Copy)]
pub struct FetchTransport;

impl FetchTransport {
  pub fn new() -> Self {
    Self
  }
}

impl HttpTransport for FetchTransport {
  fn execute(&self, request: HttpRequest, sink: Arc<HttpSink>) {
    spawn_local(async move {
      match whole(request).await {
        Ok(response) => sink.complete(response),
        Err(reason) => sink.fail(reason),
      }
    });
  }

  fn download(&self, request: HttpRequest, sink: Arc<HttpDownloadSink>) {
    spawn_local(async move {
      if let Err(reason) = streamed(request, &sink).await {
        sink.on_failed(reason);
        return;
      }
      sink.on_finished();
    });
  }
}

async fn whole(request: HttpRequest) -> Result<HttpResponse, String> {
  let (response, _deadline) = send(request).await?;
  let buffer = JsFuture::from(response.array_buffer().map_err(|e| js_reason("body", &e))?)
    .await
    .map_err(|e| js_reason("body", &e))?;
  Ok(HttpResponse {
    status: response.status(),
    headers: headers_of(&response),
    body: Uint8Array::new(&buffer).to_vec(),
  })
}

async fn streamed(request: HttpRequest, sink: &Arc<HttpDownloadSink>) -> Result<(), String> {
  let (response, _deadline) = send(request).await?;
  let headers = headers_of(&response);
  let content_length = headers
    .iter()
    .find(|header| header.name.eq_ignore_ascii_case("content-length"))
    .and_then(|header| header.value.parse::<u64>().ok());
  sink.on_response(response.status(), headers, content_length);

  let Some(body) = response.body() else {
    return Ok(());
  };
  let reader: ReadableStreamDefaultReader = body.get_reader().dyn_into().map_err(|e| js_reason("body reader", &e))?;

  loop {
    let chunk = JsFuture::from(reader.read())
      .await
      .map_err(|e| js_reason("body chunk", &e))?;
    let done = js_sys::Reflect::get(&chunk, &JsValue::from_str("done"))
      .ok()
      .and_then(|value| value.as_bool())
      .unwrap_or(true);
    if done {
      return Ok(());
    }
    let value = js_sys::Reflect::get(&chunk, &JsValue::from_str("value")).map_err(|e| js_reason("body chunk", &e))?;
    let bytes = value
      .dyn_into::<Uint8Array>()
      .map_err(|e| js_reason("body chunk", &e))?;
    sink.on_chunk(bytes.to_vec());
  }
}

struct Deadline {
  controller: AbortController,
  global: JsValue,
  clear: js_sys::Function,
  handle: JsValue,
  _on_elapsed: Closure<dyn FnMut()>,
}

impl Drop for Deadline {
  fn drop(&mut self) {
    let _ = self.clear.call1(&self.global, &self.handle);
  }
}

fn arm_deadline(global: &JsValue, timeout_ms: u32) -> Result<Option<Deadline>, String> {
  if timeout_ms == 0 {
    return Ok(None);
  }
  let controller = AbortController::new().map_err(|e| js_reason("abort controller", &e))?;
  let abort_on_elapsed = controller.clone();
  let on_elapsed = Closure::<dyn FnMut()>::new(move || abort_on_elapsed.abort());
  let set = host_function(global, "setTimeout")?;
  let clear = host_function(global, "clearTimeout")?;
  let handle = set
    .call2(
      global,
      on_elapsed.as_ref().unchecked_ref(),
      &JsValue::from_f64(f64::from(timeout_ms)),
    )
    .map_err(|e| js_reason("setTimeout", &e))?;
  Ok(Some(Deadline {
    controller,
    global: global.clone(),
    clear,
    handle,
    _on_elapsed: on_elapsed,
  }))
}

fn host_function(global: &JsValue, name: &str) -> Result<js_sys::Function, String> {
  js_sys::Reflect::get(global, &JsValue::from_str(name))
    .ok()
    .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
    .ok_or_else(|| format!("this host has no {name}"))
}

async fn send(request: HttpRequest) -> Result<(Response, Option<Deadline>), String> {
  request.method.validate()?;
  let init = RequestInit::new();
  init.set_method(request.method.as_str());
  if !request.body.is_empty() {
    init.set_body(&Uint8Array::from(request.body.as_slice()).into());
  }

  let headers = Headers::new().map_err(|e| js_reason("headers", &e))?;
  for header in &request.headers {
    headers
      .set(&header.name, &header.value)
      .map_err(|e| js_reason("headers", &e))?;
  }
  init.set_headers(&headers);

  let global = js_sys::global();
  let deadline = arm_deadline(&global, request.timeout_ms)?;
  if let Some(deadline) = &deadline {
    init.set_signal(Some(&deadline.controller.signal()));
  }

  let built = Request::new_with_str_and_init(&request.url, &init).map_err(|e| js_reason("request", &e))?;
  let fetch = host_function(&global, "fetch")?;
  let pending = fetch
    .call1(&global, &built)
    .map_err(|e| js_reason("fetch", &e))?
    .dyn_into::<js_sys::Promise>()
    .map_err(|e| js_reason("fetch", &e))?;

  let response = JsFuture::from(pending)
    .await
    .map_err(|e| js_reason("fetch", &e))?
    .dyn_into::<Response>()
    .map_err(|e| js_reason("fetch", &e))?;
  Ok((response, deadline))
}

fn headers_of(response: &Response) -> Vec<HttpHeader> {
  let mut out = Vec::new();
  let entries = js_sys::try_iter(&response.headers()).ok().flatten();
  let Some(entries) = entries else {
    return out;
  };
  for entry in entries.flatten() {
    let pair = js_sys::Array::from(&entry);
    if pair.length() < 2 {
      continue;
    }
    if let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
      out.push(HttpHeader { name, value });
    }
  }
  out
}

fn js_reason(context: &str, value: &JsValue) -> String {
  format!("{context}: {value:?}")
}

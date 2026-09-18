use std::sync::OnceLock;

use reqwest::Url;
use reqwest_middleware::ClientWithMiddleware;
use tauri::{
  http::{Request, Response, StatusCode},
  plugin::{Builder, TauriPlugin},
  Manager, Runtime, UriSchemeContext, UriSchemeResponder,
};

mod cache;
mod error;
pub use cache::{CacheConfig, CacheMode};
pub use error::{Error, Result};

pub const WEBPROXY_SCHEME: &str = "webproxy";

static CACHE_CONFIG: OnceLock<CacheConfig> = OnceLock::new();
static CLIENT: OnceLock<ClientWithMiddleware> = OnceLock::new();

/// Configures the cache before the webproxy client is initialized.
///
/// Returns `Ok(())` if configuration was set, or `Err(config)` if the client
/// has already been initialized.
pub fn configure_cache(config: CacheConfig) -> std::result::Result<(), CacheConfig> {
  CACHE_CONFIG.set(config)
}

fn client() -> &'static ClientWithMiddleware {
  CLIENT.get_or_init(|| {
    let config = CACHE_CONFIG.get().cloned().unwrap_or_default();
    cache::build_cached_client(&config)
  })
}

const HOP_BY_HOP_HEADERS: &[&str] = &[
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
];

const RESPONSE_POLICY_HEADERS: &[&str] = &[
  "content-security-policy",
  "content-security-policy-report-only",
  "x-content-security-policy",
  "x-webkit-csp",
  "x-frame-options",
  "cross-origin-embedder-policy",
  "cross-origin-opener-policy",
  "cross-origin-resource-policy",
  "origin-agent-cluster",
  "clear-site-data",
];

/// Registers the webproxy custom protocol and navigation filter on a plugin builder
/// with default cache configuration.
pub fn register<R: Runtime>(builder: Builder<R>) -> Builder<R> {
  register_with_config(builder, CacheConfig::default())
}

/// Registers the webproxy custom protocol and navigation filter on a plugin builder
/// with custom cache configuration.
pub fn register_with_config<R: Runtime>(
  builder: Builder<R>,
  config: CacheConfig,
) -> Builder<R> {
  builder
    .setup(move |app, _api| {
      let mut cfg = config.clone();
      if cfg.cache_dir.is_none() {
        if let Ok(app_cache) = app.path().app_cache_dir() {
          cfg.cache_dir = Some(app_cache.join("tauri-plugin-webproxy-cache"));
        }
      }
      let _ = CACHE_CONFIG.set(cfg);
      Ok(())
    })
    .register_asynchronous_uri_scheme_protocol(WEBPROXY_SCHEME, scheme_handler())
    .on_navigation(|_webview, url| {
      let scheme = url.scheme();
      matches!(
        scheme,
        "http" | "https" | "webproxy" | "tauri" | "asset" | "about" | "blob" | "data"
      )
    })
}

/// Initializes the webproxy plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
  register(Builder::new("webproxy")).build()
}

/// Initializes the webproxy plugin with custom cache configuration.
pub fn init_with_config<R: Runtime>(config: CacheConfig) -> TauriPlugin<R> {
  register_with_config(Builder::new("webproxy"), config).build()
}

pub fn scheme_handler<R: Runtime>(
) -> impl Fn(UriSchemeContext<'_, R>, Request<Vec<u8>>, UriSchemeResponder) + Send + Sync + 'static
{
  move |_ctx, request, responder| {
    tauri::async_runtime::spawn(async move {
      responder.respond(handle(request).await);
    });
  }
}

async fn handle(request: Request<Vec<u8>>) -> Response<Vec<u8>> {
  if request.method().as_str() == "OPTIONS"
    && request
      .headers()
      .contains_key("access-control-request-method")
  {
    return preflight_response(&request);
  }

  match extract_target(&request) {
    Ok(target) => forward(target, request).await,
    Err(message) => error_response(StatusCode::BAD_REQUEST, &message),
  }
}

/// Converts the logical custom-protocol request back to the real HTTPS target.
///
/// Wry normally reverts its Android/Windows workaround before invoking this
/// handler, so the URI is `webproxy://example.com/path`. The http(s) branch is
/// kept as a defensive fallback for runtimes that expose the workaround URI.
pub fn extract_target(request: &Request<Vec<u8>>) -> std::result::Result<Url, String> {
  let uri = request.uri();
  let scheme = uri
    .scheme_str()
    .ok_or_else(|| "webproxy request has no URI scheme".to_string())?;

  let authority = match scheme {
    WEBPROXY_SCHEME => uri
      .authority()
      .map(|value| value.as_str().to_string())
      .ok_or_else(|| "webproxy request has no target host".to_string())?,
    "http" | "https" => {
      let host = uri
        .host()
        .and_then(|host| host.strip_prefix("webproxy."))
        .ok_or_else(|| "invalid webproxy workaround host".to_string())?;
      match uri.port_u16() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
      }
    }
    other => return Err(format!("unexpected webproxy URI scheme {other:?}")),
  };

  let path_and_query = uri
    .path_and_query()
    .map(|value| value.as_str())
    .unwrap_or("/");

  Url::parse(&format!("https://{authority}{path_and_query}"))
    .map_err(|error| format!("invalid webproxy target: {error}"))
}

async fn forward(target: Url, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
  let (parts, body) = request.into_parts();
  let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
    Ok(method) => method,
    Err(error) => {
      return error_response(
        StatusCode::BAD_REQUEST,
        &format!("invalid webproxy method: {error}"),
      )
    }
  };

  let upstream_origin = target.origin().ascii_serialization();
  let mut upstream = client().request(method, target.clone());

  let mut has_user_agent = false;

  for (name, value) in &parts.headers {
    let lower = name.as_str().to_ascii_lowercase();

    if lower == "origin" {
      // The WebView sees a custom proxy origin; upstream should see the
      // corresponding HTTPS origin instead.
      upstream = upstream.header("origin", &upstream_origin);
      continue;
    }

    if lower == "user-agent" {
      has_user_agent = true;
    }

    if should_strip_request_header(&lower) {
      continue;
    }

    upstream = upstream.header(name.as_str(), value.as_bytes());
  }

  if !has_user_agent {
    upstream = upstream.header(
      "user-agent",
      "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    );
  }

  // The native client transparently negotiates and decodes gzip from upstream,
  // reducing network transfer while presenting decoded plain bytes for HTML rewriting.
  let upstream = match upstream.body(body).send().await {
    Ok(response) => response,
    Err(error) => {
      return error_response(
        StatusCode::BAD_GATEWAY,
        &format!("webproxy upstream request failed: {error}"),
      )
    }
  };

  let status = upstream.status().as_u16();
  let headers = upstream.headers().clone();

  let bytes = match upstream.bytes().await {
    Ok(bytes) => bytes,
    Err(error) => {
      return error_response(
        StatusCode::BAD_GATEWAY,
        &format!("webproxy failed to read upstream response: {error}"),
      )
    }
  };

  let is_html = headers
    .get("content-type")
    .and_then(|value| value.to_str().ok())
    .map(|value| value.to_ascii_lowercase().starts_with("text/html"))
    .unwrap_or(false);
  let identity_encoded = headers
    .get("content-encoding")
    .and_then(|value| value.to_str().ok())
    .map(|value| value.eq_ignore_ascii_case("identity"))
    .unwrap_or(true);

  // Only rewrite complete HTML documents. Never modify range responses.
  let (body, body_changed) = if status == 200 && is_html && identity_encoded {
    rewrite_html(&bytes)
  } else {
    (bytes.to_vec(), false)
  };

  let mut response = Response::builder().status(status);

  for (name, value) in &headers {
    let lower = name.as_str().to_ascii_lowercase();

    if should_strip_response_header(&lower, body_changed) {
      continue;
    }

    response = response.header(name.as_str(), value.as_bytes());
  }

  // Allow the host app to use fetch/XHR against webproxy URLs as well as use
  // them for iframe navigation.
  response = response
    .header("access-control-allow-origin", "*")
    .header("access-control-expose-headers", "*");

  response.body(body).unwrap()
}

fn should_strip_request_header(name: &str) -> bool {
  HOP_BY_HOP_HEADERS.contains(&name)
    || matches!(
      name,
      "host"
        | "content-length"
        | "accept-encoding"
        | "referer"
        | "cookie"
        | "x-frame-options"
    )
    || name.starts_with("sec-fetch-")
    || name.starts_with("access-control-request-")
}

fn should_strip_response_header(name: &str, body_changed: bool) -> bool {
  HOP_BY_HOP_HEADERS.contains(&name)
    || RESPONSE_POLICY_HEADERS.contains(&name)
    || name.starts_with("access-control-")
    || matches!(name, "content-length" | "set-cookie" | "content-encoding")
    || (body_changed && matches!(name, "etag" | "content-md5" | "digest"))
}

fn preflight_response(request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
  let requested_headers = request
    .headers()
    .get("access-control-request-headers")
    .and_then(|value| value.to_str().ok())
    .unwrap_or("*");

  Response::builder()
    .status(StatusCode::NO_CONTENT)
    .header("access-control-allow-origin", "*")
    .header(
      "access-control-allow-methods",
      "GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS",
    )
    .header("access-control-allow-headers", requested_headers)
    .header("access-control-max-age", "86400")
    .header(
      "vary",
      "Origin, Access-Control-Request-Method, Access-Control-Request-Headers",
    )
    .body(Vec::new())
    .unwrap()
}

fn escape_html(input: &str) -> String {
  input
    .replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('\"', "&quot;")
}

fn error_response(status: StatusCode, message: &str) -> Response<Vec<u8>> {
  let status_code = status.as_u16();
  let reason = status.canonical_reason().unwrap_or("Error");
  let escaped_message = escape_html(message);

  let html = format!(
    r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{status_code} {reason}</title>
  <style>
    body {{
      font-family: system-ui, -apple-system, sans-serif;
      margin: 0;
      padding: max(40px, 12vh) 24px 24px;
      display: flex;
      justify-content: center;
      align-items: flex-start;
      min-height: 100vh;
      box-sizing: border-box;
      background: #f8fafc;
      color: #1e293b;
    }}
    @media (prefers-color-scheme: dark) {{
      body {{ background: #0f172a; color: #f8fafc; }}
      .card {{ background: #1e293b !important; border-color: #334155 !important; }}
      pre {{ background: #0f172a !important; color: #94a3b8 !important; }}
      button {{ background: #334155 !important; color: #f8fafc !important; }}
    }}
    .card {{
      max-width: 480px;
      width: 100%;
      background: #fff;
      border: 1px solid #e2e8f0;
      border-radius: 8px;
      padding: 24px;
      box-shadow: 0 1px 3px rgba(0, 0, 0, 0.05);
    }}
    h1 {{ font-size: 1.25rem; margin: 0 0 12px; }}
    pre {{
      background: #f1f5f9;
      padding: 12px;
      border-radius: 6px;
      font-size: 0.8125rem;
      white-space: pre-wrap;
      word-break: break-all;
      margin: 0 0 16px;
    }}
    button {{
      padding: 8px 16px;
      border: 0;
      border-radius: 6px;
      background: #e2e8f0;
      color: #1e293b;
      cursor: pointer;
      font-size: 0.875rem;
    }}
  </style>
</head>
<body>
  <div class="card">
    <h1>{status_code} {reason}</h1>
    <pre>{escaped_message}</pre>
    <button onclick="location.reload()">Reload</button>
  </div>
</body>
</html>"#
  );

  Response::builder()
    .status(status)
    .header("content-type", "text/html; charset=utf-8")
    .header("access-control-allow-origin", "*")
    .body(html.into_bytes())
    .unwrap()
}

const STATE_BRIDGE_SCRIPT: &str = include_str!("webproxy_bridge.js");
const STATE_BRIDGE_SCRIPT_OPEN: &[u8] = b"<script data-webproxy-state-bridge>";
const STATE_BRIDGE_SCRIPT_CLOSE: &[u8] = b"</script>";

fn rewrite_html(input: &[u8]) -> (Vec<u8>, bool) {
  let (html, policies_changed) = strip_meta_frame_policies(input);
  let (html, bridge_injected) = inject_state_bridge(&html);
  (html, policies_changed || bridge_injected)
}

fn inject_state_bridge(input: &[u8]) -> (Vec<u8>, bool) {
  // Match the actual tag we inject rather than any arbitrary occurrence of the
  // marker attribute name in page text or JavaScript.
  if contains_ascii_case_insensitive(input, STATE_BRIDGE_SCRIPT_OPEN) {
    return (input.to_vec(), false);
  }

  let insert_at = find_start_tag(input, b"<head")
    .and_then(|start| find_tag_end(input, start))
    .map(|end| end + 1)
    // If the document has no explicit <head>, insert immediately after <html>.
    // The HTML parser will place the script into the implicit head, keeping the
    // bridge early enough to intercept page initialization behavior.
    .or_else(|| {
      find_start_tag(input, b"<html")
        .and_then(|start| find_tag_end(input, start))
        .map(|end| end + 1)
    })
    // Extremely small or malformed HTML: executing first is preferable to
    // appending after </html>, because the bridge should be installed ASAP.
    .unwrap_or(0);

  let script = STATE_BRIDGE_SCRIPT.as_bytes();
  let extra_len = STATE_BRIDGE_SCRIPT_OPEN.len() + script.len() + STATE_BRIDGE_SCRIPT_CLOSE.len();
  let mut output = Vec::with_capacity(input.len() + extra_len);

  output.extend_from_slice(&input[..insert_at]);
  output.extend_from_slice(STATE_BRIDGE_SCRIPT_OPEN);
  output.extend_from_slice(script);
  output.extend_from_slice(STATE_BRIDGE_SCRIPT_CLOSE);
  output.extend_from_slice(&input[insert_at..]);

  (output, true)
}

fn find_start_tag(input: &[u8], needle: &[u8]) -> Option<usize> {
  let mut from = 0usize;

  while from + needle.len() <= input.len() {
    let relative = input[from..]
      .windows(needle.len())
      .position(|window| ascii_eq_ignore_case(window, needle))?;
    let start = from + relative;
    let next = input.get(start + needle.len()).copied();

    if next.map_or(true, |byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>')) {
      return Some(start);
    }

    from = start + needle.len();
  }

  None
}

/// Removes CSP/X-Frame-Options policies declared through `<meta http-equiv>`.
/// The scan operates on raw bytes so it does not need to guess the page's text
/// encoding; HTML tag and attribute names are ASCII by definition.
fn strip_meta_frame_policies(input: &[u8]) -> (Vec<u8>, bool) {
  let mut output = Vec::with_capacity(input.len());
  let mut copied_until = 0usize;
  let mut scan_from = 0usize;
  let mut changed = false;

  while let Some(start) = find_meta_tag(input, scan_from) {
    let Some(end) = find_tag_end(input, start) else {
      break;
    };
    let tag = &input[start..=end];

    let is_http_equiv = contains_ascii_case_insensitive(tag, b"http-equiv");
    let is_blocking_policy = contains_ascii_case_insensitive(tag, b"content-security-policy")
      || contains_ascii_case_insensitive(tag, b"x-frame-options");

    if is_http_equiv && is_blocking_policy {
      output.extend_from_slice(&input[copied_until..start]);
      copied_until = end + 1;
      changed = true;
    }

    scan_from = end + 1;
  }

  if !changed {
    return (input.to_vec(), false);
  }

  output.extend_from_slice(&input[copied_until..]);
  (output, true)
}

fn find_meta_tag(input: &[u8], mut from: usize) -> Option<usize> {
  const NEEDLE: &[u8] = b"<meta";
  while from + NEEDLE.len() <= input.len() {
    let relative = input[from..]
      .windows(NEEDLE.len())
      .position(|window| ascii_eq_ignore_case(window, NEEDLE))?;
    let start = from + relative;
    let next = input.get(start + NEEDLE.len()).copied();
    if next.map_or(true, |byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>')) {
      return Some(start);
    }
    from = start + NEEDLE.len();
  }
  None
}

fn find_tag_end(input: &[u8], start: usize) -> Option<usize> {
  let mut quote = None;
  for (offset, byte) in input[start..].iter().copied().enumerate() {
    match quote {
      Some(current) if byte == current => quote = None,
      Some(_) => {}
      None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
      None if byte == b'>' => return Some(start + offset),
      None => {}
    }
  }
  None
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
  haystack
    .windows(needle.len())
    .any(|window| ascii_eq_ignore_case(window, needle))
}

fn ascii_eq_ignore_case(left: &[u8], right: &[u8]) -> bool {
  left.len() == right.len()
    && left
      .iter()
      .zip(right)
      .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn request(uri: &str) -> Request<Vec<u8>> {
    Request::builder().uri(uri).body(Vec::new()).unwrap()
  }

  #[test]
  fn keeps_target_host_in_custom_scheme() {
    let target = extract_target(&request(
      "webproxy://doc.libsodium.org/key_derivation/hkdf?q=1",
    ))
    .unwrap();
    assert_eq!(
      target.as_str(),
      "https://doc.libsodium.org/key_derivation/hkdf?q=1"
    );
  }

  #[test]
  fn accepts_wry_http_workaround_uri() {
    let target = extract_target(&request(
      "http://webproxy.doc.libsodium.org/key_derivation/hkdf",
    ))
    .unwrap();
    assert_eq!(
      target.as_str(),
      "https://doc.libsodium.org/key_derivation/hkdf"
    );
  }

  #[test]
  fn removes_meta_csp_without_touching_other_meta_tags() {
    let html = br#"<!doctype html><head>
      <meta charset="utf-8">
      <META content="default-src 'none' > x" HTTP-EQUIV='Content-Security-Policy'>
      <meta name="description" content="content-security-policy">
    </head><body>ok</body>"#;
    let (html, changed) = strip_meta_frame_policies(html);
    let html = String::from_utf8(html).unwrap();
    assert!(changed);
    assert!(!html.contains("HTTP-EQUIV='Content-Security-Policy'"));
    assert!(html.contains("<meta charset=\"utf-8\">"));
    assert!(html.contains("name=\"description\""));
  }

  #[test]
  fn injects_state_bridge_at_start_of_head() {
    let html = br#"<!doctype html><html><head><title>Hello</title></head><body>ok</body></html>"#;
    let (html, changed) = rewrite_html(html);
    let html = String::from_utf8(html).unwrap();

    assert!(changed);
    let head = html.find("<head>").unwrap();
    let bridge = html.find("data-webproxy-state-bridge").unwrap();
    let title = html.find("<title>Hello</title>").unwrap();
    assert!(head < bridge && bridge < title);
  }

  #[test]
  fn does_not_inject_state_bridge_twice() {
    let html = br#"<!doctype html><head><script data-webproxy-state-bridge></script></head>"#;
    let (html, changed) = inject_state_bridge(html);
    let html = String::from_utf8(html).unwrap();

    assert!(!changed);
    assert_eq!(html.matches("data-webproxy-state-bridge").count(), 1);
  }

  #[test]
  fn injects_state_bridge_after_html_when_head_is_missing() {
    let html = br#"<!doctype html><html><body>ok</body></html>"#;
    let (html, changed) = inject_state_bridge(html);
    let html = String::from_utf8(html).unwrap();

    assert!(changed);
    let html_tag = html.find("<html>").unwrap();
    let bridge = html.find("data-webproxy-state-bridge").unwrap();
    let body = html.find("<body>").unwrap();
    assert!(html_tag < bridge && bridge < body);
  }

  #[test]
  fn bridge_script_must_not_close_script_element() {
    assert!(
      !STATE_BRIDGE_SCRIPT.to_ascii_lowercase().contains("</script"),
      "webproxy_bridge.js must not contain a literal </script"
    );
  }

  #[test]
  fn generates_styled_error_page() {
    let resp = error_response(
      StatusCode::BAD_GATEWAY,
      "webproxy upstream request failed: connection refused <test>",
    );
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
      resp.headers().get("content-type").unwrap(),
      "text/html; charset=utf-8"
    );
    assert_eq!(resp.headers().get("access-control-allow-origin").unwrap(), "*");

    let body = String::from_utf8(resp.into_body()).unwrap();
    assert!(body.contains("502 Bad Gateway"));
    assert!(body.contains("connection refused &lt;test&gt;"));
    assert!(!body.contains("<test>"));
    assert!(body.contains("location.reload()"));
  }

  #[test]
  fn transparently_decodes_gzip_and_injects_bridge() {
    tauri::async_runtime::block_on(async {
      use std::io::{Read, Write};
      let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
      let addr = listener.local_addr().unwrap();

      let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap();
        let req_str = String::from_utf8_lossy(&buf[..n]);
        assert!(
          req_str.to_ascii_lowercase().contains("accept-encoding"),
          "request should advertise compression support"
        );

        // Gzipped "<!doctype html><html><head><title>Test</title></head><body>ok</body></html>"
        let gz_body: &[u8] = &[
          31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 179, 81, 76, 201, 79, 46, 169, 44, 72, 85, 200, 40,
          201, 205, 177, 179, 129, 146, 169, 137, 41, 118, 54, 37, 153, 37, 57, 169, 118, 33, 169,
          197, 37, 54, 250, 16, 182, 141, 62, 68, 38, 41, 63, 165, 210, 46, 63, 219, 70, 31, 204,
          0, 138, 130, 116, 1, 0, 148, 231, 186, 58, 75, 0, 0, 0,
        ];

        let response = format!(
          "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
          gz_body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(&gz_body).unwrap();
      });

      let target_url = Url::parse(&format!("http://{addr}/test.html")).unwrap();
      let req = Request::builder()
        .method("GET")
        .uri(format!("webproxy://{addr}/test.html"))
        .body(Vec::new())
        .unwrap();

      let resp = forward(target_url, req).await;
      server.join().unwrap();

      assert_eq!(resp.status(), 200);
      assert!(
        !resp.headers().contains_key("content-encoding"),
        "response to webview must not retain content-encoding"
      );

      let body_str = String::from_utf8(resp.into_body()).unwrap();
      assert!(
        body_str.contains("data-webproxy-state-bridge"),
        "bridge script should be injected into gzipped html"
      );
      assert!(body_str.contains("<title>Test</title>"));
    });
  }

  #[test]
  fn caches_responses_with_cache_control() {
    tauri::async_runtime::block_on(async {
      use std::io::{Read, Write};
      use std::sync::atomic::{AtomicUsize, Ordering};
      use std::sync::Arc;

      let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
      let addr = listener.local_addr().unwrap();

      let hit_count = Arc::new(AtomicUsize::new(0));
      let hit_count_clone = hit_count.clone();

      let server = std::thread::spawn(move || {
        // We only expect 1 request from upstream; the 2nd must hit the cache!
        for stream in listener.incoming().take(1) {
          let mut stream = stream.unwrap();
          let mut buf = [0u8; 1024];
          let _ = stream.read(&mut buf).unwrap();
          hit_count_clone.fetch_add(1, Ordering::SeqCst);

          let body = "console.log('cached asset');";
          let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nCache-Control: public, max-age=3600\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
          );
          stream.write_all(response.as_bytes()).unwrap();
        }
      });

      let target_url = Url::parse(&format!("http://{addr}/asset.js")).unwrap();
      let req1 = Request::builder()
        .method("GET")
        .uri(format!("webproxy://{addr}/asset.js"))
        .body(Vec::new())
        .unwrap();

      let resp1 = forward(target_url.clone(), req1).await;
      assert_eq!(resp1.status(), 200);
      assert_eq!(resp1.into_body(), b"console.log('cached asset');");

      // Second request: mock server has closed the listener loop,
      // so if a network request were made it would fail. It must hit cache!
      let req2 = Request::builder()
        .method("GET")
        .uri(format!("webproxy://{addr}/asset.js"))
        .body(Vec::new())
        .unwrap();

      let resp2 = forward(target_url, req2).await;
      assert_eq!(resp2.status(), 200);
      assert_eq!(resp2.into_body(), b"console.log('cached asset');");

      server.join().unwrap();
      assert_eq!(hit_count.load(Ordering::SeqCst), 1);
    });
  }
}

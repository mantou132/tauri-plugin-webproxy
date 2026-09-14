use std::path::PathBuf;
pub use http_cache_reqwest::CacheMode;
use http_cache_reqwest::{CACacheManager, Cache, HttpCache, HttpCacheOptions};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};

/// Configuration for the webproxy caching middleware.
#[derive(Debug, Clone)]
pub struct CacheConfig {
  /// Whether the HTTP cache middleware is enabled. Default is `true`.
  pub enabled: bool,
  /// The directory path where disk cache will be stored.
  /// If `None`, defaults to `<app_cache_dir>/tauri-plugin-webproxy-cache` or
  /// system temporary directory when running without an AppHandle.
  pub cache_dir: Option<PathBuf>,
  /// The caching mode to use (e.g. `CacheMode::Default`, `CacheMode::NoStore`, etc.).
  pub cache_mode: CacheMode,
}

impl Default for CacheConfig {
  fn default() -> Self {
    Self {
      enabled: true,
      cache_dir: None,
      cache_mode: CacheMode::Default,
    }
  }
}

impl CacheConfig {
  /// Creates a new default cache configuration.
  pub fn new() -> Self {
    Self::default()
  }

  /// Sets the directory where cached responses will be stored on disk.
  pub fn cache_dir(mut self, path: impl Into<PathBuf>) -> Self {
    self.cache_dir = Some(path.into());
    self
  }

  /// Enables or disables caching.
  pub fn enabled(mut self, enabled: bool) -> Self {
    self.enabled = enabled;
    self
  }

  /// Sets the cache mode.
  pub fn cache_mode(mut self, mode: CacheMode) -> Self {
    self.cache_mode = mode;
    self
  }
}

/// Builds a reqwest client wrapped with the http-cache middleware.
pub fn build_cached_client(config: &CacheConfig) -> ClientWithMiddleware {
  let reqwest_client = reqwest::Client::builder()
    // Keep upstream HTTP cookies in the native client. Cookies belonging to
    // the custom WebView origin are intentionally not forwarded upstream.
    .cookie_store(true)
    .gzip(true)
    .build()
    .expect("failed to build webproxy HTTP client");

  let mut builder = ClientBuilder::new(reqwest_client);

  if config.enabled {
    let manager = match &config.cache_dir {
      Some(dir) => CACacheManager {
        path: dir.clone(),
      },
      None => {
        let default_dir = std::env::temp_dir().join("tauri-plugin-webproxy-cache");
        CACacheManager { path: default_dir }
      }
    };

    builder = builder.with(Cache(HttpCache {
      mode: config.cache_mode,
      manager,
      options: HttpCacheOptions::default(),
    }));
  }

  builder.build()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_cache_config_builder() {
    let config = CacheConfig::new()
      .enabled(true)
      .cache_dir("/tmp/my-webproxy-cache")
      .cache_mode(CacheMode::Default);

    assert!(config.enabled);
    assert_eq!(
      config.cache_dir,
      Some(PathBuf::from("/tmp/my-webproxy-cache"))
    );
    assert!(matches!(config.cache_mode, CacheMode::Default));
  }

  #[test]
  fn test_cached_client_build() {
    let test_dir = std::env::temp_dir().join(format!(
      "tauri-webproxy-test-{}",
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
    ));
    let config = CacheConfig::new().cache_dir(test_dir.clone());
    let client = build_cached_client(&config);
    let _ = client;
    let _ = std::fs::remove_dir_all(test_dir);
  }
}

# Tauri Plugin Webproxy

Tauri 插件：提供 `webproxy://` 自定义协议，支持在 WebView 中安全代理网页并绕过 CSP / X-Frame-Options 限制，专为 iframe 嵌入第三方网页及跨域请求设计。

## 特性

- **绕过限制**: 自动移除 Content-Security-Policy (CSP) 与 X-Frame-Options 限制，使被限制的网页可在 iframe 中正常嵌入加载。
- **HTTP 磁盘缓存中间件**: 内置遵循 RFC 9111 标准的 HTTP 缓存中间件（基于 `http-cache-reqwest`），弥补 WebView 自定义协议无法享受原生 Disk Cache 的缺陷；静态资源在 `Cache-Control: max-age` 有效期内直接命中本地磁盘缓存，支持条件请求（ETag / 304 Not Modified）自动 revalidate，大幅降低加载耗时与网络带宽。
- **保持原始路径与来源**: 自定义协议映射原始主机与路径，相对资源和跨域行为自然处理。
- **移动端与全平台支持**:
  - macOS / iOS / Linux: 使用原生自定义协议 `webproxy://`
  - Android / Windows: 兼容 Wry 的 HTTP/HTTPS workaround (`http(s)://webproxy.<host>/...`)
- **自动代理动态请求**: 自动拦截代理页面内的 `fetch` 与 `XMLHttpRequest`，将绝对路径的 HTTPS 请求重定向至 webproxy，彻底解决页面内 API 的 CORS 跨域及 Cookie 丢失问题。
- **注入状态桥接脚本**: 自动在代理页面注入轻量级 state bridge，拦截 iframe 内部的导航、`window.open` 及标题变动，并通过 `postMessage` 向宿主应用同步原始真实目标 URL。
- **Cookie 保持**: 原生 Rust HTTP 客户端自带 cookie store，保持目标站点的登录/状态会话。

## 安装

### 1. Rust 依赖 (`src-tauri/Cargo.toml`)

```toml
[dependencies]
tauri-plugin-webproxy = { path = "../tauri-plugin-webproxy" }
```

### 2. 注册插件 (`src-tauri/src/lib.rs`)

默认配置（自动使用应用的 cache 目录存放 HTTP 磁盘缓存）：

```rust
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_webproxy::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

自定义缓存配置（可选）：

```rust
use tauri_plugin_webproxy::{CacheConfig, CacheMode};

pub fn run() {
    let cache_config = CacheConfig::new()
        .enabled(true)
        // 可自定义缓存目录，若不指定则自动保存在 AppCacheDir 下
        // .cache_dir("/custom/cache/dir")
        .cache_mode(CacheMode::Default);

    tauri::Builder::default()
        .plugin(tauri_plugin_webproxy::init_with_config(cache_config))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

### 3. 前端依赖

```bash
npm install tauri-plugin-webproxy-api
# 或使用本地路径引用
```

## 使用方法

### 转换为 Webproxy URL

```typescript
import { toWebproxyUrl, onWebProxyState } from 'tauri-plugin-webproxy-api'

// 将目标 HTTPS URL 转为 webproxy URL
const proxiedUrl = toWebproxyUrl('https://example.com')

// 在 iframe 中加载
const iframe = document.createElement('iframe')
iframe.src = proxiedUrl
document.body.appendChild(iframe)

// 监听 iframe 内部状态及导航变化（state.url 为真实原始 URL，无需额外解码）
const stopListening = onWebProxyState((state) => {
  console.log('Target URL:', state.url)
  console.log('Page Title:', state.title)
  console.log('Navigation Target:', state.target)
})
```

## 许可证

MIT

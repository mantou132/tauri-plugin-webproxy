# Tauri Plugin Webproxy

Tauri 插件：提供 `webproxy://` 自定义协议，支持在 WebView 中安全代理网页并绕过 CSP / X-Frame-Options 限制，专为 iframe 嵌入第三方网页及跨域请求设计。

## 特性

- **绕过限制**: 自动移除 Content-Security-Policy (CSP) 与 X-Frame-Options 限制，使被限制的网页可在 iframe 中正常嵌入加载。
- **保持原始路径与来源**: 自定义协议映射原始主机与路径，相对资源和跨域行为自然处理。
- **移动端与全平台支持**:
  - macOS / iOS / Linux: 使用原生自定义协议 `webproxy://`
  - Android / Windows: 兼容 Wry 的 HTTP/HTTPS workaround (`http(s)://webproxy.<host>/...`)
- **注入状态桥接脚本**: 自动在代理页面注入轻量级 state bridge，拦截 iframe 内部的导航、`window.open` 及标题变动，并通过 `postMessage` 向宿主应用同步。
- **Cookie 保持**: 原生 Rust HTTP 客户端自带 cookie store，保持目标站点的登录/状态会话。

## 安装

### 1. Rust 依赖 (`src-tauri/Cargo.toml`)

```toml
[dependencies]
tauri-plugin-webproxy = { path = "../tauri-plugin-webproxy" }
```

### 2. 注册插件 (`src-tauri/src/lib.rs`)

```rust
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_webproxy::init())
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

// 监听 iframe 内部状态及导航变化
const stopListening = onWebProxyState((state) => {
  console.log('Target URL:', state.url)
  console.log('Page Title:', state.title)
  console.log('Navigation Target:', state.target)
})
```

### 也支持 `fetch` 请求

```typescript
const response = await fetch(toWebproxyUrl('https://api.example.com/data'))
const data = await response.json()
```

## 许可证

MIT

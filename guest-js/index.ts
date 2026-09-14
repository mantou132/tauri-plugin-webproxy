import { convertFileSrc } from '@tauri-apps/api/core'

/**
 * State dispatched by the injected webproxy bridge script inside proxied pages/iframes.
 */
export interface WebProxyState {
  url: string
  title: string
  target: '' | '_blank' | string
}

/**
 * The message envelope received via window postMessage from the proxied page.
 */
export interface WebProxyNextStateMessage {
  type: 'next_state'
  state: WebProxyState
}

export const WEBPROXY_SCHEME = 'webproxy'
export const WEBPROXY_PROTOCOL = `${WEBPROXY_SCHEME}:`
export const WEBPROXY_HOST_PREFIX = `${WEBPROXY_SCHEME}.`

/**
 * Converts an HTTPS URL into the host-preserving `webproxy` URL understood by
 * the native protocol handler.
 *
 * Relative URLs inside a proxied HTML document keep working because only the
 * scheme changes logically: `https://example.com/a` -> `webproxy://example.com/a`.
 * On Android/Windows Wry exposes custom protocols through its http(s) workaround
 * (`http(s)://webproxy.example.com/a`); `convertFileSrc` is used only to discover
 * which outer protocol the current WebView was configured to use.
 */
export function toWebproxyUrl(url: string): string {
  const target = new URL(url)
  if (target.protocol !== 'https:') {
    throw new TypeError(`webproxy only supports HTTPS URLs, got ${target.protocol}`)
  }
  if (target.username || target.password) {
    throw new TypeError('webproxy URLs must not contain credentials')
  }
  if (target.hostname.includes(':')) {
    throw new TypeError('webproxy does not currently support IPv6 literal hosts')
  }

  const suffix = `${target.pathname}${target.search}${target.hash}`
  const probe = new URL(convertFileSrc('', WEBPROXY_SCHEME))

  // macOS / iOS / Linux register the real custom scheme.
  if (probe.protocol === WEBPROXY_PROTOCOL) {
    return `${WEBPROXY_PROTOCOL}//${target.host}${suffix}`
  }

  // Android / Windows use Wry's http(s) custom-protocol workaround:
  // webproxy://example.com/a <-> http(s)://webproxy.example.com/a
  return `${probe.protocol}//${WEBPROXY_HOST_PREFIX}${target.host}${suffix}`
}

/**
 * Listens for navigation and state change events posted by the proxied page/iframe.
 * Returns an unsubscribe function to remove the listener.
 */
export function onWebProxyState(source: Window | null, listener: (state: WebProxyState) => void): () => void {
  const eventListener = (event: MessageEvent) => {
    if (source && source !== event.source) return;
    if (event.data?.type !== 'next_state' || !event.data?.state) return
    listener(event.data.state as WebProxyState)
  }

  window.addEventListener('message', eventListener)
  return () => window.removeEventListener('message', eventListener)
}

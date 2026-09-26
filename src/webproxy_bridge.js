(() => {
  if (window.__webproxyStateBridgeInstalled) return;
  Object.defineProperty(window, "__webproxyStateBridgeInstalled", {
    value: true,
    configurable: false,
    enumerable: false,
    writable: false,
  });

  const WEBPROXY_SCHEME = "webproxy";
  const WEBPROXY_PROTOCOL = `${WEBPROXY_SCHEME}:`;
  const WEBPROXY_HOST_PREFIX = `${WEBPROXY_SCHEME}.`;

  const toWebproxyUrl = (value) => {
    try {
      const url = new URL(value, document.baseURI);
      if (url.protocol === "https:") {
        if (location.protocol === WEBPROXY_PROTOCOL) {
          return `${WEBPROXY_PROTOCOL}//${url.host}${url.pathname}${url.search}${url.hash}`;
        }
        if (location.host.startsWith(WEBPROXY_HOST_PREFIX) && !url.host.startsWith(WEBPROXY_HOST_PREFIX)) {
          return `${location.protocol}//${WEBPROXY_HOST_PREFIX}${url.host}${url.pathname}${url.search}${url.hash}`;
        }
      }
      return url.href;
    } catch {
      return String(value);
    }
  };

  const toOriginalUrl = (value) => {
    try {
      const url = new URL(value, document.baseURI);
      if (url.protocol === WEBPROXY_PROTOCOL) {
        return `https://${url.host}${url.pathname}${url.search}${url.hash}`;
      }
      if (url.host.startsWith(WEBPROXY_HOST_PREFIX)) {
        return `https://${url.host.slice(WEBPROXY_HOST_PREFIX.length)}${url.pathname}${url.search}${url.hash}`;
      }
      return url.href;
    } catch {
      return String(value);
    }
  };

  const postState = (url, target) => {
    if (parent === window) return;
    parent.postMessage(
      {
        type: "next_state",
        state: {
          url: toOriginalUrl(url),
          title: document.title,
          target,
        },
      },
      "*",
    );
  };

  // Proxy fetch requests to route through webproxy
  const originalFetch = window.fetch;
  window.fetch = function (input, init) {
    const ctx = this || window;
    try {
      if (typeof input === "string" || input instanceof URL) {
        return originalFetch.call(ctx, toWebproxyUrl(input), init);
      }
      if (input instanceof Request) {
        const proxiedUrl = toWebproxyUrl(input.url);
        if (proxiedUrl !== input.url) {
          return originalFetch.call(ctx, new Request(proxiedUrl, init ? new Request(input, init) : input));
        }
      }
    } catch {
      // Fallback to original fetch
    }
    return originalFetch.call(ctx, input, init);
  };

  // Proxy XMLHttpRequest to route through webproxy
  const originalOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    try {
      url = toWebproxyUrl(url);
    } catch {
      // Fallback
    }
    return originalOpen.call(this, method, url, ...rest);
  };

  window.open = (url, target) => {
    const normalizedTarget = !target || target === "_self" ? "" : "_blank";
    postState(new URL(url == null ? "about:blank" : url, document.baseURI).href, normalizedTarget);
    return null;
  };

  addEventListener(
    "click",
    (event) => {
      const path = typeof event.composedPath === "function" ? event.composedPath() : [];
      let anchor = null;

      for (const node of path) {
        if (node instanceof Element && node.tagName.toLowerCase() === "a" && node.hasAttribute("href")) {
          anchor = node;
          break;
        }
      }
      if (!anchor && event.target instanceof Element) {
        anchor = event.target.closest("a[href]");
      }

      if (!anchor) return;

      const rawHref = anchor.getAttribute("href");
      if (!rawHref || rawHref.startsWith("#") || rawHref.startsWith("javascript:")) {
        return;
      }

      if (anchor.hasAttribute("download")) return;

      let fullHref = "";
      try {
        fullHref = new URL(rawHref, document.baseURI).href;
      } catch {
        return;
      }

      const target = anchor.getAttribute("target") || "";
      const isBlank = target === "_blank";

      postState(fullHref, isBlank ? "_blank" : "");
      event.preventDefault();
    },
    true,
  );

  let lastTitle = document.title;
  const observeTitle = () => {
    const title = document.title;
    if (title === lastTitle) return;
    lastTitle = title;
    postState(location.href, "");
  };

  const observer = new MutationObserver(observeTitle);
  const head = document.head;
  if (head) {
    observer.observe(head, {
      subtree: true,
      childList: true,
      characterData: true,
    });
  }
})();

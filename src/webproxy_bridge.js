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
        if (location.host.startsWith(WEBPROXY_HOST_PREFIX)) {
          return `${location.protocol}//${WEBPROXY_HOST_PREFIX}${url.host}${url.pathname}${url.search}${url.hash}`;
        }
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
          url: toWebproxyUrl(url),
          title: document.title,
          target,
        },
      },
      "*",
    );
  };

  const resolveUrl = (value) => {
    try {
      return new URL(value, document.baseURI).href;
    } catch {
      return String(value);
    }
  };

  // Proxy fetch requests to route through webproxy
  const originalFetch = window.fetch;
  window.fetch = function (input, init) {
    try {
      if (typeof input === "string" || input instanceof URL) {
        return originalFetch.call(this, toWebproxyUrl(input), init);
      }
      if (input instanceof Request) {
        const proxiedUrl = toWebproxyUrl(input.url);
        if (proxiedUrl !== input.url) {
          return originalFetch.call(this, new Request(proxiedUrl, input), init);
        }
      }
    } catch {
      // Fallback to original fetch
    }
    return originalFetch.call(this, input, init);
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
    postState(resolveUrl(url == null ? "about:blank" : String(url)), normalizedTarget);
    return null;
  };

  addEventListener(
    "click",
    (event) => {
      const path = typeof event.composedPath === "function" ? event.composedPath() : [];
      const anchor =
        path.find(
          (node) =>
            node instanceof HTMLAnchorElement &&
            node.hasAttribute("href"),
        ) ||
        (event.target instanceof Element
          ? event.target.closest('a[href]')
          : null);

      if (!anchor) return;
      if (anchor.href.startsWith("#")) return;

      const target = !anchor.target || anchor.target === "_self" ? "" : "_blank";
      event.preventDefault();
      postState(anchor.href, target);
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

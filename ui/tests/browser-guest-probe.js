/* Synchronous facts only: no cookies, storage, headers or token contents. */
(() => {
  const native = (value) => typeof value === "function" && Function.prototype.toString.call(value).includes("[native code]");
  const state = window.__GUEST_STATE__;
  return {
    url: location.href,
    ready: document.readyState,
    ua: navigator.userAgent,
    hidden: document.hidden,
    globals: Object.getOwnPropertyNames(window).filter((name) => /^__TAURI|^__zerocode|^isTauri$|^ipc$/.test(name)),
    enumerable: Object.keys(window).filter((name) => /^__TAURI|^__zerocode|^__GUEST_STATE__$/.test(name)),
    native: { fetch: native(fetch), open: native(XMLHttpRequest.prototype.open), send: native(XMLHttpRequest.prototype.send), log: native(console.log) },
    notification: { present: typeof Notification, native: native(window.Notification) },
    ts: document.querySelector("[name=cf-turnstile-response]")?.value.length ?? 0,
    safari: typeof window.safari,
    applePay: typeof window.ApplePaySession,
    ipcHandler: typeof window.webkit?.messageHandlers?.ipc,
    webkitHandlers: Object.keys(window.webkit?.messageHandlers || {}),
    text: document.body?.innerText.slice(0, 500),
    errors: state?.ring.take("console", 0, { level: "warn" }).entries,
  };
})()

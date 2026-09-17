/* The observation ring — a private guest state slot in a guest page.
 *
 * Planted by Rust at document start (`initialization_script`, after the guest cloak; docs/design/
 * browser-door-for-agents.md §2.1) so that `zerocode-browser console|network|
 * diagnose` can say what a page did without a devtools window. The page gets
 * no IPC: this file only WRITES into three bounded rings, and Rust READS them
 * by polling `take` over the same eval callback the menu and grab roads use.
 *
 * Rules the pane relies on:
 *  - main frame only, idempotent, one page global — nothing else is added;
 *  - `console`, `fetch` and `XMLHttpRequest` are wrapped but the originals
 *    run untouched, failures included: observation changes no behaviour;
 *  - headers and bodies are never held (a credential's habitat); URLs lose
 *    their userinfo and sensitive query/hash values HERE, texts lose
 *    credential-shaped words HERE, and Rust scrubs the answer again;
 *  - every cap is a number in the table below; `seq` is monotonic and the
 *    rings are never emptied — the window and an agent may read the same
 *    ring, each with its own `since` cursor.
 */
(() => {
  if (window.top !== window) return;
  const key = "__GUEST_STATE__";
  if (window[key]?.ring) return;
  const state = { ring: null, menu: null, menuWatching: false };

  const CAPS = Object.freeze({
    console: 500,
    network: 500,
    errors: 100,
    text: 1_000,
    url: 2_000,
    budget: 20_000,
  });
  const SENSITIVE = [
    "authorization", "credential", "password", "passwd", "cookie",
    "secret", "sessionid", "token", "apikey", "privatekey",
  ];
  const CREDENTIAL_PREFIXES = ["bearer ", "basic ", "sk-", "ghp_", "github_pat_", "xoxb-", "xoxp-"];

  const sensitive = (name) => {
    const folded = String(name || "").toLowerCase().replace(/[^a-z0-9]/g, "");
    return SENSITIVE.some((word) => folded.includes(word));
  };

  // The same shapes `looks_like_credential` refuses on the Rust side: a
  // known prefix, or three dot-joined base64url runs (a JWT).
  const looksLikeCredential = (word) => {
    const lower = word.toLowerCase();
    if (CREDENTIAL_PREFIXES.some((prefix) => lower.startsWith(prefix))) return true;
    const parts = word.split(".");
    return parts.length === 3 && parts.every((part) => part.length >= 8 && /^[A-Za-z0-9_=-]+$/.test(part));
  };

  const scrubUrl = (raw) => {
    const text = String(raw == null ? "" : raw);
    try {
      const url = new URL(text, location.href);
      url.username = "";
      url.password = "";
      for (const key of [...url.searchParams.keys()]) {
        if (sensitive(key)) url.searchParams.set(key, "[redacted]");
      }
      if (url.hash) {
        const hash = new URLSearchParams(url.hash.slice(1));
        let changed = false;
        for (const key of [...hash.keys()]) {
          if (sensitive(key)) {
            hash.set(key, "[redacted]");
            changed = true;
          }
        }
        if (changed) url.hash = hash.toString();
      }
      return url.href.slice(0, CAPS.url);
    } catch (_) {
      return text.slice(0, CAPS.url);
    }
  };

  // A line of text: URLs inside it are scrubbed as URLs, `key=value` and
  // `key: value` pairs with a sensitive key lose their value, and any word
  // shaped like a credential is replaced whole. Bearer/Basic are two words.
  const scrubText = (raw) => {
    let text = String(raw == null ? "" : raw);
    text = text.replace(/\b(bearer|basic)\s+\S+/gi, "[redacted]");
    text = text.replace(/https?:\/\/[^\s"'<>]+/g, (url) => scrubUrl(url));
    text = text.replace(/([A-Za-z_][A-Za-z0-9_.-]*)(\s*[=:]\s*)([^\s,;&]+)/g, (whole, key, joint, value) =>
      sensitive(key) && !/^\[redacted\]/.test(value) ? `${key}${joint}[redacted]` : whole);
    text = text.replace(/\S+/g, (word) => (looksLikeCredential(word) ? "[redacted]" : word));
    return text.slice(0, CAPS.text);
  };

  const formatValue = (value) => {
    if (typeof value === "string") return value;
    if (value instanceof Error) return `${value.name}: ${value.message}`;
    if (value === undefined) return "undefined";
    if (typeof value === "function" || typeof value === "symbol") return `[${typeof value}]`;
    if (typeof value === "bigint") return `${value}n`;
    if (value && typeof value === "object") {
      try {
        const seen = new WeakSet();
        return JSON.stringify(value, function (key, held) {
          if (key && sensitive(key)) return "[redacted]";
          if (typeof held === "bigint") return `${held}n`;
          if (typeof held === "function" || typeof held === "symbol") return `[${typeof held}]`;
          if (held && typeof held === "object") {
            if (seen.has(held)) return "[circular]";
            seen.add(held);
          }
          return held;
        });
      } catch (_) {
        return "[unserializable]";
      }
    }
    return String(value);
  };

  const formatArgs = (args) => {
    const words = [];
    for (let at = 0; at < args.length; at += 1) words.push(formatValue(args[at]));
    return scrubText(words.join(" "));
  };

  let seq = 0;
  const rings = { console: [], network: [], errors: [] };
  const push = (kind, entry) => {
    seq += 1;
    entry.seq = seq;
    entry.at = Date.now();
    const ring = rings[kind];
    ring.push(entry);
    const over = ring.length - CAPS[kind];
    if (over > 0) ring.splice(0, over);
  };

  // What the navigation itself said — the first line of any diagnosis.
  const navigation = (() => {
    try {
      return performance.getEntriesByType("navigation")[0] || null;
    } catch (_) {
      return null;
    }
  })();
  const documentFacts = Object.freeze({
    status: navigation && typeof navigation.responseStatus === "number" && navigation.responseStatus > 0
      ? navigation.responseStatus
      : null,
    type: navigation ? String(navigation.type || "") || null : null,
    url: scrubUrl(location.href),
  });

  // ---- console -----------------------------------------------------------
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level];
    if (typeof original !== "function") continue;
    console[level] = new Proxy(original, { apply(target, receiver, args) {
      try {
        push("console", { level, text: formatArgs(args) });
      } catch (_) {
        // Observation never fails the page.
      }
      return Reflect.apply(target, receiver, args);
    } });
  }

  // ---- fetch ---------------------------------------------------------------
  const originalFetch = window.fetch;
  if (typeof originalFetch === "function") {
    window.fetch = new Proxy(originalFetch, { apply(target, receiver, args) {
      const [input, init] = args;
      let method = "GET";
      let url = "";
      try {
        const request = typeof Request === "function" && input instanceof Request ? input : null;
        method = String((init && init.method) || (request && request.method) || "GET").toUpperCase();
        url = scrubUrl(request ? request.url : input);
      } catch (_) {
        // Malformed arguments are the original's to refuse.
      }
      const started = performance.now();
      const promise = Reflect.apply(target, receiver, args);
      try {
        promise.then(
          (response) => {
            push("network", {
              kind: "fetch", method, url,
              status: response.status, ok: response.ok,
              ms: Math.round(performance.now() - started),
            });
          },
          (error) => {
            push("network", {
              kind: "fetch", method, url,
              status: 0, ok: false,
              ms: Math.round(performance.now() - started),
              error: error && error.name ? String(error.name) : "network",
            });
          },
        );
      } catch (_) {
        // A fetch that returned no promise is not ours to reason about.
      }
      return promise;
    } });
  }

  // ---- XMLHttpRequest ------------------------------------------------------
  const xhrProto = typeof XMLHttpRequest === "function" ? XMLHttpRequest.prototype : null;
  if (xhrProto && typeof xhrProto.open === "function" && typeof xhrProto.send === "function") {
    const originalOpen = xhrProto.open;
    const originalSend = xhrProto.send;
    const openings = new WeakMap();
    xhrProto.open = new Proxy(originalOpen, { apply(target, receiver, args) {
      const [method, url] = args;
      try {
        openings.set(receiver, { method: String(method || "GET").toUpperCase(), url: scrubUrl(url) });
      } catch (_) {
        // Nothing to record is not a reason to refuse the open.
      }
      return Reflect.apply(target, receiver, args);
    } });
    xhrProto.send = new Proxy(originalSend, { apply(target, receiver, args) {
      try {
        const opened = openings.get(receiver) || { method: "GET", url: "" };
        const started = performance.now();
        receiver.addEventListener("loadend", () => {
          const status = Number(receiver.status) || 0;
          push("network", {
            kind: "xhr", method: opened.method, url: opened.url,
            status, ok: status >= 200 && status < 400,
            ms: Math.round(performance.now() - started),
            ...(status === 0 ? { error: "network" } : {}),
          });
        }, { once: true });
      } catch (_) {
        // As above.
      }
      return Reflect.apply(target, receiver, args);
    } });
  }

  // ---- everything else the page loads ---------------------------------------
  // Images, scripts, stylesheets, fonts: no wrapper to put them behind, so the
  // resource timeline tells. `responseStatus` is absent in older engines and 0
  // for a cross-origin resource that did not allow timing — both read as
  // "unknown", never as a failure.
  if (typeof PerformanceObserver === "function") {
    try {
      const observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          const kind = String(entry.initiatorType || "other");
          if (kind === "fetch" || kind === "xmlhttprequest") continue;
          const status = typeof entry.responseStatus === "number" && entry.responseStatus > 0
            ? entry.responseStatus
            : null;
          push("network", {
            kind, method: "GET", url: scrubUrl(entry.name),
            status, ok: status === null ? null : status < 400,
            ms: Math.round(entry.duration || 0),
          });
        }
      });
      observer.observe({ type: "resource", buffered: true });
    } catch (_) {
      // An engine without resource timing simply has no rows of this kind.
    }
  }

  // ---- errors -----------------------------------------------------------------
  // Listeners beside the page's own, never `window.onerror =` over them.
  window.addEventListener("error", (event) => {
    if (!event || typeof event.message !== "string") return;
    push("errors", {
      text: scrubText(event.message),
      source: scrubUrl(event.filename || ""),
      line: Number(event.lineno) || 0,
      col: Number(event.colno) || 0,
    });
  });
  window.addEventListener("unhandledrejection", (event) => {
    const reason = event ? event.reason : undefined;
    push("errors", { text: scrubText(formatValue(reason)), source: "", line: 0, col: 0 });
  });

  // ---- the read ----------------------------------------------------------------
  const LEVELS = { error: ["error"], warn: ["warn", "error"], all: null };
  const take = (kind, since, options) => {
    const ring = rings[String(kind)] || [];
    const from = Number(since) || 0;
    const held = options && typeof options === "object" ? options : {};
    const budget = Number(held.budget) > 0 ? Number(held.budget) : CAPS.budget;
    const levels = kind === "console" ? LEVELS[held.level] === undefined ? null : LEVELS[held.level] : null;
    const failedOnly = kind === "network" && held.failed === true;
    const entries = [];
    let spent = 0;
    let more = false;
    let last = from;
    for (const entry of ring) {
      if (entry.seq <= from) continue;
      if (levels && !levels.includes(entry.level)) continue;
      if (failedOnly && entry.ok !== false) continue;
      const cost = JSON.stringify(entry).length;
      if (spent + cost > budget && entries.length > 0) {
        more = true;
        break;
      }
      spent += cost;
      entries.push(entry);
      last = entry.seq;
    }
    return { seq, last, more, document: documentFacts, entries };
  };

  state.ring = Object.freeze({ caps: CAPS, document: documentFacts, take });
  Object.defineProperty(window, key, {
    value: Object.seal(state),
    writable: false,
    configurable: false,
    enumerable: false,
  });
})();

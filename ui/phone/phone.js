/* The phone app: one list of this window's agents, and a way to be told.
 *
 * Plain DOM and fetch, no framework and no window internals: the same files
 * can later sit inside an app shell. The device token lives in this origin's
 * localStorage; a 401 forgets it and shows pairing again. */
(() => {
  "use strict";

  const { language, phoneT } = self.PHONE_I18N;
  const TOKEN_KEY = "zerocode.phone.token";
  const POLL_MS = 5000;
  const lang = language(navigator.languages?.length ? navigator.languages : [navigator.language]);
  const t = (key, vars) => phoneT(lang, key, vars);
  const el = (id) => document.getElementById(id);

  let vapidKey = null;
  let pollTimer = null;
  let lastPanes = null;

  function readToken() {
    try { return localStorage.getItem(TOKEN_KEY); } catch { return null; }
  }
  function writeToken(token) {
    try {
      if (token) localStorage.setItem(TOKEN_KEY, token);
      else localStorage.removeItem(TOKEN_KEY);
    } catch { /* private mode: the session keeps it in memory below */ }
    memoryToken = token;
  }
  let memoryToken = readToken();

  function applyTheme() {
    const light = matchMedia("(prefers-color-scheme: light)").matches;
    document.documentElement.dataset.theme = light ? "light" : "dark";
  }

  function applyWords() {
    document.documentElement.lang = lang;
    for (const node of document.querySelectorAll("[data-i18n]")) {
      node.textContent = t(node.dataset.i18n);
    }
  }

  async function api(method, path, body) {
    const headers = { Accept: "application/json" };
    if (memoryToken) headers.Authorization = `Bearer ${memoryToken}`;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const response = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      cache: "no-store",
      credentials: "omit",
    });
    let value = null;
    try { value = await response.json(); } catch { value = null; }
    return { status: response.status, value };
  }

  function show(id, visible) {
    el(id).hidden = !visible;
  }

  function say(id, words) {
    const node = el(id);
    node.textContent = words ?? "";
    node.hidden = !words;
  }

  /* ---- pairing ---------------------------------------------------------- */

  function phoneName() {
    const agent = navigator.userAgent || "";
    if (/iPad/.test(agent)) return "iPad";
    if (/iPhone/.test(agent)) return "iPhone";
    if (/Android/.test(agent)) return "Android";
    return "Phone";
  }

  async function pair(code) {
    const submit = el("phone-pair-submit");
    submit.disabled = true;
    submit.textContent = t("pair.working");
    say("phone-pair-error", "");
    try {
      const { status, value } = await api("POST", "/api/pair", {
        code: String(code).trim().toUpperCase(),
        name: phoneName(),
      });
      if (status === 200 && value?.token) {
        writeToken(value.token);
        vapidKey = value.vapidPublicKey ?? null;
        history.replaceState(null, "", "/");
        await start();
        return;
      }
      const reason = value?.error;
      say("phone-pair-error",
        reason === "owner" ? t("pair.owner")
          : reason === "code" ? t("pair.code_wrong")
            : status === 410 || reason === "burned" ? t("pair.code_gone")
              : t("pair.failed"));
    } catch {
      say("phone-pair-error", t("pair.failed"));
    } finally {
      submit.disabled = false;
      submit.textContent = t("pair.submit");
    }
  }

  function showPairing() {
    stopPolling();
    show("phone-list", false);
    show("phone-notify", false);
    show("phone-install", false);
    show("phone-pair", true);
    el("phone-machine").textContent = "";
  }

  /* ---- notifications ---------------------------------------------------- */

  const standalone = () => navigator.standalone === true
    || matchMedia("(display-mode: standalone)").matches;
  const isIos = () => /iPhone|iPad|iPod/.test(navigator.userAgent || "");
  const pushSupported = () => "serviceWorker" in navigator && "PushManager" in self
    && "Notification" in self;

  function keyBytes(base64url) {
    const padded = base64url.replace(/-/g, "+").replace(/_/g, "/")
      + "===".slice((base64url.length + 3) % 4);
    return Uint8Array.from(atob(padded), (char) => char.charCodeAt(0));
  }

  async function subscribe() {
    if (!vapidKey) return false;
    const registration = await navigator.serviceWorker.ready;
    let subscription = await registration.pushManager.getSubscription();
    if (!subscription) {
      subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: keyBytes(vapidKey),
      });
    }
    const { status } = await api("POST", "/api/push/subscribe", subscription.toJSON());
    if (status === 401) {
      writeToken(null);
      showPairing();
      return false;
    }
    return status === 200;
  }

  async function paintNotify() {
    show("phone-notify", true);
    const button = el("phone-notify-enable");
    if (!pushSupported()) {
      show("phone-install", isIos() && !standalone());
      button.hidden = true;
      say("phone-notify-state", isIos() && !standalone() ? "" : t("notify.unsupported"));
      return;
    }
    show("phone-install", false);
    if (Notification.permission === "denied") {
      button.hidden = true;
      say("phone-notify-state", t("notify.denied"));
      return;
    }
    if (Notification.permission === "granted") {
      button.hidden = true;
      const ok = await subscribe().catch(() => false);
      say("phone-notify-state", ok ? t("notify.on") : t("list.offline"));
      return;
    }
    button.hidden = false;
    say("phone-notify-state", "");
  }

  async function enableNotifications() {
    const button = el("phone-notify-enable");
    button.disabled = true;
    try {
      await Notification.requestPermission();
    } finally {
      button.disabled = false;
    }
    await paintNotify();
  }

  /* ---- the list --------------------------------------------------------- */

  function ago(sinceMs, nowMs) {
    if (!sinceMs) return "";
    const minutes = Math.max(0, Math.floor((nowMs - sinceMs) / 60000));
    if (minutes < 1) return t("time.now");
    if (minutes < 60) return t("time.minutes", { n: minutes });
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return t("time.hours", { n: hours });
    return t("time.days", { n: Math.floor(hours / 24) });
  }

  function row(pane, nowMs) {
    const item = document.createElement("li");
    item.className = "phone-row";
    item.dataset.group = pane.group;
    const mark = document.createElement("span");
    mark.className = "phone-dot";
    mark.setAttribute("aria-hidden", "true");
    const text = document.createElement("div");
    text.className = "phone-row-text";
    const title = document.createElement("p");
    title.className = "phone-row-title";
    title.textContent = pane.task ? `${pane.name} · ${pane.task}` : pane.name;
    const sub = document.createElement("p");
    sub.className = "phone-row-sub";
    const words = [pane.agent];
    if (pane.waiting) words.push(t(`wait.${pane.waiting}`));
    const when = ago(pane.sinceMs, nowMs);
    if (when) words.push(when);
    sub.textContent = words.filter(Boolean).join(" · ");
    text.append(title, sub);
    item.append(mark, text);
    return item;
  }

  function paintList(answer) {
    const groups = el("phone-groups");
    const nowMs = answer.now || Date.now();
    const panes = Array.isArray(answer.panes) ? answer.panes : [];
    const sections = [];
    for (const group of ["attention", "working", "done"]) {
      const members = panes
        .filter((pane) => pane.group === group)
        .sort((one, other) => (other.sinceMs || 0) - (one.sinceMs || 0));
      if (!members.length) continue;
      const section = document.createElement("section");
      section.className = "phone-group";
      section.dataset.group = group;
      const head = document.createElement("h2");
      head.className = "phone-group-head";
      const label = document.createElement("span");
      label.textContent = t(`group.${group}`);
      const count = document.createElement("span");
      count.className = "phone-count";
      count.textContent = String(members.length);
      head.append(label, count);
      const list = document.createElement("ul");
      list.className = "phone-rows";
      list.append(...members.map((pane) => row(pane, nowMs)));
      section.append(head, list);
      sections.push(section);
    }
    groups.replaceChildren(...sections);
    show("phone-list-empty", sections.length === 0);
    const time = new Date(nowMs).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });
    el("phone-updated").textContent = t("list.updated", { time });
  }

  async function refresh() {
    try {
      const { status, value } = await api("GET", "/api/panes");
      if (status === 401) {
        writeToken(null);
        showPairing();
        return;
      }
      if (status !== 200 || !value) throw new Error(String(status));
      lastPanes = value;
      say("phone-list-error", "");
      paintList(value);
    } catch {
      say("phone-list-error", t("list.offline"));
      if (lastPanes) paintList(lastPanes);
    }
  }

  function stopPolling() {
    if (pollTimer !== null) clearInterval(pollTimer);
    pollTimer = null;
  }

  function startPolling() {
    stopPolling();
    if (document.visibilityState !== "visible") return;
    pollTimer = setInterval(() => void refresh(), POLL_MS);
  }

  async function start() {
    show("phone-pair", false);
    const { status, value } = await api("GET", "/api/status").catch(() => ({ status: 0 }));
    if (status === 401) {
      writeToken(null);
      showPairing();
      return;
    }
    if (status === 200 && value) {
      el("phone-machine").textContent = value.name || "";
      vapidKey = value.vapidPublicKey ?? vapidKey;
    }
    show("phone-list", true);
    await refresh();
    startPolling();
    await paintNotify().catch(() => {});
  }

  /* ---- boot ------------------------------------------------------------- */

  applyTheme();
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", applyTheme);
  applyWords();
  if (pushSupported()) {
    navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch(() => {});
  }
  el("phone-pair-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const code = el("phone-pair-code").value;
    if (code.trim()) void pair(code);
  });
  el("phone-notify-enable").addEventListener("click", () => void enableNotifications());
  document.addEventListener("visibilitychange", () => {
    if (!memoryToken) return;
    if (document.visibilityState === "visible") {
      void refresh();
      startPolling();
    } else {
      stopPolling();
    }
  });

  const offered = location.pathname === "/pair" ? location.hash.slice(1) : "";
  if (offered && isIos() && !standalone()) {
    // A Home Screen app keeps its own storage: a token taken here, in Safari,
    // would not reach it. The code is kept on screen, unspent, for the app.
    showPairing();
    el("phone-pair-code").value = offered;
    show("phone-install", true);
  } else if (offered) {
    showPairing();
    el("phone-pair-code").value = offered;
    void pair(offered);
  } else if (memoryToken) {
    void start();
  } else {
    showPairing();
  }
})();

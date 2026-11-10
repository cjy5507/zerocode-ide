/* The phone app's service worker: it words a push notice and opens the app.
 *
 * It caches nothing — the list is only worth reading live — and it composes
 * the notice from names the window sent (agent, checkout folder, task id) in
 * the phone's own language. */
"use strict";

importScripts("/phone-i18n.js");

const { language, phoneT } = self.PHONE_I18N;

function words(notice) {
  const lang = language(self.navigator?.languages?.length ? self.navigator.languages : [self.navigator?.language]);
  const t = (key, vars) => phoneT(lang, key, vars);
  if (notice.kind === "test") {
    return { title: "ZeroCode", body: t("push.test"), tag: "zerocode-test" };
  }
  const who = [notice.agent, notice.name].filter(Boolean).join(" · ") || "ZeroCode";
  const line = notice.kind === "permission" ? t("push.permission") : t("push.question");
  const task = notice.task ? ` — ${notice.task}` : "";
  const more = notice.more > 0 ? ` (${t("push.more", { n: notice.more })})` : "";
  return { title: who, body: `${line}${task}${more}`, tag: notice.pane || "zerocode" };
}

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let notice = {};
  try { notice = event.data ? event.data.json() : {}; } catch { notice = {}; }
  const shown = words(notice);
  event.waitUntil(self.registration.showNotification(shown.title, {
    body: shown.body,
    tag: shown.tag,
    renotify: true,
    icon: "/icon.svg",
    data: { url: "/" },
  }));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil((async () => {
    const open = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    for (const client of open) {
      if ("focus" in client) return client.focus();
    }
    return self.clients.openWindow("/");
  })());
});

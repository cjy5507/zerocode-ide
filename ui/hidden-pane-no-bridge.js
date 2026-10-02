/* What a hidden pane's page is told before its own first script runs: there is
 * no backend here (`artifact_thumbs::hidden_pane`, t-20972).
 *
 * Tauri hands every webview its global API — `window.__TAURI__`, which this
 * window's own scripts read on their very first line (`ui/shell-boot.js`) — and
 * a hidden pane takes one picture (a thumbnail, an export) of a page an artifact
 * names, so it has no use for one. What it did have was a hazard: an artifact
 * may be this window's own `ui/index.html`, and that page, finding the API,
 * boots a second window whose every request the backend refuses (the pane is no
 * webview it knows), and whose fault reporter answered each refusal with
 * another request (`ui/shell-boot.js`, `tellWindowLog`; `tools/ipc-road` measures
 * about ten thousand a second from such a page). The 7.4 s hang of 2026-10-01
 * 17:28 ended with the thumbnail of exactly that page.
 *
 * With the API taken away such a page stops at its first line, as any page does
 * that is opened outside the app, and asks for nothing. An ordinary artifact
 * never touches it, so its picture is the same.
 *
 * Run as an initialization script, after Tauri's own (which come first, and
 * which put the API there by plain assignment, so it can be removed). Tauri's
 * `__TAURI_INTERNALS__` is defined as a property that cannot be removed, and no
 * script of this window reads it before the API.
 *
 * Nothing here changes what the backend allows: it refused every request of
 * this pane before (a `file://` page sends no origin Tauri can read, and the
 * picture of 2026-10-01 says so) and refuses them now. This only stops the page
 * from asking. */
try {
  delete window.__TAURI__;
} catch {
  // Not ours to remove.
}

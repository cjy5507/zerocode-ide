/* Guest-only cleanup, before the observation ring. No persistent globals.
 * Tauri's non-configurable globals cannot be erased by JavaScript: macOS
 * removes their user scripts before navigating to any guest document.
 * This is a defensive cleanup for configurable leftovers, not an IPC ACL.
 */
(() => {
  if (window.top !== window) return;
  const injected = Object.getOwnPropertyNames(window).filter((name) => /^__TAURI/.test(name));
  if (!injected.length) return;

  // Tauri 2.11's macOS scripts are main-frame-only. A same-origin blank
  // frame therefore supplies the engine's Notification constructor, even
  // after the plugin has overwritten ours. Never disguise an IPC shim as
  // native when the engine cannot provide the real implementation.
  let frame;
  let root;
  try {
    if (!document.documentElement) {
      root = document.createElement("html");
      document.appendChild(root);
    }
    frame = document.createElement("iframe");
    frame.hidden = true;
    frame.src = "about:blank";
    document.documentElement.appendChild(frame);
    const native = frame.contentWindow;
    if (native && !Object.getOwnPropertyNames(native).some((name) => /^__TAURI/.test(name))) {
      const descriptor = Object.getOwnPropertyDescriptor(native, "Notification");
      if (descriptor) Object.defineProperty(window, "Notification", descriptor);
    }
  } catch (_) {
    // Unsupported/blocked frames leave the platform's behavior intact.
  } finally {
    if (frame) frame.remove();
    if (root) root.remove();
  }
  for (const name of injected) {
    if (Object.getOwnPropertyDescriptor(window, name)?.configurable) delete window[name];
  }
})();

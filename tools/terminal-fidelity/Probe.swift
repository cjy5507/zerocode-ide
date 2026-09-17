// A WKWebView hosted the way the ZeroCode window hosts its own, showing one
// probe page, captured once, then gone.
//
//   swiftc -O tools/terminal-fidelity/Probe.swift -o <scratch>/terminal-probe
//   <scratch>/terminal-probe <page.html> <read-access-root> <out.png> [settle-seconds]
//
// What "the way the window hosts it" means, and where each piece comes from:
// - a transparent window (`"transparent": true`, crates/zerocode-shell/tauri.conf.json)
//   with a full-size content view under an overlay title bar (`titleBarStyle: Overlay`);
// - the HUD material behind the web view (`Effect::HudWindow`,
//   `apply_window_material` in crates/zerocode-shell/src/main.rs);
// - a web view that does not draw its own background (wry's transparent webview
//   sets `drawsBackground` false).
// Font smoothing in WebKit depends on whether the layer a glyph lands in is
// opaque, so an opaque test window would measure a different renderer.

import Cocoa
import WebKit

final class Probe: NSObject, NSApplicationDelegate, WKNavigationDelegate {
    private let page: URL
    private let root: URL
    private let out: URL
    private let settle: TimeInterval
    private var window: NSWindow!
    private var web: WKWebView!

    init(page: URL, root: URL, out: URL, settle: TimeInterval) {
        self.page = page
        self.root = root
        self.out = out
        self.settle = settle
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Wide enough for the 80×30 fixture at the preferences' font size with
        // room to spare; the analyser finds the grid wherever it lands.
        let frame = NSRect(x: 120, y: 120, width: 1000, height: 560)
        window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isOpaque = false
        window.backgroundColor = .clear
        let material = NSVisualEffectView(frame: NSRect(origin: .zero, size: frame.size))
        material.material = .hudWindow
        material.state = .active
        material.blendingMode = .behindWindow
        material.autoresizingMask = [.width, .height]
        window.contentView = material
        web = WKWebView(frame: material.bounds, configuration: WKWebViewConfiguration())
        web.autoresizingMask = [.width, .height]
        web.setValue(false, forKey: "drawsBackground")
        web.navigationDelegate = self
        material.addSubview(web)
        // On screen for the capture, but never the key window: a person may be
        // typing into another app while the probe runs.
        window.orderFrontRegardless()
        web.loadFileURL(page, allowingReadAccessTo: root)
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        DispatchQueue.main.asyncAfter(deadline: .now() + settle) { [self] in
            let capture = Process()
            capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            capture.arguments = ["-x", "-o", "-l", String(window.windowNumber), out.path]
            do {
                try capture.run()
                capture.waitUntilExit()
            } catch {
                FileHandle.standardError.write("screencapture failed: \(error)\n".data(using: .utf8)!)
            }
            print("{\"png\":\"\(out.path)\",\"window\":\(window.windowNumber),\"scale\":\(window.backingScaleFactor),\"status\":\(capture.terminationStatus)}")
            NSApp.terminate(nil)
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        FileHandle.standardError.write("load failed: \(error)\n".data(using: .utf8)!)
        NSApp.terminate(nil)
    }
}

let arguments = CommandLine.arguments
guard arguments.count >= 4 else {
    FileHandle.standardError.write("usage: terminal-probe <page.html> <read-access-root> <out.png> [settle-seconds]\n".data(using: .utf8)!)
    exit(2)
}
// Long enough for the web fonts' fallback lookups and one composited frame.
let defaultSettle: TimeInterval = 1.5
let probe = Probe(
    page: URL(fileURLWithPath: arguments[1]),
    root: URL(fileURLWithPath: arguments[2]),
    out: URL(fileURLWithPath: arguments[3]),
    settle: arguments.count > 4 ? TimeInterval(arguments[4]) ?? defaultSettle : defaultSettle
)
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.delegate = probe
app.run()

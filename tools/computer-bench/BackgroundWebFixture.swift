import AppKit
import WebKit

@MainActor
final class BackgroundWebWindow: NSWindow {
    var received: ((NSEvent) -> Void)?

    override func sendEvent(_ event: NSEvent) {
        super.sendEvent(event)
        received?(event)
    }
}

@MainActor
final class BackgroundWebFixture: NSObject, NSApplicationDelegate, WKScriptMessageHandler {
    let output: URL
    let showWindows: Bool
    var windows: [NSWindow] = []
    var views: [WKWebView] = []
    var readings: [Int: [String: Any]] = [:]
    var timer: Timer?
    var activationCount = 0
    var events: [[String: Any]] = []
    let pointerObserver = BackgroundPointerObserver()

    init(output: URL, showWindows: Bool) {
        self.output = output
        self.showWindows = showWindows
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        pointerObserver.start()
        for index in 0..<2 {
            let window = BackgroundWebWindow(contentRect: CGRect(x: 40 + index * 420, y: 100, width: 400, height: 380),
                                  styleMask: [.titled, .closable], backing: .buffered, defer: false)
            window.title = "Background web fixture \(index + 1)"
            window.isReleasedWhenClosed = false
            let configuration = WKWebViewConfiguration()
            configuration.websiteDataStore = .nonPersistent()
            configuration.userContentController.add(self, name: "fixture")
            let view = WKWebView(frame: CGRect(x: 0, y: 0, width: 400, height: 380), configuration: configuration)
            window.contentView = view
            window.makeFirstResponder(view)
            window.received = { [weak self, weak window] event in
                guard let self, let window, let raw = event.cgEvent,
                      raw.getIntegerValueField(.eventSourceUserData) != 0 else { return }
                if events.count == 64 { events.removeFirst() }
                events.append(["window": index, "type": event.type.rawValue,
                               "sourcePid": raw.getIntegerValueField(.eventSourceUnixProcessID),
                               "targetPid": raw.getIntegerValueField(.eventTargetUnixProcessID),
                               "x": event.locationInWindow.x, "y": event.locationInWindow.y,
                               "key": window.isKeyWindow, "active": NSApp.isActive])
            }
            windows.append(window)
            views.append(view)
            view.loadHTMLString(Self.page(index), baseURL: nil)
            if showWindows { window.orderFrontRegardless() } else { window.orderBack(nil) }
        }
        timer = Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.write() }
        }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        activationCount += 1
        write()
    }

    func applicationWillTerminate(_ notification: Notification) {
        pointerObserver.stop()
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.name == "fixture", let body = message.body as? [String: Any],
              let index = body["index"] as? Int, views.indices.contains(index),
              message.webView === views[index]
        else { return }
        readings[index] = body
        write()
    }

    func write() {
        guard readings.count == windows.count, windows.count == 2 else { return }
        let height = CGDisplayBounds(CGMainDisplayID()).height
        var rows: [[String: Any]] = []
        for (index, window) in windows.enumerated() {
            guard let reading = readings[index] else { return }
            var row: [String: Any] = [
                "id": window.windowNumber, "x": window.frame.minX, "y": height - window.frame.maxY,
                "width": window.frame.width, "height": window.frame.height, "visible": window.isVisible,
                "clicks": reading["clicks"] ?? 0, "text": reading["text"] ?? "",
                "scrollOffset": reading["scrollOffset"] ?? 0,
                "domEvents": reading["events"] ?? [],
            ]
            for control in ["button", "input", "scroll"] {
                guard let coordinates = reading[control] as? [String: Double],
                      let pointX = coordinates["x"], let pointY = coordinates["y"]
                else { return }
                let local = CGPoint(x: pointX, y: views[index].isFlipped ? pointY : views[index].bounds.height - pointY)
                let cocoa = window.convertPoint(toScreen: views[index].convert(local, to: nil))
                row[control + "X"] = cocoa.x
                row[control + "Y"] = height - cocoa.y
            }
            rows.append(row)
        }
        let cursor = CGEvent(source: nil)?.location
        pointerObserver.sample(cursor)
        let state: [String: Any] = [
            "pid": getpid(), "bundleId": Bundle.main.bundleIdentifier ?? "", "windows": rows,
            "events": events,
            "active": NSApp.isActive, "activationCount": activationCount,
            "sharedPointer": pointerObserver.snapshot,
            "clipboardChanges": NSPasteboard.general.changeCount,
            "foreground": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1,
            "cursorX": cursor?.x ?? 0, "cursorY": cursor?.y ?? 0, "updatedNs": DispatchTime.now().uptimeNanoseconds,
        ]
        if let data = try? JSONSerialization.data(withJSONObject: state, options: [.sortedKeys]) {
            try? data.write(to: output, options: .atomic)
        }
    }

    static func page(_ index: Int) -> String {
        """
        <!doctype html><meta charset="utf-8"><title>Owned background fixture</title>
        <style>
        body { margin: 20px 40px; font: 16px system-ui; }
        button, input { display: block; width: 300px; box-sizing: border-box; height: 36px; margin-bottom: 12px; }
        #scroll { width: 300px; height: 220px; overflow: auto; border: 1px solid; }
        #rows > div { height: 24px; }
        </style>
        <button id="button">Increment</button>
        <input id="input" aria-label="Owned test field" autocomplete="off" autocorrect="off" spellcheck="false">
        <div id="scroll"><div id="rows"></div></div>
        <script>
        const button = document.querySelector('#button');
        const input = document.querySelector('#input');
        const scroll = document.querySelector('#scroll');
        const rows = document.querySelector('#rows');
        let clicks = 0;
        const events = [];
        rows.replaceChildren(...Array.from({length: 100}, (_, number) => {
            const row = document.createElement('div'); row.textContent = `Fixture row ${number + 1}`; return row;
        }));
        function center(element) {
            const rect = element.getBoundingClientRect();
            return {x: rect.x + rect.width / 2, y: rect.y + rect.height / 2};
        }
        function report() {
            window.webkit.messageHandlers.fixture.postMessage({index: \(index), clicks, text: input.value, events,
                scrollOffset: scroll.scrollTop, button: center(button), input: center(input), scroll: center(scroll)});
        }
        button.addEventListener('click', () => { clicks += 1; report(); });
        input.addEventListener('input', report);
        scroll.addEventListener('scroll', report);
        window.addEventListener('resize', report);
        for (const kind of ['mousedown', 'mouseup', 'focusin', 'keydown']) {
            document.addEventListener(kind, event => {
                if (events.length === 32) events.shift();
                events.push({kind, target: event.target.id, x: event.clientX ?? null, y: event.clientY ?? null});
                report();
            });
        }
        report();
        requestAnimationFrame(report);
        </script>
        """
    }
}

@main
struct BackgroundWebFixtureEntry {
    @MainActor
    static func main() {
        guard CommandLine.arguments.count == 2 || (CommandLine.arguments.count == 3 && CommandLine.arguments[2] == "--show-windows") else { exit(2) }
        let application = NSApplication.shared
        application.setActivationPolicy(.accessory)
        let fixture = BackgroundWebFixture(output: URL(fileURLWithPath: CommandLine.arguments[1]),
                                           showWindows: CommandLine.arguments.count == 3)
        application.delegate = fixture
        withExtendedLifetime(fixture) { application.run() }
    }
}

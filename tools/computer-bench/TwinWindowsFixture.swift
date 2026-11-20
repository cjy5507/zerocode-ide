// The person's 11-20 scene on throwaway windows (CU slowness, t-121376): one
// app's windows with one title, the one behind holding the «Authorize» button
// the errand was for. Never activates, sits behind everything, records every
// press as `pressed <window number>` and every key a window takes as
// `key <window number> <key code>`, so a run is judged by the windows' own
// record, names its windows in `ready` (JSON, written once they stand) with
// their frames and their button's centre where a window move reads them, and
// quits by itself after two minutes.
//
//   swiftc -O tools/computer-bench/TwinWindowsFixture.swift -o <dir>/twin-fixture -framework AppKit
//   <dir>/twin-fixture <mode> <folder>
//
// Modes:
//   single      one window with «Authorize» (a scene with no shared frame)
//   twins       two at one frame; «Authorize» behind, «Overview» in front
//   twins-same  two at one frame; «Authorize» in both (a press in the wrong one shows)
//   offset      two of one size, the one behind 22 points off; «Authorize» behind
//   small-over  a smaller window of the app over the target; «Authorize» behind
import AppKit

/// A window's content that takes keys and records them under its window's number.
final class KeyView: NSView {
    var record: ((String) -> Void)?
    override var acceptsFirstResponder: Bool { true }
    override func keyDown(with event: NSEvent) { record?("key \(window?.windowNumber ?? 0) \(event.keyCode)") }
}

final class Fixture: NSObject, NSApplicationDelegate {
    let mode: String, dir: URL
    var windows: [NSWindow] = []
    /// Each window's button, in the order of `windows`.
    var buttons: [NSButton] = []

    init(mode: String, dir: URL) { self.mode = mode; self.dir = dir }

    /// A frame in the coordinates a window move takes: from the main screen's
    /// top-left corner, y down (AppKit's run up from its bottom-left).
    func topLeft(_ rect: NSRect) -> [CGFloat] {
        let height = NSScreen.screens.first?.frame.height ?? rect.maxY
        return [rect.minX, height - rect.maxY, rect.width, rect.height]
    }

    func log(_ line: String) {
        let url = dir.appendingPathComponent("events.log")
        let data = Data((line + "\n").utf8)
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile(); handle.write(data); try? handle.close()
        } else { try? data.write(to: url) }
    }

    @objc func pressed(_ sender: NSButton) { log("pressed \(sender.window?.windowNumber ?? 0)") }

    func makeWindow(at frame: NSRect, body: String, button: String) -> NSWindow {
        let window = NSWindow(contentRect: frame, styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.title = "Home | Fixture"
        window.isReleasedWhenClosed = false
        let content = KeyView(frame: NSRect(origin: .zero, size: frame.size))
        content.record = { [weak self] line in self?.log(line) }
        let label = NSTextField(labelWithString: body)
        label.frame = NSRect(x: 16, y: frame.height - 48, width: 320, height: 20)
        let press = NSButton(title: button, target: self, action: #selector(pressed(_:)))
        press.frame = NSRect(x: 16, y: frame.height - 96, width: 140, height: 28)
        content.addSubview(label); content.addSubview(press)
        buttons.append(press)
        window.contentView = content
        window.initialFirstResponder = content
        window.makeFirstResponder(content)
        return window
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        NSApp.setActivationPolicy(.accessory)
        let screen = NSScreen.screens.first?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1200, height: 800)
        let frame = NSRect(x: screen.maxX - 760, y: screen.minY + 40, width: 720, height: 480)
        switch mode {
        case "single":
            windows = [makeWindow(at: frame, body: "ONLY", button: "Authorize")]
        case "twins-same":
            windows = [makeWindow(at: frame, body: "BACK", button: "Authorize"),
                       makeWindow(at: frame, body: "FRONT", button: "Authorize")]
        case "small-over":
            windows = [makeWindow(at: frame, body: "BACK", button: "Authorize"),
                       makeWindow(at: NSRect(x: frame.minX + 160, y: frame.minY + 90, width: 400, height: 300), body: "FRONT", button: "Overview")]
        case "offset":
            windows = [makeWindow(at: frame.offsetBy(dx: -22, dy: 22), body: "BACK", button: "Authorize"),
                       makeWindow(at: frame, body: "FRONT", button: "Overview")]
        default:
            windows = [makeWindow(at: frame, body: "BACK", button: "Authorize"),
                       makeWindow(at: frame, body: "FRONT", button: "Overview")]
        }
        // The last stands in front of the others, all of them behind every
        // other app's windows: ordered to the back first, the rest below it.
        let front = windows[windows.count - 1]
        front.orderBack(nil)
        for window in windows.dropLast() { window.order(.below, relativeTo: front.windowNumber) }
        let named: [[String: Any]] = zip(windows, buttons).map { window, button in
            let pressed = topLeft(window.convertToScreen(button.convert(button.bounds, to: nil)))
            return ["number": window.windowNumber,
                    "frame": [window.frame.minX, window.frame.minY, window.frame.width, window.frame.height],
                    "top_left": topLeft(window.frame),
                    "button": [pressed[0] + pressed[2] / 2, pressed[1] + pressed[3] / 2]]
        }
        var ready: [String: Any] = ["pid": Int(getpid()), "mode": mode, "front": front.windowNumber, "windows": named,
                                    "screen": topLeft(screen)]
        if windows.count > 1 { ready["back"] = windows[0].windowNumber }
        if let data = try? JSONSerialization.data(withJSONObject: ready) {
            try? data.write(to: dir.appendingPathComponent("ready"), options: .atomic)
        }
        // A bench that dies first leaves no window behind the person's screen (t-122111 K-6).
        DispatchQueue.main.asyncAfter(deadline: .now() + lifetime) { [weak self] in
            self?.log("quit")
            NSApp.terminate(nil)
        }
    }
}

/// How long the fixture stands before it ends itself: past every scene a bench
/// runs on it (a scene takes under 15 s).
let lifetime: TimeInterval = 120

let arguments = CommandLine.arguments
let app = NSApplication.shared
let fixture = Fixture(mode: arguments.count > 1 ? arguments[1] : "twins",
                      dir: URL(fileURLWithPath: arguments.count > 2 ? arguments[2] : FileManager.default.currentDirectoryPath))
app.delegate = fixture
app.run()

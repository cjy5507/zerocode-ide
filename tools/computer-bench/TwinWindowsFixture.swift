// The person's 11-20 scene on throwaway windows (CU slowness, t-121376): one
// app's windows with one title, the one behind holding the «Authorize» button
// the errand was for. Never activates, sits behind everything, records every
// press as `pressed <window number>` so a run is judged by the windows' own
// record, and names its windows in `ready` (JSON, written once they stand).
//
//   swiftc -O tools/computer-bench/TwinWindowsFixture.swift -o <dir>/twin-fixture -framework AppKit
//   <dir>/twin-fixture <mode> <folder>
//
// Modes:
//   single      one window with «Authorize» (a scene with no shared frame)
//   twins       two at one frame; «Authorize» behind, «Overview» in front
//   twins-same  two at one frame; «Authorize» in both (a press in the wrong one shows)
//   offset      two of one size, the one behind 22 points off; «Authorize» behind
import AppKit

final class Fixture: NSObject, NSApplicationDelegate {
    let mode: String, dir: URL
    var windows: [NSWindow] = []

    init(mode: String, dir: URL) { self.mode = mode; self.dir = dir }

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
        let content = NSView(frame: NSRect(origin: .zero, size: frame.size))
        let label = NSTextField(labelWithString: body)
        label.frame = NSRect(x: 16, y: frame.height - 48, width: 320, height: 20)
        let press = NSButton(title: button, target: self, action: #selector(pressed(_:)))
        press.frame = NSRect(x: 16, y: frame.height - 96, width: 140, height: 28)
        content.addSubview(label); content.addSubview(press)
        window.contentView = content
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
        let named: [[String: Any]] = windows.map { window in
            ["number": window.windowNumber, "frame": [window.frame.minX, window.frame.minY, window.frame.width, window.frame.height]]
        }
        var ready: [String: Any] = ["pid": Int(getpid()), "mode": mode, "front": front.windowNumber, "windows": named]
        if windows.count > 1 { ready["back"] = windows[0].windowNumber }
        if let data = try? JSONSerialization.data(withJSONObject: ready) {
            try? data.write(to: dir.appendingPathComponent("ready"), options: .atomic)
        }
    }
}

let arguments = CommandLine.arguments
let app = NSApplication.shared
let fixture = Fixture(mode: arguments.count > 1 ? arguments[1] : "twins",
                      dir: URL(fileURLWithPath: arguments.count > 2 ? arguments[2] : FileManager.default.currentDirectoryPath))
app.delegate = fixture
app.run()

// A web view's late press on a throwaway window (CU slowness, t-121372): the
// app answers `AXManualAccessibility` as settable, as Chromium's application
// does, and its «Authorize» button lists no AXPress until `delay` seconds
// after that was set (the vault measured 1.7 s to 6 s in Chrome). A press
// before the offer answers success and does nothing; a mouse event to a
// window that is not key does nothing either. Beside it a link reads «Go» as
// its description and shows «Delete project» as its text: the confirmation
// check reads «Go» alone (t-121539 I-4, an older gap left open), so a
// refusal must never name it by its text (t-121849).
// Never activates, sits behind everything, writes what happened to
// `events.log`, and names its window in `ready` (JSON) once it stands.
//
//   swiftc -O tools/computer-bench/LatePressFixture.swift -o <dir>/late-fixture -framework AppKit
//   <dir>/late-fixture <mode> <folder> [delay seconds | never]
//
// Modes:
//   web     the app answers AXManualAccessibility; the button offers its press late
//   native  a plain AppKit button and no AXManualAccessibility (a native app's press)
//
// events.log lines (seconds from the switch-on, when there was one):
//   switched-on                      AXManualAccessibility was set true
//   offered <s>                      the button began to list AXPress
//   ignored-press <s>                AXPress before the offer: success, nothing done
//   ignored-mouse                    a mouse event while the window was not key
//   pressed button <s>               the button's press did its work
//   pressed link                     the «Go» / «Delete project» link was pressed
//   pressed native                   the native button was pressed
import AppKit

let manualAttribute = NSAccessibility.Attribute(rawValue: "AXManualAccessibility")

final class Record {
    let dir: URL
    var switchedOnAt: Date?
    let delay: TimeInterval?

    init(dir: URL, delay: TimeInterval?) { self.dir = dir; self.delay = delay }

    func log(_ line: String) {
        let url = dir.appendingPathComponent("events.log")
        let data = Data((line + "\n").utf8)
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile(); handle.write(data); try? handle.close()
        } else { try? data.write(to: url) }
    }

    /// Seconds since the switch-on, as a log line writes them.
    var since: String {
        guard let switchedOnAt else { return "-" }
        return String(format: "%.3f", Date().timeIntervalSince(switchedOnAt))
    }

    /// Whether the button lists its press by now: `delay` after the switch-on.
    var offered: Bool {
        guard let switchedOnAt, let delay else { return false }
        return Date().timeIntervalSince(switchedOnAt) >= delay
    }
}

var record: Record!

/// The application, answering the attribute Chromium's does.
final class FixtureApplication: NSApplication {
    var answersManual = false

    override func accessibilityAttributeNames() -> [NSAccessibility.Attribute] {
        let names = super.accessibilityAttributeNames()
        return answersManual ? names + [manualAttribute] : names
    }

    override func accessibilityAttributeValue(_ attribute: NSAccessibility.Attribute) -> Any? {
        if answersManual, attribute == manualAttribute { return record.switchedOnAt != nil }
        return super.accessibilityAttributeValue(attribute)
    }

    override func accessibilityIsAttributeSettable(_ attribute: NSAccessibility.Attribute) -> Bool {
        if answersManual, attribute == manualAttribute { return true }
        return super.accessibilityIsAttributeSettable(attribute)
    }

    override func accessibilitySetValue(_ value: Any?, forAttribute attribute: NSAccessibility.Attribute) {
        if answersManual, attribute == manualAttribute {
            if record.switchedOnAt == nil, (value as? Bool) == true || (value as? NSNumber)?.boolValue == true {
                record.switchedOnAt = Date()
                record.log("switched-on")
                if let delay = record.delay {
                    DispatchQueue.main.asyncAfter(deadline: .now() + delay) { record.log("offered \(record.since)") }
                }
            }
            return
        }
        super.accessibilitySetValue(value, forAttribute: attribute)
    }
}

/// The web view's button: it lists AXPress only once offered.
final class LateButton: NSView {
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .button }
    override func accessibilityTitle() -> String? { "Authorize" }
    override func accessibilityActionNames() -> [NSAccessibility.Action] { record.offered ? [.press] : [] }

    // No `accessibilityPerformPress`: AppKit would list AXPress for it from the
    // start. The press comes through the action call alone.
    override func accessibilityPerformAction(_ action: NSAccessibility.Action) {
        guard action == .press else { return }
        if record.offered { record.log("pressed button \(record.since)") } else { record.log("ignored-press \(record.since)") }
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { false }

    override func mouseDown(with event: NSEvent) {
        guard window?.isKeyWindow == true else { record.log("ignored-mouse"); return }
        if record.offered { record.log("pressed button \(record.since)") } else { record.log("ignored-press \(record.since)") }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.controlAccentColor.setFill()
        bounds.fill()
        ("Authorize" as NSString).draw(at: NSPoint(x: 12, y: 6), withAttributes: [.foregroundColor: NSColor.white])
    }
}

/// A link whose accessible name is «Go» and whose text says «Delete project».
final class DeleteLink: NSView {
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .link }
    override func accessibilityLabel() -> String? { "Go" }
    override func accessibilityURL() -> URL? { URL(string: "https://example.com/projects/1/delete") }

    override func accessibilityPerformPress() -> Bool {
        record.log("pressed link")
        return true
    }
}

final class Fixture: NSObject, NSApplicationDelegate {
    let mode: String
    var window: NSWindow?

    init(mode: String) { self.mode = mode }

    @objc func pressedNative(_ sender: NSButton) { record.log("pressed native") }

    func applicationDidFinishLaunching(_ note: Notification) {
        NSApp.setActivationPolicy(.accessory)
        let screen = NSScreen.screens.first?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1200, height: 800)
        let frame = NSRect(x: screen.maxX - 760, y: screen.minY + 40, width: 720, height: 480)
        let window = NSWindow(contentRect: frame, styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.title = "Home | Fixture"
        window.isReleasedWhenClosed = false
        let content = NSView(frame: NSRect(origin: .zero, size: frame.size))
        let body = NSTextField(labelWithString: mode == "native" ? "NATIVE" : "WEB")
        body.frame = NSRect(x: 16, y: frame.height - 48, width: 320, height: 20)
        content.addSubview(body)
        if mode == "native" {
            let press = NSButton(title: "Authorize", target: self, action: #selector(pressedNative(_:)))
            press.frame = NSRect(x: 16, y: frame.height - 96, width: 140, height: 28)
            content.addSubview(press)
        } else {
            let press = LateButton(frame: NSRect(x: 16, y: frame.height - 96, width: 140, height: 28))
            content.addSubview(press)
            let link = DeleteLink(frame: NSRect(x: 16, y: frame.height - 140, width: 200, height: 24))
            let text = NSTextField(labelWithString: "Delete project")
            text.frame = NSRect(origin: .zero, size: link.frame.size)
            link.addSubview(text)
            content.addSubview(link)
        }
        window.contentView = content
        window.orderBack(nil)
        self.window = window
        let ready: [String: Any] = [
            "pid": Int(getpid()), "mode": mode, "window": window.windowNumber,
            "delay": record.delay.map { $0 as Any } ?? "never",
            "frame": [window.frame.minX, window.frame.minY, window.frame.width, window.frame.height],
        ]
        if let data = try? JSONSerialization.data(withJSONObject: ready) {
            try? data.write(to: record.dir.appendingPathComponent("ready"), options: .atomic)
        }
    }
}

let arguments = CommandLine.arguments
let mode = arguments.count > 1 ? arguments[1] : "web"
let folder = URL(fileURLWithPath: arguments.count > 2 ? arguments[2] : FileManager.default.currentDirectoryPath)
let delay: TimeInterval? = arguments.count > 3 ? (arguments[3] == "never" ? nil : TimeInterval(arguments[3]) ?? 3) : 3
record = Record(dir: folder, delay: delay)
let app = FixtureApplication.shared as! FixtureApplication
app.answersManual = mode != "native"
let fixture = Fixture(mode: mode)
app.delegate = fixture
app.run()

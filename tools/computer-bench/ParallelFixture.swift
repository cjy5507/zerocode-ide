// A throwaway measuring window: never activates, sits behind everything,
// records every press to a file so a run is judged by the window's own record.
import AppKit

final class Flipped: NSView { override var isFlipped: Bool { true } }

final class Fixture: NSObject, NSApplicationDelegate {
    let count: Int, dir: URL, title: String
    var window: NSWindow!
    let doc = Flipped()
    var rows: [(id: Int, label: NSTextField, button: NSButton)] = []
    var head: [NSView] = []
    var submit: NSButton!
    var flaky: NSButton!
    var nextId = 0
    var applied = 0

    init(count: Int, dir: URL, title: String) { self.count = count; self.dir = dir; self.title = title }

    func log(_ line: String) {
        let url = dir.appendingPathComponent("events.log")
        let data = Data((line + "\n").utf8)
        if let handle = try? FileHandle(forWritingTo: url) {
            handle.seekToEndOfFile(); handle.write(data); try? handle.close()
        } else { try? data.write(to: url) }
    }

    func makeRow() -> (id: Int, label: NSTextField, button: NSButton) {
        let id = nextId; nextId += 1
        let label = NSTextField(labelWithString: "Row \(id)")
        let button = NSButton(title: "Delete", target: self, action: #selector(delete(_:)))
        button.tag = id
        doc.addSubview(label); doc.addSubview(button)
        return (id, label, button)
    }

    @objc func delete(_ sender: NSButton) { log("delete \(sender.tag)") }
    @objc func target(_ sender: NSButton) { log("target") }
    @objc func flakyPressed(_ sender: NSButton) { log("flaky") }
    @objc func submitted(_ sender: NSButton) { log("submit") }

    func relayout() {
        var y: CGFloat = 8
        for view in head { view.frame = NSRect(x: 8, y: y, width: 200, height: 24); y += 28 }
        for row in rows {
            row.label.frame = NSRect(x: 8, y: y + 3, width: 100, height: 18)
            row.button.frame = NSRect(x: 120, y: y, width: 90, height: 24)
            y += 28
        }
        submit.frame = NSRect(x: 8, y: y, width: 100, height: 24); y += 32
        doc.frame = NSRect(x: 0, y: 0, width: 440, height: y)
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        NSApp.setActivationPolicy(.accessory)
        let targetButton = NSButton(title: "Target", target: self, action: #selector(target(_:)))
        flaky = NSButton(title: "Flaky", target: self, action: #selector(flakyPressed(_:)))
        flaky.isEnabled = false
        let field = NSTextField(string: "")
        field.placeholderString = "Field"
        head = [targetButton, flaky, field]
        head.forEach { doc.addSubview($0) }
        for _ in 0..<count { rows.append(makeRow()) }
        submit = NSButton(title: "Submit", target: self, action: #selector(submitted(_:)))
        doc.addSubview(submit)
        relayout()
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 460, height: 420))
        scroll.hasVerticalScroller = true
        scroll.documentView = doc
        let screen = NSScreen.screens.first?.frame ?? NSRect(x: 0, y: 0, width: 1200, height: 800)
        window = NSWindow(contentRect: NSRect(x: screen.maxX - 480, y: screen.minY + 40, width: 460, height: 420),
                          styleMask: [.titled, .resizable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.contentView = scroll
        window.orderBack(nil)
        try? "ready".write(to: dir.appendingPathComponent("ready"), atomically: true, encoding: .utf8)
        Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in self?.poll() }
    }

    func poll() {
        let ctl = dir.appendingPathComponent("ctl")
        guard let text = try? String(contentsOf: ctl, encoding: .utf8) else { return }
        try? FileManager.default.removeItem(at: ctl)
        let parts = text.trimmingCharacters(in: .whitespacesAndNewlines).split(separator: " ").map(String.init)
        switch parts.first {
        case "remove":
            let id = Int(parts[1])!
            if let at = rows.firstIndex(where: { $0.id == id }) {
                rows[at].label.removeFromSuperview(); rows[at].button.removeFromSuperview()
                rows.remove(at: at); relayout()
            }
        case "rows":
            for row in rows { row.label.removeFromSuperview(); row.button.removeFromSuperview() }
            rows = []
            for _ in 0..<Int(parts[1])! { rows.append(makeRow()) }
            relayout()
        case "insert-top":
            let first = rows.first
            let row = makeRow()
            if let first {
                doc.addSubview(row.label, positioned: .below, relativeTo: first.label)
                doc.addSubview(row.button, positioned: .below, relativeTo: first.label)
                doc.addSubview(row.label, positioned: .below, relativeTo: row.button)
            }
            rows.insert(row, at: 0); relayout()
        case "hang":
            applied += 1
            try? "\(applied)".write(to: dir.appendingPathComponent("ack"), atomically: true, encoding: .utf8)
            Thread.sleep(forTimeInterval: Double(parts[1])! / 1000)
            return
        case "enable-flaky":
            let ms = Double(parts[1])!
            DispatchQueue.main.asyncAfter(deadline: .now() + ms / 1000) { [weak self] in self?.flaky.isEnabled = true }
        case "activate":
            // Stands in for the app a person has in front.
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
        case "raise":
            // In front of everything without taking the focus: a picture holds this window alone.
            window.level = .floating
            window.orderFrontRegardless()
        case "quit":
            NSApp.terminate(nil)
        default: break
        }
        applied += 1
        try? "\(applied)".write(to: dir.appendingPathComponent("ack"), atomically: true, encoding: .utf8)
    }
}

let args = CommandLine.arguments
let app = NSApplication.shared
let delegate = Fixture(count: Int(args[1])!, dir: URL(fileURLWithPath: args[2]), title: args[3])
app.delegate = delegate
app.run()

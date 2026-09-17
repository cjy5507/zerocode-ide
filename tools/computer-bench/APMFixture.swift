// Standalone, disposable AppKit target. No user document, account, clipboard,
// terminal or browser access. Only button handlers can advance the oracle.
import AppKit

final class Fixture: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    let label = NSTextField(labelWithString: "")
    let statePath: String
    let owner: String
    let random: Bool
    var next = "Amber"
    var events: [[String: Any]] = []
    var count = 0
    var errors = 0

    init(path: String, owner: String, random: Bool) {
        self.statePath = path
        self.owner = owner
        self.random = random
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 160, y: 160, width: 540, height: 280),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Owned APM fixture \(owner)"
        window.isReleasedWhenClosed = false
        let note = NSTextField(labelWithString: "Disposable Computer Use bench • \(random ? "random next target" : "alternating targets")")
        note.frame = NSRect(x: 24, y: 225, width: 490, height: 26)
        window.contentView!.addSubview(note)
        label.font = .monospacedSystemFont(ofSize: 21, weight: .medium)
        label.frame = NSRect(x: 24, y: 165, width: 490, height: 40)
        window.contentView!.addSubview(label)
        for (i, title) in ["Amber", "Blue"].enumerated() {
            let button = NSButton(title: title, target: self, action: #selector(press(_:)))
            button.bezelStyle = .rounded
            button.frame = NSRect(x: 50 + i * 245, y: 55, width: 190, height: 72)
            window.contentView!.addSubview(button)
        }
        publish()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc func press(_ sender: NSButton) {
        let expected = next
        let correct = sender.title == expected
        if correct {
            count += 1
            next = random ? (Bool.random() ? "Amber" : "Blue") : (expected == "Amber" ? "Blue" : "Amber")
        } else {
            errors += 1
        }
        events.append(["ordinal": events.count + 1, "label": sender.title,
                       "expected": expected, "correct": correct,
                       "uptime_ns": DispatchTime.now().uptimeNanoseconds])
        publish()
    }

    func publish() {
        label.stringValue = "Next: \(next) | Count: \(count) | Errors: \(errors)"
        let state: [String: Any] = ["owner": owner, "pid": ProcessInfo.processInfo.processIdentifier,
                                  "mode": random ? "random" : "alternate", "next": next,
                                  "count": count, "errors": errors, "events": events]
        do {
            let bytes = try JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
            try bytes.write(to: URL(fileURLWithPath: statePath), options: .atomic)
        } catch {
            fputs("fixture oracle could not write: \(error)\n", stderr)
            NSApp.terminate(nil)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

guard CommandLine.arguments.count == 4 else { fatalError("state-path owner alternate|random required") }
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let fixture = Fixture(path: CommandLine.arguments[1], owner: CommandLine.arguments[2],
                      random: CommandLine.arguments[3] == "random")
app.delegate = fixture
app.run()

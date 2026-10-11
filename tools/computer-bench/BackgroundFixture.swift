import AppKit
import CoreGraphics

@MainActor
final class BackgroundFixtureWindow: NSWindow {
    var received: ((NSEvent) -> Void)?

    override func sendEvent(_ event: NSEvent) {
        super.sendEvent(event)
        received?(event)
    }
}

@MainActor
final class BackgroundFixture: NSObject, NSApplicationDelegate {
    let output: URL
    let showWindows: Bool
    var windows: [BackgroundFixtureWindow] = []
    var inputs: [NSTextField] = []
    var buttons: [NSButton] = []
    var scrollers: [NSScrollView] = []
    var counts = [0, 0]
    var events: [[String: Any]] = []
    var timer: Timer?
    var activationCount = 0
    let pointerObserver = BackgroundPointerObserver()

    init(output: URL, showWindows: Bool) {
        self.output = output
        self.showWindows = showWindows
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        pointerObserver.start()
        let menu = NSMenu()
        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let item = NSMenuItem()
        item.submenu = edit
        menu.addItem(item)
        NSApp.mainMenu = menu
        for index in counts.indices {
            let window = BackgroundFixtureWindow(
                contentRect: CGRect(x: 40 + index * 420, y: 100, width: 400, height: 380),
                styleMask: [.titled, .closable], backing: .buffered, defer: false
            )
            window.title = "Background fixture \(index + 1)"
            window.isReleasedWhenClosed = false
            let content = NSView(frame: CGRect(x: 0, y: 0, width: 400, height: 380))
            let button = NSButton(title: "Increment \(index + 1)", target: self, action: #selector(increment(_:)))
            button.tag = index
            button.frame = CGRect(x: 40, y: 320, width: 180, height: 40)
            button.keyEquivalent = "\r"
            let input = NSTextField(frame: CGRect(x: 40, y: 270, width: 300, height: 32))
            input.placeholderString = "Owned test field"
            let scroll = NSScrollView(frame: CGRect(x: 40, y: 20, width: 300, height: 220))
            scroll.hasVerticalScroller = true
            let document = NSTextView(frame: CGRect(x: 0, y: 0, width: 280, height: 2400))
            document.isEditable = false
            document.string = (1...100).map { "Fixture row \($0)" }.joined(separator: "\n")
            scroll.documentView = document
            content.addSubview(button)
            content.addSubview(input)
            content.addSubview(scroll)
            window.contentView = content
            window.defaultButtonCell = button.cell as? NSButtonCell
            window.makeFirstResponder(input)
            window.received = { [weak self] event in self?.record(event, index: index) }
            windows.append(window)
            buttons.append(button)
            inputs.append(input)
            scrollers.append(scroll)
            if showWindows { window.orderFrontRegardless() } else { window.orderBack(nil) }
        }
        write()
        timer = Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.write() }
        }
    }

    @objc func increment(_ sender: NSButton) {
        counts[sender.tag] += 1
        write()
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        activationCount += 1
        write()
    }

    func applicationWillTerminate(_ notification: Notification) {
        pointerObserver.stop()
    }

    func record(_ event: NSEvent, index: Int) {
        guard let raw = event.cgEvent, raw.getIntegerValueField(.eventSourceUserData) != 0 else { return }
        if events.count == 64 { events.removeFirst() }
        events.append([
            "windowIndex": index, "windowNumber": event.windowNumber, "type": event.type.rawValue,
            "localX": event.locationInWindow.x, "localY": event.locationInWindow.y,
            "source": raw.getIntegerValueField(.eventSourceStateID),
            "sourcePid": raw.getIntegerValueField(.eventSourceUnixProcessID),
            "targetPid": raw.getIntegerValueField(.eventTargetUnixProcessID),
            "flags": event.modifierFlags.rawValue,
            "physicalLeftDown": CGEventSource.buttonState(.hidSystemState, button: .left),
            "tag": raw.getIntegerValueField(.eventSourceUserData),
            "active": NSApp.isActive, "activationCount": activationCount,
            "clipboardChanges": NSPasteboard.general.changeCount,
        ])
        write()
    }

    func write() {
        let height = CGDisplayBounds(CGMainDisplayID()).height
        let rows = windows.enumerated().map { index, window -> [String: Any] in
            @MainActor func point(_ view: NSView) -> CGPoint {
                let cocoa = window.convertPoint(toScreen: view.convert(CGPoint(x: view.bounds.midX, y: view.bounds.midY), to: nil))
                return CGPoint(x: cocoa.x, y: height - cocoa.y)
            }
            let button = point(buttons[index])
            let input = point(inputs[index])
            let scroll = point(scrollers[index])
            return [
                "id": window.windowNumber, "x": window.frame.minX, "y": height - window.frame.maxY,
                "width": window.frame.width, "height": window.frame.height,
                "buttonX": button.x, "buttonY": button.y, "inputX": input.x, "inputY": input.y,
                "scrollX": scroll.x, "scrollY": scroll.y, "scrollOffset": scrollers[index].contentView.bounds.minY,
                "clicks": counts[index], "text": inputs[index].stringValue, "visible": window.isVisible,
            ]
        }
        let cursor = CGEvent(source: nil)?.location
        pointerObserver.sample(cursor)
        let state: [String: Any] = [
            "pid": getpid(), "bundleId": Bundle.main.bundleIdentifier ?? "", "windows": rows, "events": events,
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
}

@main
struct BackgroundFixtureEntry {
    @MainActor
    static func main() {
        guard CommandLine.arguments.count == 2 || (CommandLine.arguments.count == 3 && CommandLine.arguments[2] == "--show-windows") else { exit(2) }
        let application = NSApplication.shared
        application.setActivationPolicy(.accessory)
        let fixture = BackgroundFixture(output: URL(fileURLWithPath: CommandLine.arguments[1]),
                                        showWindows: CommandLine.arguments.count == 3)
        application.delegate = fixture
        withExtendedLifetime(fixture) { application.run() }
    }
}

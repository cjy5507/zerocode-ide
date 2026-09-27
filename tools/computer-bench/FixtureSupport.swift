// Shared fixture infrastructure. The scene owns its stimulus and oracle;
// this file owns recording, a window around the resting pointer, and shutdown.
import AppKit

func uptimeNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

/// The fixture's record: lines appended to its files off the main thread, and
/// its state rewritten whole each time.
final class Recorder: @unchecked Sendable {
    let folder: URL
    private let queue = DispatchQueue(label: "reflex-fixture.recorder")
    private enum Stream: String, CaseIterable {
        case events = "events.jsonl", frames = "frames.jsonl", oracle = "oracle.jsonl"
    }
    private var streams: [Stream: [[String: Any]]] = [:]

    init(folder: URL) {
        self.folder = folder
    }

    func event(_ row: [String: Any]) { streams[.events, default: []].append(row) }
    func frame(_ row: [String: Any]) { streams[.frames, default: []].append(row) }
    func scene(_ row: [String: Any]) { streams[.oracle, default: []].append(row) }

    func write(_ name: String, _ object: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return }
        try? data.write(to: folder.appendingPathComponent(name), options: .atomic)
    }

    /// Append what came since the last flush, then the state: encoded here,
    /// written on the recorder's own queue.
    func flush(state: [String: Any], wait: Bool = false) {
        func lines(_ rows: [[String: Any]]) -> Data {
            var text = Data()
            for row in rows {
                guard let line = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) else { continue }
                text.append(line)
                text.append(0x0A)
            }
            return text
        }
        let appended = Stream.allCases.map { ($0.rawValue, lines(streams[$0] ?? [])) }
        streams.removeAll(keepingCapacity: true)
        let whole = (try? JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])) ?? Data()
        let folder = self.folder
        let work: @Sendable () -> Void = {
            for (name, text) in appended where !text.isEmpty {
                let url = folder.appendingPathComponent(name)
                if let handle = try? FileHandle(forWritingTo: url) {
                    handle.seekToEndOfFile()
                    handle.write(text)
                    try? handle.close()
                } else {
                    try? text.write(to: url)
                }
            }
            if !whole.isEmpty {
                try? whole.write(to: folder.appendingPathComponent("fixture.json"), options: .atomic)
            }
        }
        if wait { queue.sync(execute: work) } else { queue.async(execute: work) }
    }
}

/// One non-activating window around the resting pointer. Both fixtures report
/// the same geometry and ownership facts before a driver may start the hand.
@MainActor
struct FixtureWindow {
    let panel: NSPanel
    let ready: [String: Any]

    init(view: NSView, owner: String, seed: Int) {
        let size = view.bounds.size
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) } ?? NSScreen.main
        let visible = screen?.visibleFrame ?? CGRect(origin: .zero, size: size)
        let origin = CGPoint(x: min(max(mouse.x - size.width / 2, visible.minX), visible.maxX - size.width).rounded(.down),
                             y: min(max(mouse.y - size.height / 2, visible.minY), visible.maxY - size.height).rounded(.down))
        let frame = CGRect(origin: origin, size: size)
        panel = NSPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isFloatingPanel = false
        panel.level = .normal
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = true
        panel.acceptsMouseMovedEvents = true
        panel.isReleasedWhenClosed = false
        panel.colorSpace = .sRGB
        panel.contentView = view
        panel.orderFrontRegardless()
        let mainHeight = NSScreen.screens.first?.frame.height ?? size.height
        let quartz = CGRect(x: frame.minX, y: mainHeight - frame.maxY, width: frame.width, height: frame.height)
        ready = [
            "owner": owner, "seed": seed, "pid": Int(getpid()), "bundleId": Bundle.main.bundleIdentifier ?? "",
            "window": ["x": quartz.minX, "y": quartz.minY, "width": quartz.width, "height": quartz.height],
            "pointer": ["x": mouse.x, "y": mainHeight - mouse.y], "pointerInside": NSMouseInRect(mouse, frame, false),
            "readyNs": uptimeNs(), "events": 0, "hits": 0, "misses": 0, "active": NSApp.isActive,
        ]
    }
}

/// The fixture's recording cadence and signal flush. A prepared app carries
/// bench.json as a resource, so the scene's round bytes need no new field.
@MainActor
final class FixtureLifetime {
    private let activity: NSObjectProtocol
    private var flushTimer: Timer?
    private var observer: NSObjectProtocol?
    private var signals: [DispatchSourceSignal] = []

    init(flush: @escaping @MainActor (Bool) -> Void, becameActive: @escaping @MainActor () -> Void) {
        activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .latencyCritical], reason: "a computer input bench round")
        observer = NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil,
                                                         queue: .main) { _ in MainActor.assumeIsolated { becameActive() } }
        guard let url = Bundle.main.url(forResource: "bench", withExtension: "json"),
              let data = try? Data(contentsOf: url), let table = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let row = table["fixture_runtime"] as? [String: Any], let value = row["value"] as? [String: Any],
              let interval = value["flush_ms"] as? Double, interval > 0
        else {
            FileHandle.standardError.write(Data("the prepared fixture has no runtime table\n".utf8))
            exit(2)
        }
        flushTimer = Timer.scheduledTimer(withTimeInterval: interval / 1_000, repeats: true) { _ in
            MainActor.assumeIsolated { flush(false) }
        }
        for number in [SIGTERM, SIGINT] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { MainActor.assumeIsolated { flush(true); exit(0) } }
            source.resume()
            signals.append(source)
        }
    }
}

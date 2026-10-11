import AppKit
import CoreGraphics

@MainActor
final class BackgroundPointerObserver {
    private var monitor: Any?
    private var sources: [String: UInt64] = [:]
    private var mouseMoves: UInt64 = 0
    private var cursorChanges: UInt64 = 0
    private var previousCursor: CGPoint?
    private var cursorReadable = false
    private var overflow = false

    func start() {
        guard monitor == nil else { return }
        monitor = NSEvent.addGlobalMonitorForEvents(matching: [
            .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged,
            .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp,
            .otherMouseDown, .otherMouseUp, .scrollWheel,
        ]) { [weak self] event in
            MainActor.assumeIsolated { self?.record(event) }
        }
    }

    func stop() {
        if let monitor { NSEvent.removeMonitor(monitor) }
        monitor = nil
    }

    var snapshot: [String: Any] {
        ["ready": monitor != nil, "sources": sources, "mouseMoves": mouseMoves, "overflow": overflow,
         "cursorChanges": cursorChanges, "cursorReadable": cursorReadable]
    }

    func sample(_ cursor: CGPoint?) {
        guard let cursor, cursor.x.isFinite, cursor.y.isFinite else {
            cursorReadable = false
            return
        }
        cursorReadable = true
        if let previousCursor, previousCursor != cursor { cursorChanges += 1 }
        previousCursor = cursor
    }

    private func record(_ event: NSEvent) {
        guard let raw = event.cgEvent else {
            overflow = true
            return
        }
        let source = String(raw.getIntegerValueField(.eventSourceUnixProcessID))
        guard sources[source] != nil || sources.count < 64 else {
            overflow = true
            return
        }
        sources[source, default: 0] += 1
        switch event.type {
        case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
            mouseMoves += 1
        default:
            break
        }
    }
}

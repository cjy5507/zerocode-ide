import AppKit
import CoreGraphics
import ZeroCodeComputerUseMacOSCore

@MainActor
final class BackgroundPointerOverlay {
    private static let shared = BackgroundPointerOverlay()
    nonisolated private static let identityLock = NSLock()
    nonisolated(unsafe) private static var pointerWindowID: UInt32?
    nonisolated(unsafe) private static var pending: WindowPointerAnchor?
    nonisolated(unsafe) private static var latest: WindowPointerAnchor?
    nonisolated(unsafe) private static var displayed: WindowPointerAnchor?
    nonisolated(unsafe) private static var presentationQueued = false
    private var panel: PointerPanel?
    private var expiry: DispatchSourceTimer?
    private var activationObserver: NSObjectProtocol?
    private static let size = CGSize(width: 28, height: 34)

    private init() {
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.clear() }
        }
    }

    nonisolated static func owns(_ info: [String: Any]) -> Bool {
        guard info[kCGWindowOwnerPID as String] as? pid_t == getpid(),
              let windowID = info[kCGWindowNumber as String] as? UInt32 else { return false }
        identityLock.lock()
        defer { identityLock.unlock() }
        return pointerWindowID == windowID
    }

    nonisolated private static func windowID() -> UInt32? {
        identityLock.lock()
        defer { identityLock.unlock() }
        return pointerWindowID
    }

    nonisolated static func status() -> [String: Any] {
        identityLock.lock()
        let accepted = latest
        let shown = displayed
        let queued = presentationQueued
        identityLock.unlock()
        func described(_ anchor: WindowPointerAnchor?) -> Any {
            guard let anchor else { return NSNull() }
            return ["pid": Int(anchor.pid), "windowId": Int(anchor.windowID),
                    "x": anchor.point.x - anchor.bounds.minX, "y": anchor.point.y - anchor.bounds.minY,
                    "coordinateSpace": "window"] as [String: Any]
        }
        return ["latest": described(accepted), "displayed": described(shown), "pending": queued]
    }

    nonisolated static func show(pid: pid_t, windowID: UInt32, bounds: CGRect, point: CGPoint) {
        guard let anchor = WindowPointerAnchor(pid: pid, windowID: windowID, bounds: bounds, point: point) else { return }
        identityLock.lock()
        pending = anchor
        latest = anchor
        let enqueue = !presentationQueued
        presentationQueued = true
        identityLock.unlock()
        if enqueue {
            DispatchQueue.main.async {
                identityLock.lock()
                let latest = pending
                pending = nil
                presentationQueued = false
                identityLock.unlock()
                if let latest { shared.present(latest) }
            }
        }
    }

    nonisolated static func hide() {
        identityLock.lock()
        pending = nil
        identityLock.unlock()
        DispatchQueue.main.async { shared.clear() }
    }

    private func present(_ anchor: WindowPointerAnchor) {
        let displayHeight = CGDisplayBounds(CGMainDisplayID()).height
        let cocoaPoint = CGPoint(x: anchor.point.x, y: displayHeight - anchor.point.y)
        guard !OperatorGuardHost.isStopped(),
              NSWorkspace.shared.frontmostApplication?.processIdentifier != anchor.pid,
              NSScreen.screens.contains(where: { $0.frame.contains(cocoaPoint) }),
              DesktopSelf.ownerPid(at: anchor.point) == anchor.pid,
              anchor.visible(in: Self.windows(), ignoringWindowID: Self.windowID()),
              let frame = anchor.panelFrame(mainDisplayHeight: displayHeight, size: Self.size)
        else {
            clear()
            return
        }
        let shown: PointerPanel
        if let panel {
            shown = panel
        } else {
            shown = PointerPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel],
                                 backing: .buffered, defer: false)
            shown.isOpaque = false
            shown.backgroundColor = .clear
            shown.hasShadow = false
            shown.ignoresMouseEvents = true
            shown.hidesOnDeactivate = false
            shown.isReleasedWhenClosed = false
            shown.level = .floating
            shown.collectionBehavior = [.transient, .ignoresCycle, .fullScreenAuxiliary]
            shown.contentView = PointerView(frame: CGRect(origin: .zero, size: Self.size))
            shown.setAccessibilityElement(false)
            panel = shown
        }
        guard let windowID = UInt32(exactly: shown.windowNumber), windowID > 0 else { return }
        Self.identityLock.lock()
        Self.pointerWindowID = windowID
        Self.identityLock.unlock()
        shown.setFrame(frame, display: true)
        shown.orderFrontRegardless()
        Self.identityLock.lock()
        Self.displayed = shown.isVisible ? anchor : nil
        Self.identityLock.unlock()
        if let expiry {
            expiry.schedule(deadline: .now() + .seconds(1))
        } else {
            let expiry = DispatchSource.makeTimerSource(queue: .main)
            expiry.setEventHandler { [weak self] in
                MainActor.assumeIsolated { self?.clear() }
            }
            expiry.schedule(deadline: .now() + .seconds(1))
            expiry.resume()
            self.expiry = expiry
        }
    }

    private func clear() {
        expiry?.cancel()
        expiry = nil
        panel?.orderOut(nil)
        Self.identityLock.lock()
        Self.displayed = nil
        Self.identityLock.unlock()
    }

    private static func windows() -> [WindowPointerAnchor.Window] {
        guard let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { return [] }
        let displays = DesktopScreen.displays().map(\.bounds)
        return rows.compactMap { row in
            guard let pid = row[kCGWindowOwnerPID as String] as? NSNumber,
                  let window = row[kCGWindowNumber as String] as? NSNumber,
                  let layer = row[kCGWindowLayer as String] as? Int, layer >= 0,
                  let raw = row[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: raw),
                  let alpha = row[kCGWindowAlpha as String] as? NSNumber else { return nil }
            guard !DesktopOverlay.isOverlay(layer: layer, bounds: bounds, displays: displays),
                  !DesktopPointerPicture.isPointerPicture(layer: layer,
                      ownerName: row[kCGWindowOwnerName as String] as? String ?? "", bounds: bounds) else { return nil }
            return WindowPointerAnchor.Window(pid: pid.int32Value, id: window.uint32Value, bounds: bounds, alpha: alpha.doubleValue)
        }
    }
}

@MainActor
private final class PointerPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

@MainActor
private final class PointerView: NSView {
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        let arrow = NSBezierPath()
        arrow.move(to: CGPoint(x: 1, y: 1))
        for vertex in [CGPoint(x: 23, y: 18), CGPoint(x: 14, y: 20), CGPoint(x: 10, y: 30)] {
            arrow.line(to: vertex)
        }
        arrow.close()
        NSColor.systemPurple.setFill()
        arrow.fill()
        NSColor.white.setStroke()
        arrow.lineWidth = 1.5
        arrow.stroke()
    }
}

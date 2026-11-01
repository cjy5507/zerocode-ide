import AppKit
import CoreGraphics
import ZeroCodeComputerUseMacOSCore

@MainActor
final class BackgroundPointerOverlay {
    /// One agent's pointer: where it goes and what draws it. Several agents work
    /// at once, so each has its own and they move at the same time.
    private final class Slot {
        var panel: PointerPanel?
        var expiry: DispatchSourceTimer?
    }

    private static let shared = BackgroundPointerOverlay()
    nonisolated private static let identityLock = NSLock()
    nonisolated(unsafe) private static var pointerWindowIDs: Set<UInt32> = []
    nonisolated(unsafe) private static var pending: [String: WindowPointerAnchor] = [:]
    nonisolated(unsafe) private static var latest: WindowPointerAnchor?
    nonisolated(unsafe) private static var displayed: [String: WindowPointerAnchor] = [:]
    nonisolated(unsafe) private static var presentationQueued = false
    private var slots: [String: Slot] = [:]
    private var activationObserver: NSObjectProtocol?
    private static let arrowSize = CGSize(width: 28, height: 34)
    /// More pointers than this tell a person nothing: the oldest gives way.
    private static let maxPointers = 8
    nonisolated private static let unnamedAgent = ""

    private init() {
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.clearAll() }
        }
    }

    nonisolated static func owns(_ info: [String: Any]) -> Bool {
        guard info[kCGWindowOwnerPID as String] as? pid_t == getpid(),
              let windowID = info[kCGWindowNumber as String] as? UInt32 else { return false }
        identityLock.lock()
        defer { identityLock.unlock() }
        return pointerWindowIDs.contains(windowID)
    }

    nonisolated private static func windowIDs() -> Set<UInt32> {
        identityLock.lock()
        defer { identityLock.unlock() }
        return pointerWindowIDs
    }

    nonisolated static func status() -> [String: Any] {
        identityLock.lock()
        let accepted = latest
        let shown = displayed.values.first
        let count = displayed.count
        let queued = presentationQueued
        identityLock.unlock()
        func described(_ anchor: WindowPointerAnchor?) -> Any {
            guard let anchor else { return NSNull() }
            return ["pid": Int(anchor.pid), "windowId": Int(anchor.windowID),
                    "x": anchor.point.x - anchor.bounds.minX, "y": anchor.point.y - anchor.bounds.minY,
                    "coordinateSpace": "window"] as [String: Any]
        }
        return ["latest": described(accepted), "displayed": described(shown), "pointers": count, "pending": queued]
    }

    nonisolated static func show(pid: pid_t, windowID: UInt32, bounds: CGRect, point: CGPoint, agent: String? = nil) {
        guard let anchor = WindowPointerAnchor(pid: pid, windowID: windowID, bounds: bounds, point: point) else { return }
        identityLock.lock()
        pending[agent ?? unnamedAgent] = anchor
        latest = anchor
        let enqueue = !presentationQueued
        presentationQueued = true
        identityLock.unlock()
        if enqueue {
            DispatchQueue.main.async {
                identityLock.lock()
                let waiting = pending
                pending = [:]
                presentationQueued = false
                identityLock.unlock()
                for (agent, anchor) in waiting { shared.present(anchor, agent: agent) }
            }
        }
    }

    nonisolated static func hide() {
        identityLock.lock()
        pending = [:]
        identityLock.unlock()
        DispatchQueue.main.async { shared.clearAll() }
    }

    private func present(_ anchor: WindowPointerAnchor, agent: String) {
        let displayHeight = CGDisplayBounds(CGMainDisplayID()).height
        let cocoaPoint = CGPoint(x: anchor.point.x, y: displayHeight - anchor.point.y)
        let size = PointerView.size(arrow: Self.arrowSize, agent: agent)
        guard !OperatorGuardHost.isStopped(),
              NSWorkspace.shared.frontmostApplication?.processIdentifier != anchor.pid,
              NSScreen.screens.contains(where: { $0.frame.contains(cocoaPoint) }),
              DesktopSelf.ownerPid(at: anchor.point) == anchor.pid,
              anchor.visible(in: Self.windows(), ignoringWindowIDs: Self.windowIDs()),
              let frame = anchor.panelFrame(mainDisplayHeight: displayHeight, size: size)
        else {
            clear(agent: agent)
            return
        }
        if slots[agent] == nil, slots.count >= Self.maxPointers, let oldest = slots.keys.first {
            clear(agent: oldest)
            slots[oldest] = nil
        }
        let slot = slots[agent] ?? Slot()
        slots[agent] = slot
        let shown: PointerPanel
        if let panel = slot.panel {
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
            shown.contentView = PointerView(frame: CGRect(origin: .zero, size: size), agent: agent)
            shown.setAccessibilityElement(false)
            slot.panel = shown
        }
        guard let windowID = UInt32(exactly: shown.windowNumber), windowID > 0 else { return }
        Self.identityLock.lock()
        Self.pointerWindowIDs.insert(windowID)
        Self.identityLock.unlock()
        shown.setFrame(frame, display: true)
        shown.orderFrontRegardless()
        Self.identityLock.lock()
        Self.displayed[agent] = shown.isVisible ? anchor : nil
        Self.identityLock.unlock()
        if let expiry = slot.expiry {
            expiry.schedule(deadline: .now() + .seconds(1))
        } else {
            let expiry = DispatchSource.makeTimerSource(queue: .main)
            expiry.setEventHandler { [weak self] in
                MainActor.assumeIsolated { self?.clear(agent: agent) }
            }
            expiry.schedule(deadline: .now() + .seconds(1))
            expiry.resume()
            slot.expiry = expiry
        }
    }

    private func clear(agent: String) {
        guard let slot = slots[agent] else { return }
        slot.expiry?.cancel()
        slot.expiry = nil
        slot.panel?.orderOut(nil)
        Self.identityLock.lock()
        Self.displayed[agent] = nil
        Self.identityLock.unlock()
    }

    private func clearAll() {
        for agent in Array(slots.keys) { clear(agent: agent) }
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
    private let agent: String
    /// The colours agents are told apart by: the same agent keeps its colour.
    private static let palette: [NSColor] = [
        .systemPurple, .systemBlue, .systemTeal, .systemGreen, .systemOrange, .systemPink,
    ]
    private static var labelFont: NSFont { .systemFont(ofSize: 11, weight: .semibold) }
    private static let labelPadding = CGSize(width: 6, height: 2)
    private static let labelOrigin = CGPoint(x: 14, y: 20)
    private static let maxLabelCharacters = 24

    init(frame: CGRect, agent: String) {
        self.agent = String(agent.prefix(Self.maxLabelCharacters))
        super.init(frame: frame)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }

    /// Room for the arrow, and for the agent's name under it when it has one.
    static func size(arrow: CGSize, agent: String) -> CGSize {
        guard !agent.isEmpty else { return arrow }
        let text = (String(agent.prefix(maxLabelCharacters)) as NSString).size(withAttributes: [.font: labelFont])
        return CGSize(
            width: max(arrow.width, labelOrigin.x + text.width + labelPadding.width * 2),
            height: max(arrow.height, labelOrigin.y + text.height + labelPadding.height * 2)
        )
    }

    private var color: NSColor {
        let hash = agent.unicodeScalars.reduce(UInt32(2_166_136_261)) { ($0 ^ $1.value) &* 16_777_619 }
        return Self.palette[Int(hash) % Self.palette.count]
    }

    override func draw(_ dirtyRect: NSRect) {
        let arrow = NSBezierPath()
        arrow.move(to: CGPoint(x: 1, y: 1))
        for vertex in [CGPoint(x: 23, y: 18), CGPoint(x: 14, y: 20), CGPoint(x: 10, y: 30)] {
            arrow.line(to: vertex)
        }
        arrow.close()
        color.setFill()
        arrow.fill()
        NSColor.white.setStroke()
        arrow.lineWidth = 1.5
        arrow.stroke()
        guard !agent.isEmpty else { return }
        let attributes: [NSAttributedString.Key: Any] = [.font: Self.labelFont, .foregroundColor: NSColor.white]
        let text = (agent as NSString).size(withAttributes: attributes)
        let pill = CGRect(
            x: Self.labelOrigin.x,
            y: Self.labelOrigin.y,
            width: text.width + Self.labelPadding.width * 2,
            height: text.height + Self.labelPadding.height * 2
        )
        color.setFill()
        NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
        (agent as NSString).draw(
            at: CGPoint(x: pill.minX + Self.labelPadding.width, y: pill.minY + Self.labelPadding.height),
            withAttributes: attributes
        )
    }
}

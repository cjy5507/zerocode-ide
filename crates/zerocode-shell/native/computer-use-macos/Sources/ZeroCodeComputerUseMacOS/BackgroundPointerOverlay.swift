import AppKit
import CoreGraphics
import QuartzCore
import ZeroCodeComputerUseMacOSCore

@MainActor
final class BackgroundPointerOverlay {
    /// One agent's pointer: where it goes and what draws it. Several agents work
    /// at once, so each has its own and they move at the same time.
    private final class Slot {
        var panel: PointerPanel?
        var expiry: DispatchSourceTimer?
        /// The move under way, stepped on the main queue; a new act takes over
        /// from wherever it has got to.
        var glide: DispatchSourceTimer?
    }

    private static let shared = BackgroundPointerOverlay()
    nonisolated private static let identityLock = NSLock()
    nonisolated(unsafe) private static var pointerWindowIDs: Set<UInt32> = []
    nonisolated(unsafe) private static var pending: [String: (anchor: WindowPointerAnchor, pressed: Bool)] = [:]
    nonisolated(unsafe) private static var latest: WindowPointerAnchor?
    nonisolated(unsafe) private static var displayed: [String: WindowPointerAnchor] = [:]
    nonisolated(unsafe) private static var presentationQueued = false
    private var slots: [String: Slot] = [:]
    /// The press rings still spreading, so a stop takes them away too.
    private var ripples: [NSPanel] = []
    private static let arrowSize = CGSize(width: 28, height: 34)
    /// How long a pointer stays after its agent's last act: through the model's
    /// turn between one act and the next (8-17 s a round trip), so it moves on
    /// from where it was instead of blinking out and in again.
    private static let staysSeconds = 15
    /// Steps a second of a move: the display's own rate. The act does not wait
    /// for the move; its length is `SmoothPointerPath.glideSeconds`.
    private static let glideStepsPerSecond = 60.0
    /// How far the arc of a move stands off the straight line, as a share of
    /// its length; each agent keeps its own side.
    private static let glideBow = 0.12
    /// A press: a ring that spreads from the point once the pointer is there.
    private static let rippleSize = CGSize(width: 56, height: 56)
    private static let rippleSeconds: TimeInterval = 0.4
    /// Where it first comes from, so its first act is seen as movement too.
    private static let firstApproach = CGSize(width: -60, height: 60)
    /// Over a window the person has covered it shows the agent is there, without
    /// reading as something happening in what they are looking at.
    private static let coveredAlpha: CGFloat = 0.55
    /// More pointers than this tell a person nothing: the oldest gives way.
    private static let maxPointers = 8
    nonisolated private static let unnamedAgent = ""

    private init() {}

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

    /// `pressed`: the act was a press that went through, so a ring spreads where
    /// it landed. A move, a scroll or keys show the pointer only.
    nonisolated static func show(pid: pid_t, windowID: UInt32, bounds: CGRect, point: CGPoint, agent: String? = nil, pressed: Bool = false) {
        guard let anchor = WindowPointerAnchor(pid: pid, windowID: windowID, bounds: bounds, point: point) else { return }
        identityLock.lock()
        pending[agent ?? unnamedAgent] = (anchor, pressed)
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
                for (agent, waited) in waiting { shared.present(waited.anchor, agent: agent, pressed: waited.pressed) }
            }
        }
    }

    nonisolated static func hide() {
        identityLock.lock()
        pending = [:]
        identityLock.unlock()
        DispatchQueue.main.async { shared.clearAll() }
    }

    private func present(_ anchor: WindowPointerAnchor, agent: String, pressed: Bool) {
        let displayHeight = CGDisplayBounds(CGMainDisplayID()).height
        let cocoaPoint = CGPoint(x: anchor.point.x, y: displayHeight - anchor.point.y)
        let size = PointerView.size(arrow: Self.arrowSize, agent: agent)
        guard !OperatorGuardHost.isStopped(),
              NSScreen.screens.contains(where: { $0.frame.contains(cocoaPoint) }),
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
            let arrivesFrom = frame.offsetBy(dx: Self.firstApproach.width, dy: Self.firstApproach.height)
            shown = PointerPanel(contentRect: arrivesFrom, styleMask: [.borderless, .nonactivatingPanel],
                                 backing: .buffered, defer: false)
            shown.isOpaque = false
            shown.backgroundColor = .clear
            shown.hasShadow = false
            shown.ignoresMouseEvents = true
            shown.hidesOnDeactivate = false
            shown.isReleasedWhenClosed = false
            shown.level = .floating
            // The person's to see, and no part of what an agent reads off the screen.
            shown.sharingType = .none
            shown.collectionBehavior = [.transient, .ignoresCycle, .fullScreenAuxiliary, .canJoinAllSpaces]
            shown.contentView = PointerView(frame: CGRect(origin: .zero, size: size), agent: agent)
            shown.setAccessibilityElement(false)
            slot.panel = shown
        }
        guard let windowID = UInt32(exactly: shown.windowNumber), windowID > 0 else { return }
        Self.identityLock.lock()
        Self.pointerWindowIDs.insert(windowID)
        Self.identityLock.unlock()
        let inView = DesktopSelf.ownerPid(at: anchor.point) == anchor.pid
            && anchor.visible(in: Self.windows(), ignoringWindowIDs: Self.windowIDs())
        shown.alphaValue = inView ? 1 : Self.coveredAlpha
        // A pointer shown again moves on from where it last was.
        let from = shown.frame.origin
        shown.setContentSize(frame.size)
        shown.orderFrontRegardless()
        glide(slot, panel: shown, from: from, to: frame.origin, agent: agent) { [weak self] in
            if pressed { self?.ripple(at: cocoaPoint, agent: agent, faint: !inView) }
        }
        Self.identityLock.lock()
        Self.displayed[agent] = shown.isVisible ? anchor : nil
        Self.identityLock.unlock()
        if let expiry = slot.expiry {
            expiry.schedule(deadline: .now() + .seconds(Self.staysSeconds))
        } else {
            let expiry = DispatchSource.makeTimerSource(queue: .main)
            expiry.setEventHandler { [weak self] in
                MainActor.assumeIsolated { self?.clear(agent: agent) }
            }
            expiry.schedule(deadline: .now() + .seconds(Self.staysSeconds))
            expiry.resume()
            slot.expiry = expiry
        }
    }

    /// Steps the panel along a gentle arc to `to`, eased like a hand, in the time
    /// a person can follow (`SmoothPointerPath`); `arrived` runs once it is there.
    private func glide(_ slot: Slot, panel: NSPanel, from: CGPoint, to: CGPoint, agent: String, arrived: @escaping () -> Void) {
        slot.glide?.cancel()
        slot.glide = nil
        let distance = hypot(to.x - from.x, to.y - from.y)
        let seconds = SmoothPointerPath.glideSeconds(distance: distance)
        let steps = max(1, Int((seconds * Self.glideStepsPerSecond).rounded()))
        let side = PointerView.hash(agent) % 2 == 0 ? 1.0 : -1.0
        var path = SmoothPointerPath.points(
            from: .init(x: from.x, y: from.y), to: .init(x: to.x, y: to.y),
            steps: steps, bow: Self.glideBow * side
        )[...]
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.setEventHandler { [weak self, weak slot] in
            MainActor.assumeIsolated {
                guard let point = path.popFirst() else { return }
                panel.setFrameOrigin(CGPoint(x: point.x, y: point.y))
                guard path.isEmpty else { return }
                slot?.glide?.cancel()
                slot?.glide = nil
                if self != nil { arrived() }
            }
        }
        timer.schedule(deadline: .now(), repeating: seconds / Double(steps))
        slot.glide = timer
        timer.resume()
    }

    /// A ring spreading from where a press landed: the agent's colour, gone in
    /// `rippleSeconds`, and like the pointer no part of any screenshot.
    private func ripple(at point: CGPoint, agent: String, faint: Bool) {
        guard !OperatorGuardHost.isStopped() else { return }
        let size = Self.rippleSize
        let panel = PointerPanel(
            contentRect: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false
        )
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.ignoresMouseEvents = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.level = .floating
        panel.sharingType = .none
        panel.collectionBehavior = [.transient, .ignoresCycle, .fullScreenAuxiliary, .canJoinAllSpaces]
        panel.alphaValue = faint ? Self.coveredAlpha : 1
        panel.setAccessibilityElement(false)
        let view = NSView(frame: CGRect(origin: .zero, size: size))
        view.wantsLayer = true
        let ring = CAShapeLayer()
        ring.frame = view.bounds
        ring.path = CGPath(ellipseIn: view.bounds.insetBy(dx: 3, dy: 3), transform: nil)
        ring.fillColor = PointerView.color(for: agent).withAlphaComponent(0.18).cgColor
        ring.strokeColor = PointerView.color(for: agent).cgColor
        ring.lineWidth = 2.5
        view.layer?.addSublayer(ring)
        panel.contentView = view
        panel.orderFrontRegardless()
        if let windowID = UInt32(exactly: panel.windowNumber), windowID > 0 {
            Self.identityLock.lock()
            Self.pointerWindowIDs.insert(windowID)
            Self.identityLock.unlock()
        }
        ripples.append(panel)
        let grow = CABasicAnimation(keyPath: "transform.scale")
        grow.fromValue = 0.25
        grow.toValue = 1.0
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 0.95
        fade.toValue = 0.0
        let both = CAAnimationGroup()
        both.animations = [grow, fade]
        both.duration = Self.rippleSeconds
        both.timingFunction = CAMediaTimingFunction(name: .easeOut)
        ring.opacity = 0
        ring.add(both, forKey: "press")
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.rippleSeconds + 0.05) { [weak self] in
            MainActor.assumeIsolated { self?.dropRipple(panel) }
        }
    }

    private func dropRipple(_ panel: NSPanel) {
        panel.orderOut(nil)
        ripples.removeAll { $0 === panel }
        if let windowID = UInt32(exactly: panel.windowNumber), windowID > 0 {
            Self.identityLock.lock()
            Self.pointerWindowIDs.remove(windowID)
            Self.identityLock.unlock()
        }
    }

    private func clear(agent: String) {
        guard let slot = slots[agent] else { return }
        slot.expiry?.cancel()
        slot.expiry = nil
        slot.glide?.cancel()
        slot.glide = nil
        slot.panel?.orderOut(nil)
        Self.identityLock.lock()
        Self.displayed[agent] = nil
        Self.identityLock.unlock()
    }

    private func clearAll() {
        for agent in Array(slots.keys) { clear(agent: agent) }
        for panel in ripples { dropRipple(panel) }
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

    /// The same agent's name gives the same number: its colour and the side its
    /// moves bow to.
    static func hash(_ agent: String) -> Int {
        Int(agent.unicodeScalars.reduce(UInt32(2_166_136_261)) { ($0 ^ $1.value) &* 16_777_619 })
    }

    static func color(for agent: String) -> NSColor {
        palette[hash(agent) % palette.count]
    }

    private var color: NSColor { Self.color(for: agent) }

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

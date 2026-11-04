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
        /// Whether the person has the window under the pointer covered: its
        /// press ring is drawn as faint as the pointer.
        var covered = false
        /// The agent's one press ring, moved to each press and shown again: a
        /// window made for every press stayed in the app after it went (300
        /// presses left 323 windows and 23 MB; one ring each, 10 and 1.2 MB).
        var ring: (panel: PointerPanel, layer: CAShapeLayer)?
        var ringDrop: DispatchWorkItem?
    }

    private static let shared = BackgroundPointerOverlay()
    nonisolated private static let identityLock = NSLock()
    nonisolated(unsafe) private static var pointerWindowIDs: Set<UInt32> = []
    nonisolated(unsafe) private static var pending: [String: (anchor: WindowPointerAnchor, seconds: Double?)] = [:]
    /// Where each agent's pointer was last sent and when (screen points), so the
    /// next move is timed from where a person sees it.
    nonisolated(unsafe) private static var lastPoint: [String: (point: CGPoint, at: Date)] = [:]
    nonisolated(unsafe) private static var latest: WindowPointerAnchor?
    nonisolated(unsafe) private static var displayed: [String: WindowPointerAnchor] = [:]
    nonisolated(unsafe) private static var presentationQueued = false
    /// Which agent last acted on each app: its take-control mark wears that colour.
    nonisolated(unsafe) private static var acting = AgentActingLog()
    private var slots: [String: Slot] = [:]
    private static let arrowSize = CGSize(width: 28, height: 34)
    /// How long a pointer stays after its agent's last act: through the model's
    /// turn between one act and the next (8-17 s a round trip), so it moves on
    /// from where it was instead of blinking out and in again.
    nonisolated private static let staysSeconds = 15
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
    nonisolated private static let firstApproach = CGSize(width: -60, height: 60)
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

    /// Moves the agent's pointer to `point` and says how long the move takes,
    /// so the act lands when the pointer is there, as a person's hand does
    /// (agent-desktop's travel, 90-320 ms), and the point it was sent to. Nil
    /// when no pointer can be drawn there: the act then goes at once, with no
    /// ring. A pointer already on the point does not move, and 0 is the wait.
    nonisolated static func travel(
        pid: pid_t, windowID: UInt32, bounds: CGRect, point: CGPoint, agent: String? = nil
    ) -> (nanoseconds: UInt64, point: CGPoint)? {
        guard !OperatorGuardHost.isStopped(),
              DesktopScreen.displays().contains(where: { $0.bounds.contains(point) }),
              WindowPointerAnchor(pid: pid, windowID: windowID, bounds: bounds, point: point) != nil else { return nil }
        identityLock.lock()
        let last = lastPoint[agent ?? unnamedAgent]
        identityLock.unlock()
        // A pointer still on the screen moves on from where it is; a new one
        // comes in from beside the point, as the panel does.
        let from = last.flatMap { Date().timeIntervalSince($0.at) < Double(staysSeconds) ? $0.point : nil }
            ?? CGPoint(x: point.x + firstApproach.width, y: point.y - firstApproach.height)
        let distance = hypot(point.x - from.x, point.y - from.y)
        let seconds = distance < SmoothPointerPath.arrivedPoints ? 0 : SmoothPointerPath.glideSeconds(distance: distance)
        show(pid: pid, windowID: windowID, bounds: bounds, point: point, agent: agent, seconds: seconds > 0 ? seconds : nil)
        return (UInt64(seconds * 1_000_000_000), point)
    }

    /// A press went through at `point`, where `travel` drew the agent's pointer
    /// for this act: a ring spreads there. Nil, no pointer drawn, draws none.
    nonisolated static func pressed(at point: CGPoint?, agent: String? = nil) {
        guard let point else { return }
        DispatchQueue.main.async {
            let displayHeight = CGDisplayBounds(CGMainDisplayID()).height
            shared.ripple(at: CGPoint(x: point.x, y: displayHeight - point.y), agent: agent ?? unnamedAgent)
        }
    }

    /// Shows the agent's pointer at `point` after the act (a scroll, keys):
    /// it glides there in the time its distance takes, and nothing waits.
    nonisolated static func show(pid: pid_t, windowID: UInt32, bounds: CGRect, point: CGPoint, agent: String? = nil) {
        show(pid: pid, windowID: windowID, bounds: bounds, point: point, agent: agent, seconds: nil)
    }

    nonisolated private static func show(pid: pid_t, windowID: UInt32, bounds: CGRect, point: CGPoint, agent: String?, seconds: Double?) {
        guard let anchor = WindowPointerAnchor(pid: pid, windowID: windowID, bounds: bounds, point: point) else { return }
        identityLock.lock()
        acting.record(pid: pid, agent: agent ?? unnamedAgent)
        lastPoint[agent ?? unnamedAgent] = (point, Date())
        pending[agent ?? unnamedAgent] = (anchor, seconds)
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
                for (agent, waited) in waiting { shared.present(waited.anchor, agent: agent, seconds: waited.seconds) }
            }
        }
    }

    /// The named agent that last acted on the app `pid`, nil when none did.
    nonisolated static func actingAgent(pid: pid_t) -> String? {
        identityLock.lock()
        defer { identityLock.unlock() }
        return acting.agent(pid: pid)
    }

    nonisolated static func hide() {
        identityLock.lock()
        pending = [:]
        lastPoint = [:]
        identityLock.unlock()
        DispatchQueue.main.async { shared.clearAll() }
    }

    private func present(_ anchor: WindowPointerAnchor, agent: String, seconds: Double?) {
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
            retire(agent: oldest)
        }
        let slot = slots[agent] ?? Slot()
        slots[agent] = slot
        // Coming into view, it takes a colour no other agent in view wears.
        let recoloured = AgentColor.pointerShown(agent)
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
        if recoloured { shown.contentView?.needsDisplay = true }
        guard let windowID = UInt32(exactly: shown.windowNumber), windowID > 0 else { return }
        Self.identityLock.lock()
        Self.pointerWindowIDs.insert(windowID)
        Self.identityLock.unlock()
        let inView = DesktopSelf.ownerPid(at: anchor.point) == anchor.pid
            && anchor.visible(in: Self.windows(), ignoringWindowIDs: Self.windowIDs())
        shown.alphaValue = inView ? 1 : Self.coveredAlpha
        shown.setContentSize(frame.size)
        // A pointer still on the screen moves on from where it is; one that had
        // gone comes in from beside the point, as `travel` timed it.
        if !shown.isVisible {
            shown.setFrameOrigin(CGPoint(x: frame.minX + Self.firstApproach.width, y: frame.minY + Self.firstApproach.height))
        }
        let from = shown.frame.origin
        shown.orderFrontRegardless()
        slot.covered = !inView
        glide(slot, panel: shown, from: from, to: frame.origin, agent: agent, seconds: seconds)
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

    /// Steps the panel along a gentle arc to `to`, paced like a hand (fast off
    /// the mark, settling on the point), in `seconds` — the time `travel` told
    /// the act to wait — or the time its distance takes.
    private func glide(_ slot: Slot, panel: NSPanel, from: CGPoint, to: CGPoint, agent: String, seconds planned: Double?) {
        slot.glide?.cancel()
        slot.glide = nil
        let distance = hypot(to.x - from.x, to.y - from.y)
        let seconds = planned ?? SmoothPointerPath.glideSeconds(distance: distance)
        let steps = max(1, Int((seconds * Self.glideStepsPerSecond).rounded()))
        let side = AgentColor.hash(agent) % 2 == 0 ? 1.0 : -1.0
        var path = SmoothPointerPath.handPoints(
            from: .init(x: from.x, y: from.y), to: .init(x: to.x, y: to.y),
            steps: steps, bow: Self.glideBow * side
        )[...]
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.setEventHandler { [weak slot] in
            MainActor.assumeIsolated {
                guard let point = path.popFirst() else { return }
                panel.setFrameOrigin(CGPoint(x: point.x, y: point.y))
                guard path.isEmpty else { return }
                slot?.glide?.cancel()
                slot?.glide = nil
            }
        }
        timer.schedule(deadline: .now(), repeating: seconds / Double(steps))
        slot.glide = timer
        timer.resume()
    }

    /// A ring spreading from where a press landed: the agent's colour, gone in
    /// `rippleSeconds`, and like the pointer no part of any screenshot. Each
    /// agent has one, made at its first press and moved to every press after.
    private func ripple(at point: CGPoint, agent: String) {
        guard !OperatorGuardHost.isStopped(), let slot = slots[agent] else { return }
        let size = Self.rippleSize
        let ring = slot.ring ?? makeRing(agent: agent)
        slot.ring = ring
        slot.ringDrop?.cancel()
        Self.paint(ring.layer, agent: agent)
        ring.panel.alphaValue = slot.covered ? Self.coveredAlpha : 1
        ring.panel.setFrameOrigin(CGPoint(x: point.x - size.width / 2, y: point.y - size.height / 2))
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
        ring.layer.removeAnimation(forKey: "press")
        ring.layer.opacity = 0
        ring.layer.add(both, forKey: "press")
        ring.panel.orderFrontRegardless()
        let drop = DispatchWorkItem { [weak slot] in
            MainActor.assumeIsolated { slot?.ring?.panel.orderOut(nil) }
        }
        slot.ringDrop = drop
        DispatchQueue.main.asyncAfter(deadline: .now() + Self.rippleSeconds + 0.05, execute: drop)
    }

    /// An agent's ring window, listed as the helper's own before it ever shows,
    /// so no window list takes it for a cover.
    private func makeRing(agent: String) -> (panel: PointerPanel, layer: CAShapeLayer) {
        let size = Self.rippleSize
        let panel = PointerPanel(
            contentRect: CGRect(origin: .zero, size: size),
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
        panel.setAccessibilityElement(false)
        let view = NSView(frame: CGRect(origin: .zero, size: size))
        view.wantsLayer = true
        let ring = CAShapeLayer()
        ring.frame = view.bounds
        ring.path = CGPath(ellipseIn: view.bounds.insetBy(dx: 3, dy: 3), transform: nil)
        Self.paint(ring, agent: agent)
        ring.lineWidth = 2.5
        ring.opacity = 0
        view.layer?.addSublayer(ring)
        panel.contentView = view
        if let windowID = UInt32(exactly: panel.windowNumber), windowID > 0 {
            Self.identityLock.lock()
            Self.pointerWindowIDs.insert(windowID)
            Self.identityLock.unlock()
        }
        return (panel, ring)
    }

    /// The ring wears the agent's colour as it is at this press.
    private static func paint(_ ring: CAShapeLayer, agent: String) {
        let color = AgentColor.color(for: agent)
        ring.fillColor = color.withAlphaComponent(0.18).cgColor
        ring.strokeColor = color.cgColor
    }

    private func clear(agent: String) {
        AgentColor.pointerGone(agent)
        guard let slot = slots[agent] else { return }
        slot.expiry?.cancel()
        slot.expiry = nil
        slot.glide?.cancel()
        slot.glide = nil
        slot.ringDrop?.cancel()
        slot.ringDrop = nil
        slot.ring?.panel.orderOut(nil)
        slot.panel?.orderOut(nil)
        Self.identityLock.lock()
        Self.displayed[agent] = nil
        Self.identityLock.unlock()
    }

    /// An agent's pointer and ring give way for good: their windows close and
    /// leave the helper's list.
    private func retire(agent: String) {
        clear(agent: agent)
        guard let slot = slots.removeValue(forKey: agent) else { return }
        let panels: [NSPanel] = [slot.panel, slot.ring?.panel].compactMap { $0 }
        Self.identityLock.lock()
        for panel in panels {
            if let windowID = UInt32(exactly: panel.windowNumber) { Self.pointerWindowIDs.remove(windowID) }
        }
        Self.identityLock.unlock()
        for panel in panels { panel.close() }
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
final class PointerView: NSView {
    /// The whole name picks the colour, as for its ring and its app's mark;
    /// only what is written under the arrow is cut short.
    private let agent: String
    private var label: String { String(agent.prefix(Self.maxLabelCharacters)) }
    private static var labelFont: NSFont { .systemFont(ofSize: 11, weight: .semibold) }
    private static let labelPadding = CGSize(width: 6, height: 2)
    private static let labelOrigin = CGPoint(x: 14, y: 20)
    private static let maxLabelCharacters = 24

    init(frame: CGRect, agent: String) {
        self.agent = agent
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

    /// The arrow wears the agent's colour as it is; its name is white on the
    /// same colour made dark enough to read, as the take-control mark is.
    static func color(for agent: String) -> NSColor { AgentColor.color(for: agent) }
    static func nameGround(for agent: String) -> NSColor { AgentColor.ground(for: agent) }

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
        let text = (label as NSString).size(withAttributes: attributes)
        let pill = CGRect(
            x: Self.labelOrigin.x,
            y: Self.labelOrigin.y,
            width: text.width + Self.labelPadding.width * 2,
            height: text.height + Self.labelPadding.height * 2
        )
        Self.nameGround(for: agent).setFill()
        NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
        (label as NSString).draw(
            at: CGPoint(x: pill.minX + Self.labelPadding.width, y: pill.minY + Self.labelPadding.height),
            withAttributes: attributes
        )
    }
}

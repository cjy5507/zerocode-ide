import AppKit

/// What the control looks like, in points. It sits on top of any app, light or
/// dark, so it carries its own ground, as a system HUD does: the colour of the
/// agent that last acted on the app, as its pointer wears it, inside a white
/// rim, with the panel's shadow under both.
enum ShareTakeLook {
    /// A 32 pt round mark, on the UI's spacing scale.
    static let mark: CGFloat = 32
    /// The space before the name when the control is open.
    static let labelLead: CGFloat = 12
    static let symbol = "hand.raised.fill"
    /// How long the control takes to open under the pointer.
    static let openSeconds = 0.14
    @MainActor static var labelFont: NSFont { .systemFont(ofSize: 12, weight: .medium) }

    @MainActor static func labelWidth(_ label: String) -> CGFloat {
        (label as NSString).size(withAttributes: [.font: labelFont]).width
    }

    /// The ground when no named agent has acted on the app: a HUD's black, see-
    /// through unless the person asked for less transparency.
    static let neutralAlpha: CGFloat = 0.72
    static let neutralOverAlpha: CGFloat = 0.86
    /// How much darker an agent's ground goes under the person's pointer.
    static let overDarkening: CGFloat = 0.15

    /// What the hand and the name are drawn on. An agent's ground is opaque,
    /// so the app under it cannot lighten it below white's 4.5:1.
    @MainActor static func ground(agent: String?, over: Bool, reduceTransparency: Bool) -> NSColor {
        guard let agent, !agent.isEmpty else {
            return NSColor.black.withAlphaComponent(reduceTransparency ? 1 : (over ? neutralOverAlpha : neutralAlpha))
        }
        let ground = AgentColor.ground(for: agent)
        return over ? ground.blended(withFraction: overDarkening, of: .black) ?? ground : ground
    }

    /// The rim that keeps the mark apart from any app: white, and under
    /// Increase Contrast wider, with a black line outside it for white apps.
    static func rim(increaseContrast: Bool) -> (white: CGFloat, dark: CGFloat) {
        increaseContrast ? (2, 1) : (1.5, 0)
    }
}

/// A press counts only as a physical mouse event, and nothing else can press it:
/// the accessibility road and `performClick` do nothing.
@MainActor
final class ShareTakeButton: NSButton {
    var onPointer: ((Bool) -> Void)?
    /// The named agent that last acted on this control's app; nil draws the neutral ground.
    var agent: String? {
        didSet { if agent != oldValue { needsDisplay = true } }
    }
    /// Which handing-out of colours it was drawn by: a colour that moved draws again.
    var colours = 0 {
        didSet { if colours != oldValue { needsDisplay = true } }
    }
    private var over = false
    private var tracking: NSTrackingArea?

    override var acceptsFirstResponder: Bool { false }
    override var needsPanelToBecomeKey: Bool { false }
    override func mouseDown(with event: NSEvent) {
        guard let cg = event.cgEvent else { return }
        ShareControlPanel.takeControl(sourcePID: cg.getIntegerValueField(.eventSourceUnixProcessID),
                                      sourceUserData: cg.getIntegerValueField(.eventSourceUserData))
    }
    /// A screen reader names it as a button, drawn as it is by hand.
    override func accessibilityRole() -> NSAccessibility.Role? { .button }
    override func accessibilityPerformPress() -> Bool { false }
    override func performClick(_ sender: Any?) {}

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
    }

    /// Reduce Transparency and Increase Contrast change how it is drawn.
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        guard window != nil else { return }
        NSWorkspace.shared.notificationCenter.addObserver(
            self, selector: #selector(displayOptionsChanged),
            name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification, object: nil)
    }

    @objc nonisolated private func displayOptionsChanged(_ note: Notification) {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.needsDisplay = true }
        }
    }

    override func mouseEntered(with event: NSEvent) { pointer(over: true) }
    override func mouseExited(with event: NSEvent) { pointer(over: false) }

    private func pointer(over: Bool) {
        self.over = over
        needsDisplay = true
        onPointer?(over)
    }

    override func draw(_ dirtyRect: NSRect) {
        let display = NSWorkspace.shared
        let rim = ShareTakeLook.rim(increaseContrast: display.accessibilityDisplayShouldIncreaseContrast)
        func pill(_ rect: CGRect) -> NSBezierPath {
            NSBezierPath(roundedRect: rect, xRadius: rect.height / 2, yRadius: rect.height / 2)
        }
        // Rings are filled between two outlines, so a see-through ground is
        // never drawn over the white.
        func ring(_ outer: CGRect, width: CGFloat, color: NSColor) -> CGRect {
            let inner = outer.insetBy(dx: width, dy: width)
            guard width > 0 else { return inner }
            let path = pill(outer)
            path.append(pill(inner).reversed)
            path.windingRule = .evenOdd
            color.setFill()
            path.fill()
            return inner
        }
        let inside = ring(ring(bounds, width: rim.dark, color: .black), width: rim.white, color: .white)
        let shape = pill(inside)
        ShareTakeLook.ground(agent: agent, over: over,
                             reduceTransparency: display.accessibilityDisplayShouldReduceTransparency).setFill()
        shape.fill()
        NSGraphicsContext.saveGraphicsState()
        shape.addClip()
        let markX = bounds.maxX - ShareTakeLook.mark
        let attributes: [NSAttributedString.Key: Any] = [.font: ShareTakeLook.labelFont, .foregroundColor: NSColor.white]
        let text = (title as NSString).size(withAttributes: attributes)
        // Read the name only as far as the control has opened.
        if bounds.width > ShareTakeLook.mark {
            (title as NSString).draw(at: CGPoint(x: markX - text.width, y: (bounds.height - text.height) / 2), withAttributes: attributes)
        }
        let symbol = NSImage.SymbolConfiguration(pointSize: 14, weight: .semibold)
            .applying(.init(paletteColors: [.white]))
        if let image = NSImage(systemSymbolName: ShareTakeLook.symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(symbol) {
            let size = image.size
            let at = CGRect(x: markX + (ShareTakeLook.mark - size.width) / 2, y: (bounds.height - size.height) / 2,
                            width: size.width, height: size.height)
            image.draw(in: at, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}

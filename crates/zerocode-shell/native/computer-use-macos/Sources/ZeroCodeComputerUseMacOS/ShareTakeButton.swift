import AppKit

/// What the control looks like, in points. It sits on top of any app, light or
/// dark, so it carries its own ground, as a system HUD does.
enum ShareTakeLook {
    /// A 32 pt round mark, on the UI's spacing scale.
    static let mark: CGFloat = 32
    /// The space before the name when the control is open.
    static let labelLead: CGFloat = 12
    static let symbol = "hand.raised.fill"
    @MainActor static var labelFont: NSFont { .systemFont(ofSize: 12, weight: .medium) }

    @MainActor static func labelWidth(_ label: String) -> CGFloat {
        (label as NSString).size(withAttributes: [.font: labelFont]).width
    }
}

/// A press counts only as a physical mouse event, and nothing else can press it:
/// the accessibility road and `performClick` do nothing.
@MainActor
final class ShareTakeButton: NSButton {
    var onPointer: ((Bool) -> Void)?
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

    override func mouseEntered(with event: NSEvent) { pointer(over: true) }
    override func mouseExited(with event: NSEvent) { pointer(over: false) }

    private func pointer(over: Bool) {
        self.over = over
        needsDisplay = true
        onPointer?(over)
    }

    override func draw(_ dirtyRect: NSRect) {
        let shape = bounds.insetBy(dx: 0.5, dy: 0.5)
        let pill = NSBezierPath(roundedRect: shape, xRadius: shape.height / 2, yRadius: shape.height / 2)
        NSColor.black.withAlphaComponent(over ? 0.86 : 0.72).setFill()
        pill.fill()
        NSColor.white.withAlphaComponent(0.18).setStroke()
        pill.lineWidth = 1
        pill.stroke()
        NSGraphicsContext.saveGraphicsState()
        pill.addClip()
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
            image.draw(in: at, from: .zero, operation: .sourceOver, fraction: over ? 1 : 0.92, respectFlipped: true, hints: nil)
        }
        NSGraphicsContext.restoreGraphicsState()
    }
}

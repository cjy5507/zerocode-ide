import AppKit

/// What an agent's pointer looks like, in points of a flipped view (the origin
/// at the top left, as `PointerView` is). It stands over any app, light or
/// dark, and over a photograph, so it carries its own edge: the agent's colour
/// inside a white rim, with a soft shadow under both, as the take-control mark
/// does. Its name stands beside it and never over it.
enum PointerLook {
    /// Room on every side for the shadow, so none of it is cut at the view's edge.
    static let margin: CGFloat = 6
    /// Where the pointer points: the outermost point of its rim. The window is
    /// put so that this point of the view lies on the point of the act
    /// (`WindowPointerAnchor.panelFrame`), and nothing else knows where it is.
    static let tip = CGPoint(x: margin, y: margin)

    /// The two edges that meet at the tip, as angles below the horizontal, and
    /// their length. A dart with no tail: the person's own pointer has one, so
    /// the two are told apart by shape as well as by colour.
    static let wingDegrees: (right: CGFloat, down: CGFloat) = (26, 72)
    static let wingLength: CGFloat = 22
    /// How far along the line from the tip to the middle of the back the notch
    /// is cut: 1 is no notch at all.
    static let notch: CGFloat = 0.8
    /// The body is filled and then stroked in its own colour with a round join,
    /// so every corner is a circle of half this width.
    static let bodyStroke: CGFloat = 3

    /// The rim that keeps the pointer apart from any app: white, and under
    /// Increase Contrast wider, with a dark line outside it for white apps.
    /// The take-control mark's own numbers (`ShareTakeLook.rim`).
    static func rim(increaseContrast: Bool) -> (white: CGFloat, dark: CGFloat) {
        increaseContrast ? (2, 1) : (1.5, 0)
    }

    static let shadowBlur: CGFloat = 3.5
    /// Down the screen, whichever way the view's axes run.
    static let shadowDrop: CGFloat = 1.5
    static let shadowAlpha: CGFloat = 0.3

    @MainActor static var labelFont: NSFont { .systemFont(ofSize: 11, weight: .semibold) }
    static let labelPadding = CGSize(width: 7, height: 2.5)
    /// The white line round the name's ground.
    static let labelRim: CGFloat = 1
    /// Where the name's ground begins, from the tip: under the right wing and
    /// beside the lower one, with ground showing between the rim of the dart
    /// and the line round the name. Under Increase Contrast both are wider and
    /// the dart stands further back from its tip, so the name stands off by as
    /// much again.
    static func labelOffset(increaseContrast: Bool) -> CGSize {
        increaseContrast ? CGSize(width: 19, height: 22) : CGSize(width: 16, height: 19)
    }
    /// The corner of the name's ground that is nearest the dart is tighter than
    /// the other three, which are half its height: the name leans to its dart.
    static let labelNearCorner: CGFloat = 4
    static let maxLabelCharacters = 24

    /// From the tip towards the middle of the back: the way the dart lies.
    static var axis: CGVector {
        let (right, down) = (unit(wingDegrees.right), unit(wingDegrees.down))
        let (x, y) = (right.dx + down.dx, right.dy + down.dy)
        let length = hypot(x, y)
        return CGVector(dx: x / length, dy: y / length)
    }

    /// How far the rim's outermost point stands off the body's corner: the
    /// round joins of the body and of the rims around it.
    static func reach(increaseContrast: Bool) -> CGFloat {
        let rim = rim(increaseContrast: increaseContrast)
        return bodyStroke / 2 + rim.white + rim.dark
    }

    /// The body's four corners, from the one at the tip round by the right
    /// wing, the notch and the lower wing. The corner at the tip stands `reach`
    /// behind the tip along the axis, so the rim's round corner ends exactly on
    /// the tip.
    static func corners(increaseContrast: Bool) -> [CGPoint] {
        let back = reach(increaseContrast: increaseContrast)
        let corner = CGPoint(x: tip.x + axis.dx * back, y: tip.y + axis.dy * back)
        let (right, down) = (unit(wingDegrees.right), unit(wingDegrees.down))
        let rightEnd = CGPoint(x: corner.x + right.dx * wingLength, y: corner.y + right.dy * wingLength)
        let downEnd = CGPoint(x: corner.x + down.dx * wingLength, y: corner.y + down.dy * wingLength)
        let middle = CGPoint(x: (rightEnd.x + downEnd.x) / 2, y: (rightEnd.y + downEnd.y) / 2)
        let cut = CGPoint(x: corner.x + (middle.x - corner.x) * notch, y: corner.y + (middle.y - corner.y) * notch)
        return [corner, rightEnd, cut, downEnd]
    }

    /// The body's outline, to be filled and stroked with a round join.
    static func body(increaseContrast: Bool) -> NSBezierPath {
        let corners = corners(increaseContrast: increaseContrast)
        let path = NSBezierPath()
        path.move(to: corners[0])
        for corner in corners.dropFirst() { path.line(to: corner) }
        path.close()
        path.lineJoinStyle = .round
        return path
    }

    /// The name's ground, sized to the name as it is written.
    @MainActor static func labelFrame(_ label: String, increaseContrast: Bool) -> CGRect {
        let text = (label as NSString).size(withAttributes: [.font: labelFont])
        let offset = labelOffset(increaseContrast: increaseContrast)
        return CGRect(x: tip.x + offset.width, y: tip.y + offset.height,
                      width: ceil(text.width) + labelPadding.width * 2,
                      height: ceil(text.height) + labelPadding.height * 2)
    }

    /// Room for the dart, for the agent's name beside it when it has one, and
    /// for the shadow of both.
    @MainActor static func size(agent: String) -> CGSize {
        // The widest the rim is ever drawn, so the view is one size whatever
        // the display options are when it is drawn.
        let outline = body(increaseContrast: true).bounds
        let spread = reach(increaseContrast: true) + margin
        var size = CGSize(width: ceil(outline.maxX + spread), height: ceil(outline.maxY + spread))
        guard !agent.isEmpty else { return size }
        // Under Increase Contrast the name stands further off and a dark line
        // stands outside the white one.
        let label = labelFrame(String(agent.prefix(maxLabelCharacters)), increaseContrast: true)
        let around = labelRim + rim(increaseContrast: true).dark + margin
        size.width = max(size.width, ceil(label.maxX + around))
        size.height = max(size.height, ceil(label.maxY + around))
        return size
    }

    /// The name's ground `grown` wider all round: three corners half its height,
    /// the one nearest the dart tighter.
    static func labelGround(_ frame: CGRect, grown: CGFloat = 0) -> NSBezierPath {
        let rect = frame.insetBy(dx: -grown, dy: -grown)
        let round = rect.height / 2
        let near = min(round, labelNearCorner + grown)
        let path = NSBezierPath()
        path.move(to: CGPoint(x: rect.minX + near, y: rect.minY))
        path.line(to: CGPoint(x: rect.maxX - round, y: rect.minY))
        path.appendArc(withCenter: CGPoint(x: rect.maxX - round, y: rect.midY), radius: round, startAngle: -90, endAngle: 90)
        path.line(to: CGPoint(x: rect.minX + round, y: rect.maxY))
        path.appendArc(withCenter: CGPoint(x: rect.minX + round, y: rect.midY), radius: round, startAngle: 90, endAngle: 180)
        path.line(to: CGPoint(x: rect.minX, y: rect.minY + near))
        path.appendArc(withCenter: CGPoint(x: rect.minX + near, y: rect.minY + near), radius: near, startAngle: 180, endAngle: 270)
        path.close()
        return path
    }

    private static func unit(_ degrees: CGFloat) -> CGVector {
        let radians = degrees * .pi / 180
        return CGVector(dx: cos(radians), dy: sin(radians))
    }
}

@MainActor
final class PointerView: NSView {
    /// The whole name picks the colour, as for its ring and its app's mark;
    /// only what is written beside the dart is cut short.
    private let agent: String
    private var label: String { String(agent.prefix(PointerLook.maxLabelCharacters)) }
    /// Increase Contrast as the person has it, unless a picture is asked for
    /// with one or the other.
    var increaseContrast: Bool?

    init(frame: CGRect, agent: String) {
        self.agent = agent
        super.init(frame: frame)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    override var isFlipped: Bool { true }

    static func size(agent: String) -> CGSize { PointerLook.size(agent: agent) }

    /// The dart wears the agent's colour as it is; its name is white on the
    /// same colour made dark enough to read, as the take-control mark is.
    static func color(for agent: String) -> NSColor { AgentColor.color(for: agent) }
    static func nameGround(for agent: String) -> NSColor { AgentColor.ground(for: agent) }

    override func draw(_ dirtyRect: NSRect) {
        let increaseContrast = increaseContrast ?? NSWorkspace.shared.accessibilityDisplayShouldIncreaseContrast
        let rim = PointerLook.rim(increaseContrast: increaseContrast)
        let body = PointerLook.body(increaseContrast: increaseContrast)
        let pill = agent.isEmpty ? nil : PointerLook.labelFrame(label, increaseContrast: increaseContrast)
        /// The dart `width` wider all round: its outline stroked with a round join.
        func dart(grown width: CGFloat, _ color: NSColor) {
            color.set()
            body.lineWidth = PointerLook.bodyStroke + width * 2
            body.fill()
            body.stroke()
        }
        /// The name's ground `width` wider all round.
        func tag(grown width: CGFloat, _ color: NSColor) {
            guard let pill else { return }
            color.setFill()
            PointerLook.labelGround(pill, grown: width).fill()
        }

        // The shadow is cast once, by the rim of the dart and of the name
        // together, so neither casts one on the other.
        NSGraphicsContext.saveGraphicsState()
        let shadow = NSShadow()
        shadow.shadowColor = NSColor.black.withAlphaComponent(PointerLook.shadowAlpha)
        shadow.shadowBlurRadius = PointerLook.shadowBlur
        // A shadow's offset is in the window's own space, y up, whichever way
        // the view is flipped: down the screen is negative.
        shadow.shadowOffset = CGSize(width: 0, height: -PointerLook.shadowDrop)
        shadow.set()
        NSGraphicsContext.current?.cgContext.beginTransparencyLayer(auxiliaryInfo: nil)
        dart(grown: rim.white + rim.dark, .white)
        tag(grown: PointerLook.labelRim + rim.dark, .white)
        NSGraphicsContext.current?.cgContext.endTransparencyLayer()
        NSGraphicsContext.restoreGraphicsState()

        if rim.dark > 0 {
            dart(grown: rim.white + rim.dark, .black)
            tag(grown: PointerLook.labelRim + rim.dark, .black)
        }
        dart(grown: rim.white, .white)
        tag(grown: PointerLook.labelRim, .white)
        dart(grown: 0, Self.color(for: agent))
        guard let pill else { return }
        tag(grown: 0, Self.nameGround(for: agent))
        let attributes: [NSAttributedString.Key: Any] = [.font: PointerLook.labelFont, .foregroundColor: NSColor.white]
        (label as NSString).draw(
            at: CGPoint(x: pill.minX + PointerLook.labelPadding.width, y: pill.minY + PointerLook.labelPadding.height),
            withAttributes: attributes
        )
    }
}

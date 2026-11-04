import AppKit

/// The colour a person tells an agent apart by: its pointer, its press ring,
/// its name under the pointer, and the take-control mark over the app it last
/// acted on all come from here, so the same agent wears one colour everywhere.
@MainActor
enum AgentColor {
    /// The colours agents are told apart by: the same agent keeps its colour.
    static let palette: [NSColor] = [
        .systemPurple, .systemBlue, .systemTeal, .systemGreen, .systemOrange, .systemPink,
    ]
    /// White words and marks on an agent's colour read at least this well
    /// (WCAG AA for ordinary text).
    static let minimumContrast = 4.5
    /// How much darker each try is while a colour is too light for white on it.
    private static let darkeningStep: CGFloat = 0.05

    /// The same agent's name gives the same number: its colour and the side its
    /// moves bow to.
    nonisolated static func hash(_ agent: String) -> Int {
        Int(agent.unicodeScalars.reduce(UInt32(2_166_136_261)) { ($0 ^ $1.value) &* 16_777_619 })
    }

    static func color(for agent: String) -> NSColor {
        palette[hash(agent) % palette.count]
    }

    /// The agent's colour as a ground under white: the palette colour, darkened
    /// only as far as white on it reaches `minimumContrast`. Resolved for the
    /// appearance drawing now, so light, dark and Increase Contrast each get theirs.
    static func ground(for agent: String) -> NSColor {
        readable(color(for: agent))
    }

    static func readable(_ color: NSColor) -> NSColor {
        var shade = srgb(color)
        var fraction: CGFloat = 0
        while contrast(shade, .white) < minimumContrast, fraction < 1 {
            fraction = min(1, fraction + darkeningStep)
            shade = srgb(color).blended(withFraction: fraction, of: .black) ?? .black
        }
        return shade
    }

    /// WCAG contrast ratio between two opaque colours, 1 to 21.
    static func contrast(_ a: NSColor, _ b: NSColor) -> Double {
        let (one, other) = (luminance(a), luminance(b))
        return (max(one, other) + 0.05) / (min(one, other) + 0.05)
    }

    static func luminance(_ color: NSColor) -> Double {
        let c = srgb(color)
        func channel(_ v: CGFloat) -> Double {
            let v = Double(v)
            return v <= 0.040_45 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * channel(c.redComponent) + 0.7152 * channel(c.greenComponent) + 0.0722 * channel(c.blueComponent)
    }

    private static func srgb(_ color: NSColor) -> NSColor {
        color.usingColorSpace(.sRGB) ?? .black
    }
}

/// The agent that last acted on each app, as the helper heard it while drawing
/// that agent's pointer. The newest act wins; the oldest app is forgotten
/// past `limit`, so a long session does not grow it.
struct AgentActingLog {
    static let limit = 32
    private var entries: [pid_t: (agent: String, order: Int)] = [:]
    private var order = 0

    mutating func record(pid: pid_t, agent: String) {
        order += 1
        entries[pid] = (agent, order)
        if entries.count > Self.limit, let oldest = entries.min(by: { $0.value.order < $1.value.order })?.key {
            entries[oldest] = nil
        }
    }

    /// The agent's name, nil when none acted here or the act carried no name.
    func agent(pid: pid_t) -> String? {
        guard let agent = entries[pid]?.agent, !agent.isEmpty else { return nil }
        return agent
    }
}

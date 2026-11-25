import AppKit

/// The colour a person tells an agent apart by: its pointer, its press ring,
/// its name under the pointer, and the take-control mark over the app it last
/// acted on all come from here, so the same agent wears one colour everywhere.
@MainActor
enum AgentColor {
    /// The colours agents are told apart by: the same agent keeps its colour.
    ///
    /// Six hues of one family, each drawn for where it stands. Over a light
    /// desktop it is a tone white already reads on at `minimumContrast`, so an
    /// agent's dart, its name and its app's mark are one colour. Over a dark
    /// one it is a brighter tone, and only the grounds under white are darkened
    /// from it. Under Increase Contrast it is a deeper and a brighter one still.
    ///
    /// The hues stand where the two nearest of the six are as far apart as six
    /// such tones allow (cyan from green, blue from violet), and they are handed
    /// out in this order, so the agents that work first are the furthest apart:
    /// blue and orange, then pink. Green comes last.
    static let tones: [AgentTone] = [
        AgentTone(light: 0x1A6DEE, dark: 0x3A81F6, lightContrast: 0x0050C1, darkContrast: 0x4D90FF),  // blue
        AgentTone(light: 0xBB5600, dark: 0xD56400, lightContrast: 0x8F4000, darkContrast: 0xEA6E00),  // orange
        AgentTone(light: 0xD42C7B, dark: 0xE14E8C, lightContrast: 0xAB005E, darkContrast: 0xED6199),  // pink
        AgentTone(light: 0x00808A, dark: 0x00939F, lightContrast: 0x006169, darkContrast: 0x00A1AE),  // cyan
        AgentTone(light: 0x9D4BD8, dark: 0xAB64E2, lightContrast: 0x8029B7, darkContrast: 0xB775EC),  // violet
        AgentTone(light: 0x008622, dark: 0x009A28, lightContrast: 0x006617, darkContrast: 0x2EA83E),  // green
    ]
    static let palette: [NSColor] = tones.map(\.color)
    /// White words and marks on an agent's colour read at least this well
    /// (WCAG AA for ordinary text).
    static let minimumContrast = 4.5
    /// How much darker each try is while a colour is too light for white on it.
    private static let darkeningStep: CGFloat = 0.05

    /// A pointer that carries no name: no agent's colour.
    static let neutral: NSColor = .systemGray
    /// Which colour each agent wears, kept in the helper's memory only.
    static var book = AgentColorBook(colours: palette.count)

    /// The same agent's name gives the same number: the side its moves bow to.
    nonisolated static func hash(_ agent: String) -> Int {
        Int(agent.unicodeScalars.reduce(UInt32(2_166_136_261)) { ($0 ^ $1.value) &* 16_777_619 })
    }

    static func color(for agent: String) -> NSColor {
        book.wear(agent).map { palette[$0] } ?? neutral
    }

    /// The agent's pointer is on the screen: it wears a colour no one else in
    /// view wears while one is to spare. True when its colour changed.
    @discardableResult
    static func pointerShown(_ agent: String) -> Bool { book.pointerShown(agent) }

    static func pointerGone(_ agent: String) { book.pointerGone(agent) }

    /// The agents whose colour the take-control marks wear now.
    static func marking(_ agents: Set<String>) { book.marking(agents) }

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

/// One colour of the palette: its sRGB tone for each appearance it is drawn in.
struct AgentTone: Sendable, Equatable {
    let light: Int
    let dark: Int
    let lightContrast: Int
    let darkContrast: Int

    /// The appearances a tone is told apart by.
    static let appearances: [NSAppearance.Name] = [
        .aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua,
    ]

    /// The tone as 0xRRGGBB for one of `appearances`; any other name is light's.
    func hex(for appearance: NSAppearance.Name) -> Int {
        switch appearance {
        case .darkAqua: dark
        case .accessibilityHighContrastAqua: lightContrast
        case .accessibilityHighContrastDarkAqua: darkContrast
        default: light
        }
    }

    /// The tone as a colour that follows the appearance drawing it.
    var color: NSColor {
        NSColor(name: nil) { appearance in
            let hex = hex(for: appearance.bestMatch(from: Self.appearances) ?? .aqua)
            return NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                           blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
        }
    }
}

/// Which palette colour each agent wears: the next one in the order the helper
/// first saw the agents, kept by the name while the helper lives. Once every
/// colour has been given, a new agent takes one that no pointer or mark in view
/// wears, and only when all are in view do two agents share one. Nothing here
/// leaves the helper's memory.
struct AgentColorBook {
    /// Names remembered at most; the longest gone out of view is forgotten.
    static let limit = 64
    let colours: Int
    private var given: [String: (index: Int, seen: Int)] = [:]
    private var handedOut = Set<Int>()
    private var clock = 0
    private var pointers = Set<String>()
    private var marks = Set<String>()
    /// Goes up whenever a colour is given or moves, so marks draw again.
    private(set) var generation = 0

    init(colours: Int) { self.colours = max(1, colours) }

    var remembered: Int { given.count }

    /// The colour the agent wears, nil for no name or a name not given one yet.
    func index(for agent: String) -> Int? { given[agent]?.index }

    /// The colour the agent wears, given now if it has none.
    mutating func wear(_ agent: String) -> Int? {
        guard !agent.isEmpty else { return nil }
        if let held = given[agent]?.index { return held }
        give(agent)
        return given[agent]?.index
    }

    @discardableResult
    mutating func pointerShown(_ agent: String) -> Bool {
        guard !agent.isEmpty, !pointers.contains(agent) else { return false }
        pointers.insert(agent)
        let before = given[agent]?.index
        if let before, worn(byOthersThan: agent)[before] == nil {
            clock += 1
            given[agent]?.seen = clock
            return false
        }
        give(agent)
        return before != nil && given[agent]?.index != before
    }

    mutating func pointerGone(_ agent: String) {
        guard pointers.remove(agent) != nil else { return }
        clock += 1
        given[agent]?.seen = clock
    }

    mutating func marking(_ agents: Set<String>) {
        let gone = marks.subtracting(agents)
        marks = agents.filter { !$0.isEmpty }
        for agent in gone where given[agent] != nil {
            clock += 1
            given[agent]?.seen = clock
        }
    }

    private var inView: Set<String> { pointers.union(marks) }

    private func worn(byOthersThan agent: String) -> [Int: Int] {
        var count: [Int: Int] = [:]
        for other in inView where other != agent {
            if let index = given[other]?.index { count[index, default: 0] += 1 }
        }
        return count
    }

    /// A colour never given, else one nobody else in view wears (the one worn
    /// longest ago), else the one fewest in view wear.
    private mutating func give(_ agent: String) {
        let worn = worn(byOthersThan: agent)
        let index: Int
        if let fresh = (0..<colours).first(where: { !handedOut.contains($0) && worn[$0] == nil }) {
            index = fresh
        } else if let spare = (0..<colours).filter({ worn[$0] == nil }).min(by: { lastWorn($0) < lastWorn($1) }) {
            index = spare
        } else {
            index = (0..<colours).min { (worn[$0] ?? 0, $0) < (worn[$1] ?? 0, $1) } ?? 0
        }
        clock += 1
        given[agent] = (index, clock)
        handedOut.insert(index)
        generation += 1
        forgetPastLimit()
    }

    private func lastWorn(_ index: Int) -> Int {
        given.values.filter { $0.index == index }.map(\.seen).max() ?? 0
    }

    private mutating func forgetPastLimit() {
        let view = inView
        while given.count > Self.limit,
              let oldest = given.filter({ !view.contains($0.key) }).min(by: { $0.value.seen < $1.value.seen })?.key {
            given[oldest] = nil
        }
    }
}

/// One drawn pointer: an agent has one for each app it works in, so an agent
/// working in several apps at once is seen in each of them.
struct PointerKey: Hashable, Sendable {
    let agent: String
    let pid: pid_t
}

/// The pointers in view. An agent wears its colour while any of its pointers
/// is in view, so the roster says when its last one goes.
struct PointerRoster {
    private var inView = Set<PointerKey>()

    var count: Int { inView.count }

    /// The pointer came into view.
    mutating func shown(_ key: PointerKey) { inView.insert(key) }

    /// The pointer went. True when its agent has none left in view.
    @discardableResult
    mutating func gone(_ key: PointerKey) -> Bool {
        guard inView.remove(key) != nil else { return false }
        return !inView.contains { $0.agent == key.agent }
    }
}

/// The pointers that wait to be drawn. They are drawn one at a time, the
/// longest waiting first, so none is drawn twice while another waits; one that
/// waits again before its turn keeps the turn and takes its newest place.
struct PointerWaiting<Place> {
    private var places: [PointerKey: Place] = [:]
    private var order: [PointerKey] = []

    var isEmpty: Bool { order.isEmpty }

    /// `key` waits to be drawn at `place`. True when none waited before it:
    /// the drawing is to be begun.
    @discardableResult
    mutating func enter(_ key: PointerKey, at place: Place) -> Bool {
        let first = order.isEmpty
        if places.updateValue(place, forKey: key) == nil { order.append(key) }
        return first
    }

    /// The pointer that has waited longest, and where it is to be drawn.
    mutating func next() -> (key: PointerKey, place: Place)? {
        guard !order.isEmpty else { return nil }
        let key = order.removeFirst()
        guard let place = places.removeValue(forKey: key) else { return nil }
        return (key, place)
    }

    /// None waits any more.
    mutating func forget() {
        places = [:]
        order = []
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

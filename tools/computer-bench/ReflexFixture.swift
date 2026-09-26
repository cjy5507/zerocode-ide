// The realtime bench's own fixture (t-6767): one borderless, non-activating
// window of its own process and bundle, a SpriteKit scene that plays the round
// fixture_reflex.py drew from its seed — a strip across the top in the phase's
// colour, one target at a time, still decoys of the other colour, curtains
// sweeping across — and an oracle of its own. Every event the window server
// hands this window is recorded with the process that posted it, and every
// press is judged here against what this scene drew, never against anything a
// run says about itself. A hit target goes; nothing else a run does changes
// the round.
//
// It keeps no number of its own: the round file carries the canvas, the strip,
// the palette, the frame rate, how far a press may lag the frame it answers
// (the reflex table's frame age) and the round. It never activates itself,
// reads nothing but its own files, and writes only into its state folder.
import AppKit
import SpriteKit

struct RGB: Decodable {
    let r: Double
    let g: Double
    let b: Double

    var color: NSColor { NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: 1) }
}

struct Canvas: Decodable {
    let width: Double
    let height: Double
}

struct Phase: Decodable {
    let index: Int
    let colour: String
    let startMs: Double
    let endMs: Double
}

/// A target, a decoy or a curtain of the round: where it starts, how it
/// drifts (points a second) and when it shows, in the round's milliseconds.
struct Mover: Decodable {
    let id: String
    let colour: String?
    let appearMs: Double
    let expireMs: Double
    let x: Double
    let y: Double
    let vx: Double?
    let vy: Double?
    let radius: Double?
    let width: Double?
    let height: Double?

    func at(_ ms: Double) -> CGPoint {
        let elapsed = (ms - appearMs) / 1_000
        return CGPoint(x: x + (vx ?? 0) * elapsed, y: y + (vy ?? 0) * elapsed)
    }

    func shows(at ms: Double) -> Bool { appearMs <= ms && ms < expireMs }
}

struct Schedule: Decodable {
    let seed: Int
    let lengthMs: Double
    let phases: [Phase]
    let targets: [Mover]
    let decoys: [Mover]
    let curtains: [Mover]
}

struct Round: Decodable {
    let owner: String
    let seed: Int
    let canvas: Canvas
    let hud: Double
    let palette: [String: RGB]
    let framesPerSecond: Int
    let frameAgeNs: UInt64
    let schedule: Schedule
}

func uptimeNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

/// One shape as a frame drew it, in the window's content points (top-left origin).
struct Drawn {
    enum Kind: String { case target, decoy }
    let id: String
    let kind: Kind
    let colour: String
    let centre: CGPoint
    let radius: Double

    func covers(_ point: CGPoint) -> Bool { hypot(point.x - centre.x, point.y - centre.y) <= radius }
}

/// What one frame showed, kept a little while so a press is judged against
/// what was on the screen when the run could have seen it.
struct Frame {
    let seq: Int
    let ns: UInt64
    let phase: Int
    let colour: String
    let shapes: [Drawn]
    let curtains: [CGRect]

    func curtained(_ point: CGPoint) -> Bool { curtains.contains { $0.contains(point) } }
}

/// The fixture's record: lines appended to its files off the main thread, and
/// its state rewritten whole each time.
final class Recorder: @unchecked Sendable {
    let folder: URL
    private let queue = DispatchQueue(label: "reflex-fixture.recorder")
    private var events: [[String: Any]] = []
    private var frames: [[String: Any]] = []

    init(folder: URL) {
        self.folder = folder
    }

    func event(_ row: [String: Any]) { events.append(row) }
    func frame(_ row: [String: Any]) { frames.append(row) }

    func write(_ name: String, _ object: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return }
        try? data.write(to: folder.appendingPathComponent(name), options: .atomic)
    }

    /// Append what came since the last flush, then the state: encoded here,
    /// written on the recorder's own queue.
    func flush(state: [String: Any], wait: Bool = false) {
        func lines(_ rows: [[String: Any]]) -> Data {
            var text = Data()
            for row in rows {
                guard let line = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) else { continue }
                text.append(line)
                text.append(0x0A)
            }
            return text
        }
        let appended = [("events.jsonl", lines(events)), ("frames.jsonl", lines(frames))]
        events = []
        frames = []
        let whole = (try? JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])) ?? Data()
        let folder = self.folder
        let work: @Sendable () -> Void = {
            for (name, text) in appended where !text.isEmpty {
                let url = folder.appendingPathComponent(name)
                if let handle = try? FileHandle(forWritingTo: url) {
                    handle.seekToEndOfFile()
                    handle.write(text)
                    try? handle.close()
                } else {
                    try? text.write(to: url)
                }
            }
            if !whole.isEmpty {
                try? whole.write(to: folder.appendingPathComponent("fixture.json"), options: .atomic)
            }
        }
        if wait { queue.sync(execute: work) } else { queue.async(execute: work) }
    }
}

/// The scene: the round played on the host clock from the goal's moment
/// (`start.json`, written by the runner), each frame recorded, each press judged.
@MainActor
final class Arena: SKScene {
    let round: Round
    let recorder: Recorder
    let strip: SKSpriteNode
    private var nodes: [String: SKNode] = [:]
    private var hitAt: [String: UInt64] = [:]
    private var ring: [Frame] = []
    private var seq = 0
    private(set) var t0Ns: UInt64?
    private(set) var firstFrameNs: UInt64?
    private(set) var lastFrameNs: UInt64?
    var downs = 0
    var ups = 0
    var hits = 0
    var misses: [String: Int] = [:]
    var held: Set<Int> = []
    var received = 0

    init(round: Round, recorder: Recorder) {
        self.round = round
        self.recorder = recorder
        strip = SKSpriteNode(color: round.palette["idle"]?.color ?? .gray,
                             size: CGSize(width: round.canvas.width, height: round.hud))
        super.init(size: CGSize(width: round.canvas.width, height: round.canvas.height))
        backgroundColor = round.palette["ground"]?.color ?? .black
        anchorPoint = .zero
        scaleMode = .resizeFill
        strip.anchorPoint = .zero
        strip.position = CGPoint(x: 0, y: round.canvas.height - round.hud)
        strip.zPosition = 3
        addChild(strip)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("the fixture is built in code") }

    /// A content point (top-left origin) in the scene's own (bottom-left) space.
    private func scene(_ point: CGPoint) -> CGPoint { CGPoint(x: point.x, y: round.canvas.height - point.y) }

    private var startFile: URL { recorder.folder.appendingPathComponent("start.json") }

    override func update(_ currentTime: TimeInterval) {
        let now = uptimeNs()
        if t0Ns == nil,
           let data = try? Data(contentsOf: startFile),
           let start = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let t0 = (start["t0Ns"] as? NSNumber)?.uint64Value {
            t0Ns = t0
        }
        guard let t0 = t0Ns, now >= t0 else { return }
        let ms = Double(now - t0) / 1_000_000
        guard ms < round.schedule.lengthMs,
              let phase = round.schedule.phases.first(where: { $0.startMs <= ms && ms < $0.endMs })
        else {
            strip.color = round.palette["idle"]?.color ?? .gray
            for node in nodes.values { node.removeFromParent() }
            nodes.removeAll()
            return
        }
        strip.color = round.palette[phase.colour]?.color ?? .gray
        var shapes: [Drawn] = []
        for mover in round.schedule.targets where mover.shows(at: ms) && hitAt[mover.id] == nil {
            shapes.append(Drawn(id: mover.id, kind: .target, colour: mover.colour ?? phase.colour,
                                centre: mover.at(ms), radius: mover.radius ?? 0))
        }
        for mover in round.schedule.decoys where mover.shows(at: ms) {
            shapes.append(Drawn(id: mover.id, kind: .decoy, colour: mover.colour ?? phase.colour,
                                centre: mover.at(ms), radius: mover.radius ?? 0))
        }
        var curtains: [(String, CGRect)] = []
        for mover in round.schedule.curtains where mover.shows(at: ms) {
            let corner = mover.at(ms)
            curtains.append((mover.id, CGRect(x: corner.x, y: corner.y, width: mover.width ?? 0, height: mover.height ?? 0)))
        }
        var showing = Set<String>()
        for shape in shapes {
            showing.insert(shape.id)
            let node = nodes[shape.id] ?? {
                let disc = SKShapeNode(circleOfRadius: shape.radius)
                disc.fillColor = round.palette[shape.colour]?.color ?? .white
                disc.strokeColor = .clear
                disc.lineWidth = 0
                disc.zPosition = 1
                addChild(disc)
                nodes[shape.id] = disc
                return disc
            }()
            node.position = scene(shape.centre)
        }
        for (id, rect) in curtains {
            showing.insert(id)
            let node = nodes[id] ?? {
                let sheet = SKSpriteNode(color: round.palette["curtain"]?.color ?? .darkGray, size: rect.size)
                sheet.anchorPoint = CGPoint(x: 0, y: 1)
                sheet.zPosition = 2
                addChild(sheet)
                nodes[id] = sheet
                return sheet
            }()
            node.position = scene(rect.origin)
        }
        for (id, node) in nodes where !showing.contains(id) {
            node.removeFromParent()
            nodes[id] = nil
        }
        seq += 1
        firstFrameNs = firstFrameNs ?? now
        lastFrameNs = now
        let frame = Frame(seq: seq, ns: now, phase: phase.index, colour: phase.colour, shapes: shapes,
                          curtains: curtains.map(\.1))
        ring.append(frame)
        // Keep the frames a press may still be judged against, and the one before them.
        let keepFrom = now > 2 * round.frameAgeNs ? now - 2 * round.frameAgeNs : 0
        if let first = ring.firstIndex(where: { $0.ns >= keepFrom }), first > 1 {
            ring.removeFirst(first - 1)
        }
        recorder.frame(["seq": seq, "ns": now, "tMs": ms, "phase": phase.index,
                        "shown": shapes.map(\.id) + curtains.map(\.0)])
    }

    /// A press at `point` at `atNs`: a hit when a target of the phase's colour
    /// covered the point, not under a curtain, on a frame shown within the frame
    /// age before the press (or the frame on screen when that span began), and
    /// no earlier press took it. Otherwise why it missed, read off the newest
    /// frame shown by the press.
    func judge(_ point: CGPoint, atNs: UInt64) -> [String: String] {
        let from = atNs > round.frameAgeNs ? atNs - round.frameAgeNs : 0
        var seen = ring.filter { $0.ns > from && $0.ns <= atNs }
        if let before = ring.last(where: { $0.ns <= from }) { seen.insert(before, at: 0) }
        for frame in seen.reversed() {
            for shape in frame.shapes where shape.kind == .target && shape.colour == frame.colour
                && hitAt[shape.id] == nil && shape.covers(point) && !frame.curtained(point) {
                hitAt[shape.id] = atNs
                return ["hit": shape.id]
            }
        }
        let newest = ring.last { $0.ns <= atNs }
        if newest?.curtained(point) == true { return ["miss": "curtain"] }
        if let decoy = newest?.shapes.first(where: { $0.kind == .decoy && $0.covers(point) }) {
            return ["miss": "decoy", "near": decoy.id]
        }
        if let target = ring.reversed().lazy.flatMap(\.shapes).first(where: { $0.kind == .target && $0.covers(point) }) {
            return ["miss": "late", "near": target.id]
        }
        if point.y < round.hud { return ["miss": "hud"] }
        return ["miss": t0Ns == nil ? "before_round" : "empty"]
    }

    var state: [String: Any] {
        var state: [String: Any] = [
            "owner": round.owner, "seed": round.seed, "pid": Int(getpid()), "frames": seq,
            "downs": downs, "ups": ups, "hits": hits, "misses": misses, "held": Array(held).sorted(),
            "events": received,
        ]
        if let t0Ns { state["t0Ns"] = t0Ns }
        if let firstFrameNs { state["firstFrameNs"] = firstFrameNs }
        if let lastFrameNs { state["lastFrameNs"] = lastFrameNs }
        return state
    }
}

/// The scene's view: every mouse, scroll and key event this window is handed
/// is recorded with its source process and judged when it is a press.
@MainActor
final class ArenaView: SKView {
    var arena: Arena?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas { removeTrackingArea(area) }
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeAlways, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }

    private func record(_ kind: String, _ event: NSEvent, extra: [String: Any] = [:]) {
        guard let arena else { return }
        let received = uptimeNs()
        let local = convert(event.locationInWindow, from: nil)
        let point = CGPoint(x: local.x, y: bounds.height - local.y)
        let posted = UInt64(max(0, event.timestamp) * 1_000_000_000)
        var row: [String: Any] = [
            "kind": kind, "evNs": posted, "rxNs": received,
            "x": Double(point.x), "y": Double(point.y),
            "sourcePid": event.cgEvent?.getIntegerValueField(.eventSourceUnixProcessID) ?? -1,
            "userData": event.cgEvent?.getIntegerValueField(.eventSourceUserData) ?? 0,
        ]
        row.merge(extra) { _, new in new }
        arena.received += 1
        row["n"] = arena.received
        if kind == "down" {
            arena.downs += 1
            arena.held.insert(event.buttonNumber)
            // The post time judges the press when it is a time on this clock; else its arrival.
            let at = posted > 0 && posted <= received ? posted : received
            let judged = event.buttonNumber == 0 ? arena.judge(point, atNs: at) : ["miss": "button"]
            if judged["hit"] != nil {
                arena.hits += 1
            } else if let why = judged["miss"] {
                arena.misses[why, default: 0] += 1
            }
            row["judged"] = judged
            row["button"] = event.buttonNumber
        } else if kind == "up" {
            arena.ups += 1
            arena.held.remove(event.buttonNumber)
            row["button"] = event.buttonNumber
        }
        arena.recorder.event(row)
    }

    override func mouseDown(with event: NSEvent) { record("down", event) }
    override func mouseUp(with event: NSEvent) { record("up", event) }
    override func rightMouseDown(with event: NSEvent) { record("down", event) }
    override func rightMouseUp(with event: NSEvent) { record("up", event) }
    override func otherMouseDown(with event: NSEvent) { record("down", event) }
    override func otherMouseUp(with event: NSEvent) { record("up", event) }
    override func mouseMoved(with event: NSEvent) { record("move", event) }
    override func mouseDragged(with event: NSEvent) { record("drag", event) }
    override func rightMouseDragged(with event: NSEvent) { record("drag", event) }
    override func otherMouseDragged(with event: NSEvent) { record("drag", event) }
    override func scrollWheel(with event: NSEvent) { record("scroll", event) }
    override func keyDown(with event: NSEvent) { record("key", event, extra: ["repeat": event.isARepeat, "key": Int(event.keyCode)]) }
    override func keyUp(with event: NSEvent) { record("key", event, extra: ["repeat": true, "key": Int(event.keyCode)]) }
    override func flagsChanged(with event: NSEvent) { record("flags", event) }
}

@MainActor
final class Fixture: NSObject, NSApplicationDelegate {
    let round: Round
    let recorder: Recorder
    var panel: NSPanel?
    var arena: Arena?
    var flushTimer: Timer?
    var becameActive = false
    var activity: NSObjectProtocol?
    var signals: [DispatchSourceSignal] = []

    init(round: Round, recorder: Recorder) {
        self.round = round
        self.recorder = recorder
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // A visible round keeps its frame rate: no App Nap, no timer coalescing.
        activity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .latencyCritical],
                                                         reason: "a reflex bench round")
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil,
                                               queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.becameActive = true }
        }
        let size = CGSize(width: round.canvas.width, height: round.canvas.height)
        // Around the pointer, inside the screen it is on: the run's first glide
        // starts where the pointer rests, so it never crosses another app.
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) } ?? NSScreen.main
        let visible = screen?.visibleFrame ?? CGRect(origin: .zero, size: size)
        // Whole points, so the window server names the window exactly where it is.
        let origin = CGPoint(x: min(max(mouse.x - size.width / 2, visible.minX), visible.maxX - size.width).rounded(.down),
                             y: min(max(mouse.y - size.height / 2, visible.minY), visible.maxY - size.height).rounded(.down))
        let frame = CGRect(origin: origin, size: size)
        let panel = NSPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isFloatingPanel = false
        panel.level = .normal
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = true
        panel.acceptsMouseMovedEvents = true
        panel.isReleasedWhenClosed = false
        panel.colorSpace = .sRGB
        let arena = Arena(round: round, recorder: recorder)
        let view = ArenaView(frame: CGRect(origin: .zero, size: size))
        view.preferredFramesPerSecond = round.framesPerSecond
        view.ignoresSiblingOrder = true
        view.arena = arena
        view.presentScene(arena)
        panel.contentView = view
        panel.orderFrontRegardless()
        self.panel = panel
        self.arena = arena
        // Quartz's global space (top-left of the main display), the space a run's points are in.
        let mainHeight = NSScreen.screens.first?.frame.height ?? size.height
        let quartz = CGRect(x: frame.minX, y: mainHeight - frame.maxY, width: frame.width, height: frame.height)
        recorder.write("ready.json", [
            "owner": round.owner, "seed": round.seed, "pid": Int(getpid()),
            "bundleId": Bundle.main.bundleIdentifier ?? "",
            "window": ["x": quartz.minX, "y": quartz.minY, "width": quartz.width, "height": quartz.height],
            "pointer": ["x": mouse.x, "y": mainHeight - mouse.y],
            "pointerInside": NSMouseInRect(mouse, frame, false),
            "readyNs": uptimeNs(), "events": 0, "hits": 0, "misses": 0,
            "active": NSApp.isActive,
        ])
        flushTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.flush() }
        }
        for signalNumber in [SIGTERM, SIGINT] {
            signal(signalNumber, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: signalNumber, queue: .main)
            source.setEventHandler { [weak self] in
                MainActor.assumeIsolated {
                    self?.flush(wait: true)
                    exit(0)
                }
            }
            source.resume()
            signals.append(source)
        }
    }

    func flush(wait: Bool = false) {
        guard let arena else { return }
        var state = arena.state
        state["becameActive"] = becameActive || NSApp.isActive
        state["endedNs"] = uptimeNs()
        recorder.flush(state: state, wait: wait)
    }
}

let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write(Data("usage: ReflexFixture <round.json> <state-folder>\n".utf8))
    exit(2)
}
let folder = URL(fileURLWithPath: arguments[2], isDirectory: true)
guard let data = FileManager.default.contents(atPath: arguments[1]),
      let round = try? JSONDecoder().decode(Round.self, from: data)
else {
    FileHandle.standardError.write(Data("the round file does not read\n".utf8))
    exit(2)
}
let fixture = MainActor.assumeIsolated { Fixture(round: round, recorder: Recorder(folder: folder)) }
let app = NSApplication.shared
// A regular app, so the helper can resolve the run's scope by this bundle's id;
// it never activates itself and its window takes no focus.
app.setActivationPolicy(.regular)
app.delegate = fixture
app.run()

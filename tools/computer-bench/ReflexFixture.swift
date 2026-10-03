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
//
// A round's kind (t-26708) adds to the same scene: `timing` shows a target in
// the preview colour until its arm, and a press before the arm is `early`;
// `avoid` sends a sweeper from outside the field toward where the pointer
// rested at its launch, with a pad shown on the far side — at arrival the
// pointer under the sweeper is `struck`, else `clear`, written to the oracle
// stream; `panel` is the plain scene with more decoys. The plain round reads
// none of these fields.
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

    /// When a target becomes right to press (a timing round's preview ends); absent, at its appearance.
    let armMs: Double?

    func at(_ ms: Double) -> CGPoint {
        let elapsed = (ms - appearMs) / 1_000
        return CGPoint(x: x + (vx ?? 0) * elapsed, y: y + (vy ?? 0) * elapsed)
    }

    func shows(at ms: Double) -> Bool { appearMs <= ms && ms < expireMs }
    func armed(at ms: Double) -> Bool { (armMs ?? appearMs) <= ms }
}

/// A sweeper of an avoid round: launched at `appearMs` from a point outside
/// the field toward where the pointer rests then, arriving `flightMs` later,
/// with a pad to move to shown on the far side while it flies.
struct Sweep: Decodable {
    let id: String
    let appearMs: Double
    let flightMs: Double
    let fromX: Double
    let fromY: Double
    let width: Double
    let height: Double
    let padX: Double
    let padY: Double
    let padRadius: Double

    var arriveMs: Double { appearMs + flightMs }
    func flies(at ms: Double) -> Bool { appearMs <= ms && ms < arriveMs }
}

struct Schedule: Decodable {
    let seed: Int
    let lengthMs: Double
    let phases: [Phase]
    let targets: [Mover]
    let decoys: [Mover]
    let curtains: [Mover]
    let sweeps: [Sweep]?
}

/// What a covered round (t-12979) puts over the field from the fixture's own
/// app: a second sheet at a place in the window's content points (top-left
/// origin), from a moment of the round.
struct CoverSpec: Decodable {
    let kind: String
    let x: Double
    let y: Double
    let width: Double
    let height: Double
    let appearMs: Double
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
    let cover: CoverSpec?
    /// The round's kind (t-26708); absent for the plain round.
    let kind: String?
}

/// One shape as a frame drew it, in the window's content points (top-left origin).
struct Drawn {
    enum Kind: String { case target, decoy }
    let id: String
    let kind: Kind
    let colour: String
    let centre: CGPoint
    let radius: Double
    /// A target right to press; a preview is not, and a decoy never is.
    let armed: Bool

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
    /// Called once, when the round's cover is due (t-12979).
    var onCover: (@MainActor (CoverSpec) -> Void)?
    private var covered = false
    /// Where the pointer rests now, in the window's content points — read off
    /// the window server, never off an event the hand must post first.
    var pointerNow: (@MainActor () -> CGPoint?)?
    /// Each sweeper in flight: where it heads (the pointer's rest at its launch) and when it left.
    private var flights: [String: (end: CGPoint, launchedNs: UInt64)] = [:]
    private var arrived: Set<String> = []

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
        if !covered, let cover = round.cover, ms >= cover.appearMs {
            covered = true
            onCover?(cover)
        }
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
            let armed = mover.armed(at: ms)
            shapes.append(Drawn(id: mover.id, kind: .target, colour: armed ? mover.colour ?? phase.colour : "preview",
                                centre: mover.at(ms), radius: mover.radius ?? 0, armed: armed))
        }
        for mover in round.schedule.decoys where mover.shows(at: ms) {
            shapes.append(Drawn(id: mover.id, kind: .decoy, colour: mover.colour ?? phase.colour,
                                centre: mover.at(ms), radius: mover.radius ?? 0, armed: false))
        }
        let sweeps = sweepers(at: ms, now: now)
        var curtains: [(String, CGRect)] = []
        for mover in round.schedule.curtains where mover.shows(at: ms) {
            let corner = mover.at(ms)
            curtains.append((mover.id, CGRect(x: corner.x, y: corner.y, width: mover.width ?? 0, height: mover.height ?? 0)))
        }
        var showing = Set<String>()
        for shape in shapes {
            showing.insert(shape.id)
            let colour = round.palette[shape.colour]?.color ?? .white
            let node = nodes[shape.id] ?? {
                let disc = SKShapeNode(circleOfRadius: shape.radius)
                disc.fillColor = colour
                disc.strokeColor = .clear
                disc.lineWidth = 0
                disc.zPosition = 1
                addChild(disc)
                nodes[shape.id] = disc
                return disc
            }()
            // A target's colour changes once, at its arm: set when it differs, never every frame.
            if let disc = node as? SKShapeNode, disc.fillColor != colour { disc.fillColor = colour }
            node.position = scene(shape.centre)
        }
        for (id, rect, pad, radius) in sweeps {
            showing.insert(id)
            showing.insert(id + ":pad")
            let sheet = nodes[id] ?? {
                let sheet = SKSpriteNode(color: round.palette["sweeper"]?.color ?? .orange, size: rect.size)
                sheet.anchorPoint = CGPoint(x: 0, y: 1)
                sheet.zPosition = 1
                addChild(sheet)
                nodes[id] = sheet
                return sheet
            }()
            sheet.position = scene(rect.origin)
            let disc = nodes[id + ":pad"] ?? {
                let disc = SKShapeNode(circleOfRadius: radius)
                disc.fillColor = round.palette["pad"]?.color ?? .green
                disc.strokeColor = .clear
                disc.lineWidth = 0
                disc.zPosition = 1
                addChild(disc)
                nodes[id + ":pad"] = disc
                return disc
            }()
            disc.position = scene(pad)
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
                        "shown": shapes.map { $0.kind == .target && !$0.armed ? $0.id + ":preview" : $0.id }
                            + curtains.map(\.0) + sweeps.map(\.0)])
    }

    /// The sweepers in flight at `ms`: each one's rectangle on this frame, its
    /// pad and the pad's radius. A sweeper first seen now heads for where the
    /// pointer rests at this moment; one past its arrival is judged once —
    /// the pointer under it is struck, else clear — and written to the oracle
    /// stream with the moments it left and arrived.
    private func sweepers(at ms: Double, now: UInt64) -> [(String, CGRect, CGPoint, Double)] {
        var flying: [(String, CGRect, CGPoint, Double)] = []
        for sweep in round.schedule.sweeps ?? [] where sweep.appearMs <= ms && !arrived.contains(sweep.id) {
            let start = CGPoint(x: sweep.fromX, y: sweep.fromY)
            let flight = flights[sweep.id] ?? {
                let end = pointerNow?() ?? CGPoint(x: round.canvas.width / 2, y: (round.hud + round.canvas.height) / 2)
                flights[sweep.id] = (end, now)
                return (end, now)
            }()
            let share = min(1, (ms - sweep.appearMs) / sweep.flightMs)
            let centre = CGPoint(x: start.x + (flight.end.x - start.x) * share, y: start.y + (flight.end.y - start.y) * share)
            let rect = CGRect(x: centre.x - sweep.width / 2, y: centre.y - sweep.height / 2, width: sweep.width, height: sweep.height)
            if ms >= sweep.arriveMs {
                arrived.insert(sweep.id)
                let pointer = pointerNow?()
                let struck = pointer.map { rect.contains($0) } ?? false
                var row: [String: Any] = ["kind": "sweep", "id": sweep.id, "launchedNs": flight.launchedNs, "arrivedNs": now,
                                          "outcome": struck ? "struck" : "clear"]
                if let pointer { row["pointer"] = ["x": Double(pointer.x), "y": Double(pointer.y)] }
                recorder.scene(row)
                continue
            }
            flying.append((sweep.id, rect, CGPoint(x: sweep.padX, y: sweep.padY), sweep.padRadius))
        }
        return flying
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
            for shape in frame.shapes where shape.kind == .target && shape.armed && shape.colour == frame.colour
                && hitAt[shape.id] == nil && shape.covers(point) && !frame.curtained(point) {
                hitAt[shape.id] = atNs
                return ["hit": shape.id]
            }
        }
        let newest = ring.last { $0.ns <= atNs }
        if newest?.curtained(point) == true { return ["miss": "curtain"] }
        if let preview = newest?.shapes.first(where: { $0.kind == .target && !$0.armed && $0.covers(point) }) {
            return ["miss": "early", "near": preview.id]
        }
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
    var lifetime: FixtureLifetime?
    var becameActive = false
    var cover: CoverSheet?

    init(round: Round, recorder: Recorder) {
        self.round = round
        self.recorder = recorder
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let size = CGSize(width: round.canvas.width, height: round.canvas.height)
        let arena = Arena(round: round, recorder: recorder)
        let view = ArenaView(frame: CGRect(origin: .zero, size: size))
        view.preferredFramesPerSecond = round.framesPerSecond
        view.ignoresSiblingOrder = true
        view.arena = arena
        view.presentScene(arena)
        let window = FixtureWindow(view: view, owner: round.owner, seed: round.seed)
        self.panel = window.panel
        self.arena = arena
        arena.onCover = { [weak self] spec in self?.showCover(spec) }
        arena.pointerNow = { [weak view] in
            guard let view, let window = view.window else { return nil }
            let local = view.convert(window.mouseLocationOutsideOfEventStream, from: nil)
            return CGPoint(x: local.x, y: view.bounds.height - local.y)
        }
        recorder.write("ready.json", window.ready)
        lifetime = FixtureLifetime(flush: { [weak self] wait in self?.flush(wait: wait) },
                                   becameActive: { [weak self] in self?.becameActive = true })
    }

    /// The round's cover from this app: a sheet over the fixture's window, at
    /// the place the scene names in its content points.
    func showCover(_ spec: CoverSpec) {
        guard let panel else { return }
        let mainHeight = NSScreen.screens.first?.frame.height ?? panel.frame.maxY
        let quartz = CGRect(x: panel.frame.minX + spec.x, y: mainHeight - panel.frame.maxY + spec.y,
                            width: spec.width, height: spec.height)
        let sheet = CoverSheet(kind: spec.kind, quartz: quartz)
        sheet.show()
        cover = sheet
    }

    func flush(wait: Bool = false) {
        guard let arena else { return }
        var state = arena.state
        if let cover { state["cover"] = cover.state }
        state["becameActive"] = becameActive || NSApp.isActive
        state["endedNs"] = uptimeNs()
        recorder.flush(state: state, wait: wait)
    }
}

@main
@MainActor
struct ReflexFixtureMain {
    static func main() {
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
        let fixture = Fixture(round: round, recorder: Recorder(folder: folder))
        let app = NSApplication.shared
        // A regular app, so the helper can resolve the run's scope by this bundle's id;
        // it never activates itself and its window takes no focus.
        app.setActivationPolicy(.regular)
        app.delegate = fixture
        withExtendedLifetime(fixture) { app.run() }
    }
}

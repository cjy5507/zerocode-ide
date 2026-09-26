// The RTS oracle owns the scene and judges only events AppKit delivered to
// this process. It never activates a window or posts an input of its own.
import AppKit
import Carbon.HIToolbox

struct RtsColour: Decodable {
    let r: Double, g: Double, b: Double
    var color: NSColor { NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: 1) }
}

struct RtsInput: Decodable {
    let label: String
    let colour: RtsColour
    let key: String?
    let button: String?
    let modifiers: [String]
    let input: String
}

struct RtsTarget: Decodable {
    struct Action: Decodable { let id: String, input: String }
    let id: String, kind: String
    let appearMs: Double, expireMs: Double
    let x: Double, y: Double, width: Double, height: Double
    let actions: [Action]
    var rect: CGRect { CGRect(x: x, y: y, width: width, height: height) }
}

struct RtsRound: Decodable {
    struct Canvas: Decodable { let width: Double, height: Double }
    struct Drawing: Decodable { let unit_grid: Int, unit_pt: Double, label_pt: Double }
    struct Schedule: Decodable { let lengthMs: Double; let targets: [RtsTarget] }
    let owner: String
    let seed: Int
    let canvas: Canvas
    let hud: Double
    let ground: RtsColour
    let drawing: Drawing
    let inputs: [String: RtsInput]
    let framesPerSecond: Int
    let schedule: Schedule
}

@MainActor
final class RtsArena: NSView {
    let round: RtsRound
    let log: Recorder
    var t0: UInt64?
    var displayed: RtsTarget?
    var shownAt: [String: UInt64] = [:]
    var completed: Set<String> = []
    var held: Set<String> = []
    var modifiers: [String] = []
    var misses: [String: Int] = [:]
    var drag: (id: String, start: CGPoint)?
    var frames = 0
    var events = 0
    var becameActive = false

    init(_ round: RtsRound, _ log: Recorder) {
        self.round = round
        self.log = log
        super.init(frame: CGRect(x: 0, y: 0, width: round.canvas.width, height: round.canvas.height))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("the fixture is built in code") }
    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    func tick() {
        if t0 == nil, let data = try? Data(contentsOf: log.folder.appendingPathComponent("start.json")),
           let start = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            t0 = (start["t0Ns"] as? NSNumber)?.uint64Value
        }
        needsDisplay = true
        displayIfNeeded()
    }

    override func draw(_ dirtyRect: NSRect) {
        round.ground.color.setFill()
        bounds.fill()
        // The legend gives a planner all task colours even before the count-in.
        let names = round.inputs.keys.sorted()
        let width = bounds.width / Double(names.count)
        for (index, name) in names.enumerated() {
            let input = round.inputs[name]!
            input.colour.color.setFill()
            CGRect(x: Double(index) * width, y: 0, width: width, height: round.hud).fill()
            (input.label as NSString).draw(at: CGPoint(x: Double(index) * width, y: 0), withAttributes: [
                .font: NSFont.systemFont(ofSize: round.drawing.label_pt), .foregroundColor: NSColor.black,
            ])
        }
        let now = uptimeNs()
        guard let t0, now >= t0 else { return }
        let ms = Double(now - t0) / 1_000_000
        displayed = round.schedule.targets.first {
            $0.appearMs <= ms && ms < $0.expireMs && !$0.actions.allSatisfy { completed.contains($0.id) }
        }
        if let target = displayed, let input = round.inputs[target.kind] {
            input.colour.color.setFill()
            target.rect.fill()
            if target.kind == "group" {
                round.ground.color.setFill()
                let count = round.drawing.unit_grid
                for row in 1...count {
                    for column in 1...count {
                        let x = target.x + Double(column) * target.width / Double(count + 1)
                        let y = target.y + Double(row) * target.height / Double(count + 1)
                        CGRect(x: x, y: y, width: round.drawing.unit_pt, height: round.drawing.unit_pt).fill()
                    }
                }
            }
            shownAt[target.id] = shownAt[target.id] ?? now
        }
        frames += 1
        log.frame(["seq": frames, "ns": now, "shown": displayed.map { [$0.id] } ?? []])
    }

    static func names(_ flags: NSEvent.ModifierFlags) -> [String] {
        [("cmd", NSEvent.ModifierFlags.command), ("ctrl", .control), ("opt", .option), ("shift", .shift)]
            .compactMap { flags.contains($0.1) ? $0.0 : nil }
    }

    func receive(_ kind: String, _ event: NSEvent) {
        let now = uptimeNs()
        let point = convert(event.locationInWindow, from: nil)
        let mods = Self.names(event.modifierFlags)
        var row: [String: Any] = ["kind": kind, "rxNs": now, "evNs": UInt64(max(0, event.timestamp) * 1_000_000_000),
                                  "x": point.x, "y": point.y, "button": event.buttonNumber,
                                  "sourcePid": event.cgEvent?.getIntegerValueField(.eventSourceUnixProcessID) ?? -1,
                                  "userData": event.cgEvent?.getIntegerValueField(.eventSourceUserData) ?? 0,
                                  "modifiers": mods]
        if kind == "keyDown" || kind == "keyUp" {
            row["keyCode"] = Int(event.keyCode)
            row["characters"] = event.charactersIgnoringModifiers ?? ""
            row["repeat"] = event.isARepeat
        }
        events += 1
        row["n"] = events
        var pairing: String?
        switch kind {
        case "down": if !held.insert("button:\(event.buttonNumber)").inserted { pairing = "double_down" }
        case "up": if held.remove("button:\(event.buttonNumber)") == nil { pairing = "unpaired_release" }
        case "keyDown": if !held.insert("key:\(event.keyCode)").inserted { pairing = "double_down" }
        case "keyUp": if held.remove("key:\(event.keyCode)") == nil { pairing = "unpaired_release" }
        default: break
        }
        if kind == "flags" || kind == "keyDown" || kind == "keyUp" { modifiers = mods }
        if pairing != nil || kind == "down" || kind == "keyDown" || (kind == "up" && drag != nil) {
            let verdict = pairing.map { ["miss": $0] } ?? judge(kind, event, point, mods)
            row["judged"] = verdict
            if let miss = verdict["miss"] { misses[miss, default: 0] += 1 }
        }
        log.event(row)
    }

    private func judge(_ kind: String, _ event: NSEvent, _ point: CGPoint, _ mods: [String]) -> [String: String] {
        guard let target = displayed, let wanted = round.inputs[target.kind] else { return ["miss": "empty"] }
        func hit(_ input: String) -> [String: String] {
            guard let action = target.actions.first(where: { $0.input == input }), !completed.contains(action.id)
            else { return ["miss": "duplicate"] }
            completed.insert(action.id)
            return ["hit": action.id, "input": input]
        }
        if kind == "keyDown" {
            guard let key = wanted.key, !event.isARepeat else { return ["miss": "key"] }
            // SDK virtual-key identities, judged independently of the helper's key map.
            let matches = (key == "a" && event.keyCode == kVK_ANSI_A) || (key == "1" && event.keyCode == kVK_ANSI_1)
            guard matches, mods == wanted.modifiers.sorted() else { return ["miss": "chord"] }
            if target.kind == "group", !completed.contains("\(target.id):drag") { return ["miss": "unselected"] }
            return hit(wanted.input)
        }
        if target.kind == "group" {
            guard event.buttonNumber == 0, mods.isEmpty else { return ["miss": "drag_button"] }
            if kind == "down" {
                guard point.x <= target.rect.minX, point.y <= target.rect.minY else { return ["miss": "box_start"] }
                drag = (target.id, point)
                return ["pending": target.id]
            }
            defer { drag = nil }
            guard let drag, drag.id == target.id, point.x >= target.rect.maxX, point.y >= target.rect.maxY,
                  drag.start.x < point.x, drag.start.y < point.y else { return ["miss": "box_end"] }
            return hit("drag")
        }
        guard kind == "down", let button = wanted.button, target.rect.contains(point),
              event.buttonNumber == (button == "right" ? 1 : 0), mods == wanted.modifiers.sorted()
        else { return ["miss": "click"] }
        return hit(wanted.input)
    }

    func flush(wait: Bool = false) {
        var state: [String: Any] = ["owner": round.owner, "seed": round.seed, "pid": Int(getpid()),
                                    "frames": frames, "events": events, "hits": completed.count, "misses": misses,
                                    "held": held.sorted() + modifiers, "becameActive": becameActive || NSApp.isActive]
        if let t0 { state["t0Ns"] = t0 }
        log.flush(state: state, wait: wait)
    }

    /// AppKit events delivered directly to the oracle: nothing is posted to
    /// the window server and no fixture window is opened by this test.
    func selfTest() {
        let now = ProcessInfo.processInfo.systemUptime
        func mouse(_ type: NSEvent.EventType, _ point: CGPoint, _ flags: NSEvent.ModifierFlags = []) -> NSEvent {
            let event = NSEvent.mouseEvent(with: type, location: CGPoint(x: point.x, y: bounds.height - point.y), modifierFlags: flags, timestamp: now,
                                          windowNumber: 0, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
            let cg = event.cgEvent!
            cg.setIntegerValueField(.mouseEventButtonNumber, value: type == .rightMouseDown || type == .rightMouseUp ? 1 : 0)
            return NSEvent(cgEvent: cg)!
        }
        func key(_ down: Bool, _ code: Int, _ flags: NSEvent.ModifierFlags) -> NSEvent {
            NSEvent.keyEvent(with: down ? .keyDown : .keyUp, location: .zero, modifierFlags: flags,
                            timestamp: now, windowNumber: 0, context: nil, characters: "",
                            charactersIgnoringModifiers: "", isARepeat: false, keyCode: UInt16(code))!
        }
        func control(_ down: Bool) -> NSEvent {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: CGKeyCode(kVK_Control), keyDown: down)!
            event.flags = down ? .maskControl : []
            return NSEvent(cgEvent: event)!
        }
        for kind in round.inputs.keys.sorted() {
            let target = round.schedule.targets.first { $0.kind == kind }!
            let wanted = round.inputs[kind]!
            displayed = target
            if kind == "group" {
                receive("keyDown", key(true, kVK_ANSI_1, .control))
                receive("keyUp", key(false, kVK_ANSI_1, []))
                precondition(misses["unselected"] == 1)
                misses.removeAll()
                receive("down", mouse(.leftMouseDown, CGPoint(x: target.rect.minX - 1, y: target.rect.minY - 1)))
                receive("up", mouse(.leftMouseUp, CGPoint(x: target.rect.maxX + 1, y: target.rect.maxY + 1)))
                precondition(completed.contains("\(target.id):drag"))
            }
            if let name = wanted.key {
                let code = name == "a" ? kVK_ANSI_A : kVK_ANSI_1
                let flags: NSEvent.ModifierFlags = wanted.modifiers.contains("ctrl") ? .control : []
                if !flags.isEmpty { receive("flags", control(true)) }
                receive("keyDown", key(true, code, flags))
                receive("keyUp", key(false, code, flags))
                if !flags.isEmpty { receive("flags", control(false)) }
            } else {
                let point = CGPoint(x: target.rect.midX, y: target.rect.midY)
                let flags: NSEvent.ModifierFlags = kind == "shift" ? .shift : kind == "cmd" ? .command : []
                receive("down", mouse(kind == "right" ? .rightMouseDown : .leftMouseDown, point, flags))
                receive("up", mouse(kind == "right" ? .rightMouseUp : .leftMouseUp, point, flags))
            }
            precondition(target.actions.allSatisfy { completed.contains($0.id) }, "\(kind) not received")
            precondition(held.isEmpty && modifiers.isEmpty && misses.isEmpty)
        }
        let shift = round.schedule.targets.first { $0.kind == "shift" }!
        displayed = shift
        let point = CGPoint(x: shift.rect.midX, y: shift.rect.midY)
        receive("down", mouse(.leftMouseDown, point))
        receive("up", mouse(.leftMouseUp, point))
        precondition(misses["click"] == 1, "a missing modifier is wrong input")
        receive("down", mouse(.leftMouseDown, point, .shift))
        receive("up", mouse(.leftMouseUp, point, .shift))
        precondition(misses["duplicate"] == 1, "a second press cannot earn another hit")
        receive("up", mouse(.leftMouseUp, point))
        precondition(misses["unpaired_release"] == 1, "every release belongs to a press")
        flush(wait: true)
        print("RTS oracle self-test passed (no input posted)")
    }

    override func mouseDown(with event: NSEvent) { receive("down", event) }
    override func rightMouseDown(with event: NSEvent) { receive("down", event) }
    override func mouseUp(with event: NSEvent) { receive("up", event) }
    override func rightMouseUp(with event: NSEvent) { receive("up", event) }
    override func mouseDragged(with event: NSEvent) { receive("drag", event) }
    override func mouseMoved(with event: NSEvent) { receive("move", event) }
}

@main
@MainActor
final class RtsFixture: NSObject, NSApplicationDelegate {
    var arena: RtsArena!
    var panel: NSPanel!
    var timers: [Timer] = []
    var lifetime: FixtureLifetime?
    var keyMonitor: Any?

    static func main() throws {
        var arguments = Array(CommandLine.arguments.dropFirst())
        let testing = arguments.first == "--self-test"
        if testing { arguments.removeFirst() }
        guard arguments.count == 2 else { exit(2) }
        let app = NSApplication.shared
        let delegate = RtsFixture()
        let round = try JSONDecoder().decode(RtsRound.self, from: Data(contentsOf: URL(fileURLWithPath: arguments[0])))
        delegate.arena = RtsArena(round, Recorder(folder: URL(fileURLWithPath: arguments[1], isDirectory: true)))
        if testing { delegate.arena.selfTest(); return }
        app.setActivationPolicy(.regular)
        app.delegate = delegate
        withExtendedLifetime(delegate) { app.run() }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let window = FixtureWindow(view: arena, owner: arena.round.owner, seed: arena.round.seed)
        panel = window.panel
        arena.log.write("ready.json", window.ready)
        lifetime = FixtureLifetime(flush: { [weak self] wait in self?.arena.flush(wait: wait) },
                                   becameActive: { [weak self] in self?.arena.becameActive = true })
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .flagsChanged]) { [weak self] event in
            MainActor.assumeIsolated {
                self?.arena.receive(event.type == .keyDown ? "keyDown" : event.type == .keyUp ? "keyUp" : "flags", event)
            }
            return nil
        }
        timers.append(Timer.scheduledTimer(withTimeInterval: 1 / Double(arena.round.framesPerSecond), repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.arena.tick() }
        })
    }
}

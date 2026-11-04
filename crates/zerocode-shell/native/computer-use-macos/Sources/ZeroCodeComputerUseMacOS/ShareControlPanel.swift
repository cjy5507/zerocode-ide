import AppKit
import ZeroCodeComputerUseMacOSCore

/// The shell's numbered share, plus a physical press not yet heard by it.
struct ShareControlState {
    private(set) var target: String?
    private(set) var generation = 0
    private(set) var revision = -1
    private(set) var takenBack = false
    private(set) var event = 0

    mutating func update(target: String?, generation: Int, revision: Int, takenBack: Bool = false) {
        guard revision > self.revision else { return }
        // A newer presentation is not a hand-back. Keep a physical press until
        // the window acknowledges it or replaces this share's ownership.
        let pendingPress = self.takenBack && self.generation == generation
        self.target = target
        self.generation = generation
        self.revision = revision
        self.takenBack = takenBack || pendingPress
        event += 1
    }

    mutating func takeControl(displayedRevision: Int) -> Bool {
        guard target != nil, !takenBack, displayedRevision == revision else { return false }
        takenBack = true
        event += 1
        return true
    }

    static func humanMouse(sourcePID: Int64, sourceUserData: Int64) -> Bool {
        sourcePID == 0 && sourceUserData == 0
    }
}

/// One app's control: the app in use it stands beside, and the window the
/// share last named for it (t-93286: every app in use has its own).
struct SharePanelSpec: Equatable {
    let target: String
    let windowID: UInt32?
    let windowIndex: Int?

    /// The controls a share notice asks for: one per app in `panels`, else one
    /// for its `target`; none when nothing is in use.
    static func list(_ params: [String: JSONValue]) -> [SharePanelSpec] {
        let window = { (fields: [String: JSONValue]) in
            (fields["windowId"]?.number.flatMap(UInt32.init(exactly:)), fields["windowIndex"]?.number.flatMap(Int.init(exactly:)))
        }
        guard case let .array(rows)? = params["panels"] else {
            guard let target = params["target"]?.string else { return [] }
            let (id, index) = window(params)
            return [SharePanelSpec(target: target, windowID: id, windowIndex: index)]
        }
        var seen = Set<String>()
        return rows.compactMap { row in
            guard case let .object(fields) = row, let target = fields["target"]?.string, seen.insert(target).inserted else { return nil }
            let (id, index) = window(fields)
            return SharePanelSpec(target: target, windowID: id, windowIndex: index)
        }
    }
}

/// The controls by app: one kept across notices, made for an app that joined
/// and closed for one that left, so the panels never outnumber the apps.
struct SharePanelRoster<Panel> {
    private(set) var panels: [String: Panel] = [:]

    mutating func reconcile(_ targets: [String], make: (String) -> Panel, close: (Panel) -> Void) {
        let wanted = Set(targets)
        for (target, panel) in panels where !wanted.contains(target) {
            panels[target] = nil
            close(panel)
        }
        for target in targets where panels[target] == nil {
            panels[target] = make(target)
        }
    }
}

/// The panels belong to the signed helper, never to the app being driven.
/// State and hit bounds are readable from input threads without the main queue.
@MainActor
final class ShareControlPanel {
    private static let shared = ShareControlPanel()
    nonisolated private static let lock = NSLock()
    nonisolated(unsafe) private static var state = ShareControlState()
    /// Where every control on the screen is: no agent input lands in any of them.
    nonisolated(unsafe) private static var hitBounds: [CGRect] = []
    nonisolated(unsafe) private static var displayedRevision: Int?
    private var roster = SharePanelRoster<ShareTargetControl>()
    private var timer: DispatchSourceTimer?
    private var label = ShareControlPanel.controlLabel("")
    nonisolated(unsafe) private static var observed: [pid_t: UInt32] = [:]

    nonisolated static func isHelperPID(_ pid: pid_t) -> Bool { pid == getpid() }

    nonisolated static func agentWindows(_ rows: [[String: Any]]) -> [[String: Any]] {
        rows.filter { ($0[kCGWindowOwnerPID as String] as? pid_t) != getpid() }
    }

    nonisolated static func refuseHelper(pid: pid_t) throws {
        if isHelperPID(pid) {
            throw ProviderError.coded("app_blocked", "the human control panel is never an agent target")
        }
    }

    nonisolated static func refusePoint(_ point: CGPoint) throws {
        lock.lock()
        let bounds = hitBounds
        lock.unlock()
        if bounds.contains(where: { $0.contains(point) }) {
            throw ProviderError.coded("app_blocked", "the human control panel cannot receive agent input")
        }
    }

    nonisolated static func refuseInput(method: String, params: [String: JSONValue]) throws {
        lock.lock()
        let bounds = hitBounds
        let taken = state.takenBack
        lock.unlock()
        try refuseInput(method: method, params: params, bounds: bounds, taken: taken)
    }

    /// The same admission against fixture bounds: no window or input API is read.
    nonisolated static func refuseInput(method: String, params: [String: JSONValue], bounds: [CGRect], taken: Bool) throws {
        if taken && Provider.actingMethods.contains(method) {
            throw ProviderError.coded("person_took_control", "the person took control; wait until they hand it back")
        }
        for (x, y) in (method == "mouseMove" ? [("x", "y")] : Provider.pointedMethods[method] ?? []) {
            let point = try desktopPoint(params, x, y)
            if bounds.contains(where: { $0.contains(point) }) {
                throw ProviderError.coded("app_blocked", "the human control panel cannot receive agent input")
            }
        }
    }

    nonisolated static func status() -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        return ["target": state.target as Any? ?? NSNull(), "generation": state.generation,
                "revision": state.revision, "event": state.event, "takenBack": state.takenBack,
                "controls": hitBounds.count]
    }

    nonisolated static func controlLabel(_ label: String) -> String {
        label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "Take control" : label
    }

    /// At rest the control is one round mark over a corner of the person's app;
    /// under their pointer it opens to say its name, in any of the five locales.
    nonisolated static func controlSize(labelWidth: CGFloat, open: Bool) -> CGSize {
        let mark = ShareTakeLook.mark
        return open ? CGSize(width: mark + ceil(labelWidth) + ShareTakeLook.labelLead, height: mark) : CGSize(width: mark, height: mark)
    }

    /// Top right of the window, 8 pt in: it opens leftwards, so the mark the
    /// person is pointing at stays under their pointer.
    nonisolated static func controlBounds(window: CGRect, size: CGSize) -> CGRect {
        CGRect(x: window.maxX - size.width - 8, y: window.minY + 8, width: size.width, height: size.height)
    }

    /// The window an app's control stands on, and where on it: the window the
    /// share named, else the one it already stands on, else the one last
    /// looked at, else the app's first. None when another ordinary window
    /// covers that corner: the control hides rather than lift the app.
    nonisolated static func placement(
        _ spec: SharePanelSpec, pinned: UInt32?, observed: UInt32?, windows: [WindowCandidate], rows: [[String: Any]], size: CGSize
    ) -> (windowId: UInt32, bounds: CGRect)? {
        let selected: WindowCandidate?
        if let windowID = spec.windowID { selected = windows.first { $0.windowId == windowID } }
        else if let pinned { selected = windows.first { $0.windowId == pinned } }
        else if let windowIndex = spec.windowIndex { selected = windows.indices.contains(windowIndex) ? windows[windowIndex] : nil }
        else if let observed { selected = windows.first { $0.windowId == observed } }
        else { selected = windows.first }
        guard let window = selected else { return nil }
        let bounds = controlBounds(window: window.bounds, size: size)
        let ahead = rows.prefix { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value != window.windowId }
        let covered = ahead.contains { row in
            guard (row[kCGWindowOwnerPID as String] as? pid_t) != getpid(),
                  (row[kCGWindowLayer as String] as? Int) == 0,
                  let raw = row[kCGWindowBounds as String] as? NSDictionary,
                  let frame = CGRect(dictionaryRepresentation: raw) else { return false }
            return frame.intersects(bounds)
        }
        return covered ? nil : (window.windowId, bounds)
    }

    private func pointer(_ control: ShareTargetControl, over: Bool) {
        guard control.open != over else { return }
        control.open = over
        refresh(easing: control.target)
    }

    nonisolated static func update(_ params: [String: JSONValue]) throws {
        let revision = try requiredInteger(params, "revision")
        let generation = try requiredInteger(params, "generation")
        lock.lock()
        guard revision >= state.revision else { lock.unlock(); return }
        let wasTaken = state.takenBack
        state.update(target: params["target"]?.string, generation: generation, revision: revision,
                     takenBack: params["takenBack"]?.bool == true)
        let cancel = !wasTaken && state.takenBack
        lock.unlock()
        if cancel { cancelInput() }
        let label = controlLabel(params["buttonLabel"]?.string ?? "")
        let specs = SharePanelSpec.list(params)
        DispatchQueue.main.async {
            lock.lock()
            let currentRevision = state.revision
            lock.unlock()
            guard currentRevision == revision else { return }
            if !label.isEmpty { shared.label = label }
            shared.present(specs)
            shared.refresh()
            if shared.timer == nil, stateNeedsPanel() {
                let timer = DispatchSource.makeTimerSource(queue: .main)
                timer.schedule(deadline: .now(), repeating: .milliseconds(250))
                timer.setEventHandler { MainActor.assumeIsolated { shared.refresh() } }
                timer.resume()
                shared.timer = timer
            }
        }
        OperatorGuardHost.shareChanged()
    }

    /// One control per app in use: an app that left closes its panel.
    private func present(_ specs: [SharePanelSpec]) {
        roster.reconcile(specs.map(\.target), make: { target in
            let control = ShareTargetControl(target: target)
            control.onPointer = { [weak self, weak control] over in
                guard let self, let control else { return }
                self.pointer(control, over: over)
            }
            return control
        }, close: { $0.close() })
        for spec in specs { roster.panels[spec.target]?.aim(spec) }
    }

    nonisolated private static func stateNeedsPanel() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return state.target != nil && !state.takenBack
    }

    nonisolated static func observedWindow(pid: pid_t, windowID: UInt32) {
        lock.lock()
        observed[pid] = windowID
        lock.unlock()
        DispatchQueue.main.async {
            var moved = false
            for control in shared.roster.panels.values {
                if control.observed(pid: pid, windowID: windowID) { moved = true }
            }
            if moved { shared.refresh() }
        }
    }

    /// Physical NSEvent only. All CU paths also reject this helper and its bounds.
    nonisolated static func takeControl(sourcePID: Int64, sourceUserData: Int64) {
        guard ShareControlState.humanMouse(sourcePID: sourcePID, sourceUserData: sourceUserData) else { return }
        lock.lock()
        let took = displayedRevision.map { state.takeControl(displayedRevision: $0) } ?? false
        lock.unlock()
        guard took else { return }
        cancelInput()
        OperatorGuardHost.shareChanged()
        DispatchQueue.main.async { shared.hide() }
    }

    nonisolated static func cancelInput() {
        OperatorHandHost.revokeForShare()
        BackgroundPointerOverlay.hide()
        ReflexRuntimeHost.operatorStopped(reason: "person_took_control")
    }

    private func refresh(easing: String? = nil) {
        Self.lock.lock()
        let current = Self.state
        Self.lock.unlock()
        guard current.target != nil, !current.takenBack else {
            hide()
            timer?.cancel()
            timer = nil
            return
        }
        let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]]
        Self.lock.lock()
        let observed = Self.observed
        Self.lock.unlock()
        // Each control is placed on its own window; one that cannot stand
        // there hides alone, before the others' bounds are published.
        var shown: [ShareTargetControl] = []
        for control in roster.panels.values {
            if let rows, control.place(label: label, rows: rows, observed: observed, easing: easing == control.target) {
                shown.append(control)
            } else {
                control.hide()
            }
        }
        Self.lock.lock()
        guard Self.state.revision == current.revision, !Self.state.takenBack else {
            Self.lock.unlock()
            hide()
            return
        }
        Self.hitBounds = shown.compactMap { $0.bounds }
        Self.displayedRevision = shown.isEmpty ? nil : current.revision
        for control in shown { control.orderFront() }
        Self.lock.unlock()
    }

    private func hide() {
        for control in roster.panels.values { control.hide() }
        Self.lock.lock()
        Self.hitBounds = []
        Self.displayedRevision = nil
        Self.lock.unlock()
    }
}

/// One app's control and the panel it is drawn in, kept while the app is in use.
@MainActor
final class ShareTargetControl {
    let target: String
    var onPointer: ((Bool) -> Void)?
    /// The person's pointer is on the control: it says its name.
    var open = false
    /// Where it stands on the screen while shown.
    private(set) var bounds: CGRect?
    private var spec: SharePanelSpec
    private var panel: ShareTakePanel?
    private var pinned: (pid_t, UInt32)?
    private var application: NSRunningApplication?
    /// The panel is on its way to a new width: `place` leaves its frame alone.
    private var resizing = false

    init(target: String) {
        self.target = target
        self.spec = SharePanelSpec(target: target, windowID: nil, windowIndex: nil)
    }

    /// The window the share names now; another than before is found afresh.
    func aim(_ spec: SharePanelSpec) {
        if self.spec.windowID != spec.windowID || self.spec.windowIndex != spec.windowIndex { pinned = nil }
        self.spec = spec
    }

    /// A look at another window of this app moves a control the share pinned to no window.
    func observed(pid: pid_t, windowID: UInt32) -> Bool {
        guard application?.processIdentifier == pid, spec.windowID == nil, pinned?.1 != windowID else { return false }
        pinned = nil
        return true
    }

    /// Put the control on its window, not yet ordered in front; false when it cannot stand there.
    func place(label: String, rows: [[String: Any]], observed: [pid_t: UInt32], easing: Bool) -> Bool {
        if application?.isTerminated != false {
            pinned = nil
            let query = target.lowercased()
            application = NSWorkspace.shared.runningApplications.sorted { $0.isActive && !$1.isActive }.first { app in
                guard app.processIdentifier != getpid(), !app.isTerminated else { return false }
                if "pid:\(app.processIdentifier)" == query { return true }
                let descriptor = AppDescriptor(name: app.localizedName ?? "", bundleId: app.bundleIdentifier,
                                               pid: app.processIdentifier, app: app)
                return applicationAnswers(to: query, name: descriptor.name, bundleId: descriptor.bundleId, otherNames: descriptor.otherNames)
            }
        }
        guard let app = application else { return false }
        let windows = WindowList.candidates(infos: rows, pid: app.processIdentifier).filter { $0.layer == 0 && $0.isOnScreen }
        if pinned?.0 != app.processIdentifier { pinned = nil }
        let size = ShareControlPanel.controlSize(labelWidth: ShareTakeLook.labelWidth(label), open: open)
        guard let placed = ShareControlPanel.placement(
            spec, pinned: pinned?.1, observed: observed[app.processIdentifier], windows: windows, rows: rows, size: size
        ) else { return false }
        let bounds = placed.bounds
        pinned = (app.processIdentifier, placed.windowId)
        let frame = CGRect(x: bounds.minX, y: CGDisplayBounds(CGMainDisplayID()).height - bounds.maxY,
                           width: bounds.width, height: bounds.height)
        let shown = panel ?? makePanel(frame: frame, label: label)
        if let button = shown.contentView as? NSButton { button.title = label; button.setAccessibilityLabel(label) }
        if easing, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            resizing = true
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = ShareTakeLook.openSeconds
                context.timingFunction = CAMediaTimingFunction(name: .easeOut)
                shown.animator().setFrame(frame, display: true)
            }, completionHandler: { [weak self, weak shown] in
                MainActor.assumeIsolated {
                    self?.resizing = false
                    shown?.invalidateShadow()
                }
            })
        } else if !resizing, shown.frame != frame {
            shown.setFrame(frame, display: true)
            shown.invalidateShadow()
        }
        self.bounds = bounds
        return true
    }

    private func makePanel(frame: CGRect, label: String) -> ShareTakePanel {
        let made = ShareTakePanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        made.hidesOnDeactivate = false
        made.isReleasedWhenClosed = false
        made.level = .floating
        made.collectionBehavior = [.transient, .ignoresCycle, .fullScreenAuxiliary]
        made.sharingType = .none
        made.isOpaque = false
        made.backgroundColor = .clear
        made.hasShadow = true
        let button = ShareTakeButton(title: label, target: nil, action: nil)
        button.onPointer = { [weak self] over in self?.onPointer?(over) }
        button.setAccessibilityLabel(label)
        made.contentView = button
        panel = made
        return made
    }

    func orderFront() { panel?.orderFrontRegardless() }

    func hide() {
        open = false
        bounds = nil
        panel?.orderOut(nil)
    }

    /// The app left the share: its panel goes, not only out of sight.
    func close() {
        hide()
        panel?.close()
        panel = nil
    }
}

@MainActor
final class ShareTakePanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

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

/// The panel belongs to the signed helper, never to the app being driven.
/// State and hit bounds are readable from input threads without the main queue.
@MainActor
final class ShareControlPanel {
    private static let shared = ShareControlPanel()
    nonisolated private static let lock = NSLock()
    nonisolated(unsafe) private static var state = ShareControlState()
    nonisolated(unsafe) private static var hitBounds: CGRect?
    nonisolated(unsafe) private static var displayedRevision: Int?
    private var panel: ShareTakePanel?
    private var timer: DispatchSourceTimer?
    private var pinned: (pid_t, UInt32)?
    private var label = ShareControlPanel.controlLabel("")
    private var windowID: UInt32?
    private var windowIndex: Int?
    private var presentedTarget: String?
    private var application: NSRunningApplication?
    nonisolated(unsafe) private static var observed: [pid_t: UInt32] = [:]
    /// The person's pointer is on the control: it says its name.
    private var open = false
    /// The panel is on its way to a new width: `refresh` leaves its frame alone.
    private var resizing = false
    private static let openSeconds = 0.14

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
        if bounds?.contains(point) == true {
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
    nonisolated static func refuseInput(method: String, params: [String: JSONValue], bounds: CGRect?, taken: Bool) throws {
        if taken && Provider.actingMethods.contains(method) {
            throw ProviderError.coded("person_took_control", "the person took control; wait until they hand it back")
        }
        for (x, y) in (method == "mouseMove" ? [("x", "y")] : Provider.pointedMethods[method] ?? []) {
            if let bounds, bounds.contains(try desktopPoint(params, x, y)) {
                throw ProviderError.coded("app_blocked", "the human control panel cannot receive agent input")
            }
        }
    }

    nonisolated static func status() -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        return ["target": state.target as Any? ?? NSNull(), "generation": state.generation,
                "revision": state.revision, "event": state.event, "takenBack": state.takenBack]
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

    fileprivate func pointer(over: Bool) {
        guard open != over else { return }
        open = over
        refresh(easing: true)
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
        let windowID = params["windowId"]?.number.flatMap(UInt32.init(exactly:))
        let windowIndex = params["windowIndex"]?.number.flatMap(Int.init(exactly:))
        DispatchQueue.main.async {
            lock.lock()
            let currentRevision = state.revision
            lock.unlock()
            guard currentRevision == revision else { return }
            if !label.isEmpty { shared.label = label }
            if shared.windowID != windowID || shared.windowIndex != windowIndex {
                shared.pinned = nil
            }
            shared.windowID = windowID
            shared.windowIndex = windowIndex
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
            guard shared.application?.processIdentifier == pid, shared.windowID == nil,
                  shared.pinned?.1 != windowID else { return }
            shared.pinned = nil
            shared.refresh()
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

    private func refresh(easing: Bool = false) {
        Self.lock.lock()
        let current = Self.state
        Self.lock.unlock()
        guard let target = current.target, !current.takenBack else {
            pinned = nil
            application = nil
            hide()
            timer?.cancel()
            timer = nil
            return
        }
        if presentedTarget != target {
            pinned = nil
            application = nil
            presentedTarget = target
        }
        if application?.isTerminated != false {
            application = NSWorkspace.shared.runningApplications.sorted { $0.isActive && !$1.isActive }.first { app in
                let query = target.lowercased()
                guard app.processIdentifier != getpid(), !app.isTerminated else { return false }
                if "pid:\(app.processIdentifier)" == query { return true }
                let descriptor = AppDescriptor(name: app.localizedName ?? "", bundleId: app.bundleIdentifier,
                                               pid: app.processIdentifier, app: app)
                return applicationAnswers(to: query, name: descriptor.name, bundleId: descriptor.bundleId, otherNames: descriptor.otherNames)
            }
        }
        guard let app = application,
              let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else { hide(); return }
        let windows = WindowList.candidates(infos: rows, pid: app.processIdentifier).filter { $0.layer == 0 && $0.isOnScreen }
        if pinned?.0 != app.processIdentifier { pinned = nil }
        Self.lock.lock()
        let observedID = Self.observed[app.processIdentifier]
        Self.lock.unlock()
        let selected: WindowCandidate?
        if let windowID { selected = windows.first { $0.windowId == windowID } }
        else if let pinned { selected = windows.first { $0.windowId == pinned.1 } }
        else if let windowIndex { selected = windows.indices.contains(windowIndex) ? windows[windowIndex] : nil }
        else if let observedID { selected = windows.first { $0.windowId == observedID } }
        else { selected = pinned.flatMap { pin in windows.first { $0.windowId == pin.1 } } ?? (pinned == nil ? windows.first : nil) }
        guard let window = selected else { hide(); return }
        pinned = (app.processIdentifier, window.windowId)
        let size = Self.controlSize(labelWidth: ShareTakeLook.labelWidth(label), open: open)
        let bounds = Self.controlBounds(window: window.bounds, size: size)
        // Hide when another ordinary window covers the corner. Never lift the target.
        let ahead = rows.prefix { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value != window.windowId }
        if ahead.contains(where: { row in
            guard (row[kCGWindowOwnerPID as String] as? pid_t) != getpid(),
                  (row[kCGWindowLayer as String] as? Int) == 0,
                  let raw = row[kCGWindowBounds as String] as? NSDictionary,
                  let frame = CGRect(dictionaryRepresentation: raw) else { return false }
            return frame.intersects(bounds)
        }) { hide(); return }
        let frame = CGRect(x: bounds.minX, y: CGDisplayBounds(CGMainDisplayID()).height - bounds.maxY,
                           width: bounds.width, height: bounds.height)
        let shown: ShareTakePanel
        if let panel { shown = panel } else {
            shown = ShareTakePanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            shown.hidesOnDeactivate = false
            shown.isReleasedWhenClosed = false
            shown.level = .floating
            shown.collectionBehavior = [.transient, .ignoresCycle, .fullScreenAuxiliary]
            shown.sharingType = .none
            shown.isOpaque = false
            shown.backgroundColor = .clear
            shown.hasShadow = true
            let button = ShareTakeButton(title: label, target: nil, action: nil)
            button.onPointer = { [weak self] over in self?.pointer(over: over) }
            button.setAccessibilityLabel(label)
            shown.contentView = button
            panel = shown
        }
        if let button = shown.contentView as? NSButton { button.title = label; button.setAccessibilityLabel(label) }
        if easing, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            resizing = true
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = Self.openSeconds
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
        Self.lock.lock()
        guard Self.state.revision == current.revision, !Self.state.takenBack else {
            Self.lock.unlock()
            hide()
            return
        }
        Self.hitBounds = bounds
        Self.displayedRevision = current.revision
        shown.orderFrontRegardless()
        Self.lock.unlock()
    }

    private func hide() {
        open = false
        panel?.orderOut(nil)
        Self.lock.lock()
        Self.hitBounds = nil
        Self.displayedRevision = nil
        Self.lock.unlock()
    }
}

@MainActor
final class ShareTakePanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

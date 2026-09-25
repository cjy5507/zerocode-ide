import AppKit
@preconcurrency import ApplicationServices
import Carbon.HIToolbox
import CoreGraphics
import Darwin
import Foundation
import ImageIO
import ZeroCodeComputerUseMacOSCore
import ScreenCaptureKit

private let providerName = "zerocode-computer-use-macos"
private let providerVersion = "1.0.0"
private let providerProtocolVersion = 2

struct Request: Decodable {
    let id: Int
    let method: String
    let params: [String: JSONValue]?
    let token: String?
}

enum JSONValue: Decodable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: JSONValue].self))
        }
    }

    var string: String? {
        if case let .string(value) = self { return value }
        return nil
    }

    var number: Double? {
        if case let .number(value) = self { return value }
        return nil
    }

    var bool: Bool? {
        if case let .bool(value) = self { return value }
        return nil
    }
}

enum ProviderError: Error {
    case coded(String, String)

    var code: String {
        switch self {
        case let .coded(code, _):
            return code
        }
    }

    var message: String {
        switch self {
        case let .coded(_, message):
            return message
        }
    }
}

struct AppDescriptor {
    let name: String
    let bundleId: String?
    let pid: pid_t
    let app: NSRunningApplication

    /// The other names it answers to — its bundle's own words and its
    /// executable (`applicationOtherNames`) — read off the bundle only when
    /// asked, which `matches` does after the name and the identifier miss.
    var otherNames: [String] {
        applicationOtherNames(
            info: app.bundleURL.flatMap { Bundle(url: $0) }?.infoDictionary,
            executableURL: app.executableURL
        )
    }

    var needsManualAccessibilityMode: Bool {
        // Chromium/Electron apps often need this private AX mode, but applying it
        // broadly can corrupt native Cocoa app trees into app-root-only nodes.
        guard let bundleId = bundleId?.lowercased() else {
            return false
        }
        return bundleId.hasPrefix("com.google.chrome") ||
            bundleId.hasPrefix("com.microsoft.edgemac") ||
            bundleId.hasPrefix("com.brave.browser") ||
            bundleId.hasPrefix("com.operasoftware.opera") ||
            bundleId.hasPrefix("com.vivaldi.vivaldi") ||
            bundleId == "com.github.electron" ||
            bundleId == "com.tinyspeck.slackmacgap" ||
            bundleId == "com.spotify.client" ||
            bundleId == "com.hnc.discord" ||
            bundleId == "com.microsoft.teams2" ||
            bundleId == "notion.id"
    }

    var isKnownBrowser: Bool {
        let bundle = bundleId?.lowercased() ?? ""
        let appName = name.lowercased()
        return bundle == "com.apple.safari" ||
            bundle == "org.mozilla.firefox" ||
            bundle == "company.thebrowser.browser" ||
            bundle == "app.zen-browser.zen" ||
            bundle.hasPrefix("com.google.chrome") ||
            bundle.hasPrefix("com.microsoft.edgemac") ||
            bundle.hasPrefix("com.brave.browser") ||
            bundle.hasPrefix("com.operasoftware.opera") ||
            bundle.hasPrefix("com.vivaldi.vivaldi") ||
            appName == "safari" ||
            appName == "firefox" ||
            appName == "arc" ||
            appName == "zen" ||
            appName.contains("chrome") ||
            appName.contains("chromium") ||
            appName.contains("edge") ||
            appName.contains("brave") ||
            appName.contains("opera") ||
            appName.contains("vivaldi")
    }
}

final class ElementRecord {
    let index: Int
    let element: AXUIElement
    let localFrame: CGRect?
    let actions: [String]
    let signature: String
    /// The face a mark is drawn from (`snapshot.elements`): the role, the
    /// line's name (a row's text), the placeholder and the traits.
    let role: String
    let name: String?
    let placeholder: String?
    let traits: [String]
    /// The frame cut by every clipping container above it (`MarkPin.clipRoles`),
    /// and the nearest named element above it (the row a star sits in).
    let visible: CGRect?
    let context: String?

    init(
        index: Int,
        element: AXUIElement,
        localFrame: CGRect?,
        actions: [String],
        signature: String,
        role: String = "",
        name: String? = nil,
        placeholder: String? = nil,
        traits: [String] = [],
        visible: CGRect? = nil,
        context: String? = nil
    ) {
        self.index = index
        self.element = element
        self.localFrame = localFrame
        self.actions = actions
        self.signature = signature
        self.role = role
        self.name = name
        self.placeholder = placeholder
        self.traits = traits
        self.visible = visible
        self.context = context
    }

    /// The words a mark's pin holds to: the name, else the placeholder.
    var words: String? { name ?? placeholder }
}

struct Snapshot {
    let id: String
    let app: AppDescriptor
    let windowTitle: String
    let windowBounds: CGRect
    let windowId: CGWindowID
    let windowLayer: Int
    let treeText: String
    let focusedElementId: Int?
    let screenshot: ScreenshotPayload?
    let screenshotStatus: ScreenshotStatus
    let screenshotScale: CGSize
    let screenshotEngine: String?
    let elements: [Int: ElementRecord]
    let truncated: Bool
    let maxDepthReached: Bool

    func withoutScreenshotPayload() -> Snapshot {
        Snapshot(
            id: id,
            app: app,
            windowTitle: windowTitle,
            windowBounds: windowBounds,
            windowId: windowId,
            windowLayer: windowLayer,
            treeText: treeText,
            focusedElementId: focusedElementId,
            screenshot: nil,
            screenshotStatus: .skipped,
            screenshotScale: CGSize(width: 1, height: 1),
            screenshotEngine: nil,
            elements: elements,
            truncated: truncated,
            maxDepthReached: maxDepthReached
        )
    }
}

struct ScreenshotPayload {
    let data: String
    let width: Int
    let height: Int
    let scale: Double
}

struct CapturedImage {
    let image: CGImage
    let engine: String
}

enum ScreenshotStatus {
    case captured
    case skipped
    case failed(String)
}

private struct CachedSnapshotEntry {
    let snapshotId: String
    let keys: [String]
    let createdAt: Date
}

final class Provider {
    private var snapshots: [String: Snapshot] = [:]
    private var snapshotEntries: [CachedSnapshotEntry] = []

    /// The methods that change something — a stopped operator refuses these
    /// and still answers every look. Mirrors `ComputerMethod::acts` in
    /// `zerocode-core::computer_use`.
    static let actingMethods: Set<String> = [
        "click", "performSecondaryAction", "setValue", "typeText", "pressKey", "hotkey", "pasteText", "scroll", "drag",
        "mouseMove", "mouseClick", "mouseDrag", "mouseScroll", "key", "holdKey", "type",
        "launchApp", "quitApp", "activateApp", "openTarget", "windowAction", "clipboardWrite",
    ]

    /// The desktop input methods that must not land on ZeroCode's own window
    /// (§1.6), and where their point is.
    static let pointedMethods: [String: [(String, String)]] = [
        "mouseClick": [("x", "y")],
        "mouseScroll": [("x", "y")],
        "mouseDrag": [("fromX", "fromY"), ("toX", "toY")],
    ]
    static let keyedMethods: Set<String> = ["key", "holdKey", "type"]

    func handle(method: String, params: [String: JSONValue]) throws -> Any {
        guard Self.actingMethods.contains(method) else { return try dispatch(method: method, params: params) }
        // One hand (realtime v1 §5.4): an acting request holds it from its
        // count to its last event — never beside a reflex run — and a stop
        // lets go of whatever it pressed.
        return try OperatorHandHost.whileHeld {
            try OperatorGuardHost.admit()
            // What repaints from here on may be this act's doing (§7.1 eye).
            ScreenEye.markAct()
            return try dispatch(method: method, params: params)
        }
    }

    private func dispatch(method: String, params: [String: JSONValue]) throws -> Any {
        // Confirmation and execution share one fresh dispatch snapshot. It
        // is local to this request: a confirmed retry must observe anew.
        var dispatchSnapshot: Snapshot?
        func snapshotForDispatch() throws -> Snapshot {
            if let snapshot = dispatchSnapshot { return snapshot }
            let snapshot = try currentSnapshot(params: params)
            dispatchSnapshot = snapshot
            return snapshot
        }
        // The last step (§1.5): a press that lands on a payment, transfer or
        // delete control is held for the person unless the window already
        // carries their answer.
        if params["confirmed"]?.bool != true,
           let words = confirmGuardWords(params["confirmGuard"]),
           let target = try confirmTarget(method: method, params: params, snapshot: snapshotForDispatch),
           let kind = confirmKind(of: target, words: words) {
            throw ProviderError.coded("confirmation_required", "\(kind): \(target)")
        }
        if params["allowSelf"]?.bool != true {
            for (xKey, yKey) in Self.pointedMethods[method] ?? [] {
                try DesktopSelf.refuseOwnWindow(at: try desktopPoint(params, xKey, yKey))
            }
            if Self.keyedMethods.contains(method) {
                try DesktopSelf.refuseOwnFrontmost()
            }
        }
        // An action that names an app or a window acts in it, and ZeroCode's
        // own is never one (§1.6), whatever the verb: a press, a key or a
        // close there could answer the person's own question for them.
        if Self.actingMethods.contains(method) {
            try refuseOwnTarget(params)
        }
        switch method {
        case "listenStart":
            guard let config = soundSenseConfig(params) else {
                throw ProviderError.coded("invalid_argument", "listenStart needs the window's sound table")
            }
            let app = try params["app"]?.string.map { try resolveApp($0) }
            return try SoundListener.start(config: config, app: app)
        case "listenStop":
            return SoundListener.stop()
        case "eyeStart":
            guard let config = eyeConfig(params) else {
                throw ProviderError.coded("invalid_argument", "eyeStart needs the window's eye table")
            }
            return try ScreenEye.start(config: config, displayIndex: try optionalInteger(params, "display"))
        case "eyeChanges":
            return try ScreenEye.changes(
                displayIndex: try optionalInteger(params, "display"),
                after: try optionalInteger(params, "after"),
                fromMs: try optionalInteger(params, "fromMs").map(Int64.init)
            )
        case "eyeFrame":
            return try ScreenEye.frame(displayIndex: try optionalInteger(params, "display"))
        case "soundRead":
            return try SoundListener.read(after: Int(params["after"]?.number ?? 0))
        case "reflexStart":
            guard case let .object(table)? = params["eye"], let eye = eyeConfig(table) else {
                throw ProviderError.coded("invalid_argument", "reflexStart needs the window's eye table")
            }
            return try ReflexRuntimeHost.start(
                runId: try reflexRunId(params),
                plan: Data(try requiredString(params, "plan").utf8),
                limits: Data(try requiredString(params, "limits").utf8),
                perception: Data(try requiredString(params, "perception").utf8),
                runPolicy: Data(try requiredString(params, "runPolicy").utf8),
                capability: Data(try requiredString(params, "capability").utf8),
                eye: eye,
                display: try requiredInteger(params, "display"),
                hand: OperatorHandHost.hand,
                admit: { OperatorGuardHost.admission() },
                standing: { OperatorGuardHost.standing() },
                actingScope: { try actingScope($0) }
            )
        case "resume":
            OperatorGuardHost.resume(resetBudget: params["resetBudget"]?.bool == true)
            return OperatorGuardHost.status()
        case "handshake":
            return providerHandshake()
        case "listApps":
            return ["apps": listApps().map(renderListedApp)]
        case "listWindows":
            return try listWindows(params: params)
        case "getAppState":
            // Only a look asks for the faces a marked look plans from; an
            // action's answer never carries them.
            return try renderSnapshot(observe(params: params), elementFrames: params[MarkPin.elementFramesKey]?.bool == true)
        case "click":
            return try actionResult(params: params) { try click(params: params, snapshot: snapshotForDispatch()) }
        case "performSecondaryAction":
            return try actionResult(params: params) { try performSecondaryAction(params: params, snapshot: snapshotForDispatch()) }
        case "setValue":
            return try actionResult(params: params) { try setValue(params: params) }
        case "typeText":
            return try actionResult(params: params) { try typeText(params: params) }
        case "pressKey":
            return try actionResult(params: params) { try pressKey(params: params) }
        case "hotkey":
            return try actionResult(params: params) { try hotkey(params: params) }
        case "pasteText":
            return try actionResult(params: params) { try pasteText(params: params) }
        case "scroll":
            return try actionResult(params: params) { try scroll(params: params) }
        case "drag":
            return try actionResult(params: params) { try drag(params: params) }
        // The desktop, no app named (docs/design/computer-use-full-operator.md §2.1).
        case "screenshotDesktop":
            return try DesktopScreen.capture(params: params)
        case "displays":
            return ["displays": DesktopScreen.displays().enumerated().map { DesktopScreen.render($1, index: $0) }]
        case "cursorPosition":
            let point = Input.desktopCursorPosition()
            return ["x": point.x, "y": point.y]
        case "mouseMove":
            let target = try desktopPoint(params, "x", "y")
            let steps = try optionalInteger(params, "steps") ?? 1
            if steps > 1 {
                // One round trip, many events: the helper glides at its own cadence
                // so a human-paced move costs the caller one call, not one per step.
                try Input.desktopMove(to: target, steps: steps)
                return ["path": "synthetic", "steps": steps]
            }
            try Input.desktopMove(to: target)
            return ["path": "synthetic"]
        case "mouseClick":
            let count = try positiveInteger(params["clickCount"]?.number, defaultValue: 1, name: "clickCount")
            guard count <= SyntheticMouseClickDelivery.maxClickCount else {
                throw ProviderError.coded("invalid_argument", "clickCount must be at most \(SyntheticMouseClickDelivery.maxClickCount)")
            }
            try Input.desktopClick(
                at: try desktopPoint(params, "x", "y"),
                button: try mouseButton(params["mouseButton"]?.string),
                count: count,
                modifiers: try KeyMap.parseModifiers(params["modifiers"]?.string)
            )
            return ["path": "synthetic", "clickCount": count]
        case "mouseDrag":
            let steps = try optionalInteger(params, "steps") ?? Input.desktopDragSteps
            try Input.desktopDrag(from: try desktopPoint(params, "fromX", "fromY"), to: try desktopPoint(params, "toX", "toY"), steps: steps)
            return ["path": "synthetic", "steps": steps]
        case "mouseScroll":
            let dx = Int32(params["dx"]?.number ?? 0)
            let dy = Int32(params["dy"]?.number ?? 0)
            try Input.desktopScroll(at: try desktopPoint(params, "x", "y"), dx: dx, dy: dy)
            return ["path": "synthetic", "dx": dx, "dy": dy]
        case "key":
            let key = try requiredString(params, "key")
            let secureInput = try gateDesktopChord(key)
            try Input.pressKey(key, pid: 0)
            return withSecureInput(["path": "synthetic"], secureInput)
        case "holdKey":
            let ms = try requiredInteger(params, "ms")
            let key = try requiredString(params, "key")
            let secureInput = try gateDesktopChord(key)
            try Input.desktopHoldKey(key, milliseconds: ms)
            return withSecureInput(["path": "synthetic", "ms": ms], secureInput)
        case "type":
            let text = try requiredStringAllowingEmpty(params, "text")
            let (verification, secureInput) = try typeWatched(text, pid: 0, receiver: nil, config: keyboardGuardConfig(params))
            return withSecureInput(["path": "synthetic", "verification": verification], secureInput)
        // Apps, windows and the system (§2.2).
        case "launchApp":
            return try DesktopApps.launch(params: params, running: listApps())
        case "quitApp":
            return try DesktopApps.quit(app: try resolveApp(try requiredString(params, "app")), force: params["force"]?.bool == true)
        case "activateApp":
            return try DesktopApps.activate(app: try resolveApp(try requiredString(params, "app")))
        case "openTarget":
            return try DesktopApps.open(params: params, running: listApps())
        case "listAllWindows":
            return ["windows": DesktopWindows.listAll(everyLayer: params[MarkPin.everyLayerKey]?.bool == true)]
        case "windowAction":
            return try DesktopWindows.act(params: params)
        case "clipboardRead":
            return DesktopClipboard.read()
        case "clipboardWrite":
            DesktopClipboard.write(try requiredStringAllowingEmpty(params, "text"))
            return ["written": true]
        // The meaning layer (§2.3).
        case "findElements":
            return try findElements(params: params)
        case "readText":
            return try readText(params: params)
        default:
            throw ProviderError.coded("invalid_argument", "unknown method '\(method)'")
        }
    }

    private func actionResult(params: [String: JSONValue], action runAction: () throws -> [String: Any]) throws -> [String: Any] {
        var action = try runAction()
        do {
            return try renderActionResult(action: action, snapshot: observe(params: params))
        } catch let error as ProviderError where (error.code == "window_not_found" || error.code == "window_stale") && hasRequestedWindowSelector(params) {
            var fallbackParams = params
            fallbackParams.removeValue(forKey: "windowId")
            fallbackParams.removeValue(forKey: "windowIndex")
            if action["verification"] == nil {
                action["verification"] = ["state": "unverified", "reason": "window_changed"]
            }
            return try renderActionResult(action: action, snapshot: observe(params: fallbackParams))
        }
    }

    private func observe(params: [String: JSONValue]) throws -> Snapshot {
        let query = try requiredString(params, "app")
        let windowId = try requestedWindowId(params)
        let windowIndex = try requestedWindowIndex(params)
        let app = try resolveApp(query)
        if params["restoreWindow"]?.bool == true {
            recoverWindow(app)
        }
        let snapshot = try buildSnapshot(
            app: app,
            includeScreenshot: params["noScreenshot"]?.bool != true,
            windowId: windowId,
            windowIndex: windowIndex,
            restoreWindow: params["restoreWindow"]?.bool == true
        )
        // Why: cached snapshots only validate element identity for follow-up
        // actions; retaining MB-scale screenshot base64 in the long-lived agent grows memory.
        rememberSnapshot(
            query: query,
            app: app,
            snapshot: snapshot.withoutScreenshotPayload(),
            params: params,
            windowIndex: windowIndex
        )
        return snapshot
    }

    private func rememberSnapshot(
        query: String,
        app: AppDescriptor,
        snapshot cachedSnapshot: Snapshot,
        params: [String: JSONValue],
        windowIndex: Int?
    ) {
        let keys = [query, app.name, app.bundleId ?? "", "pid:\(app.pid)"]
            .filter { !$0.isEmpty }
            .map { $0.lowercased() }
        let namespace = snapshotNamespace(params)
        var storedKeys: [String] = []
        let canonicalWindowKey = snapshotCanonicalWindowIdKey(cachedSnapshot.windowId)
        if !isExplicitSnapshotNamespace(namespace) {
            snapshots[canonicalWindowKey.lowercased()] = cachedSnapshot
            storedKeys.append(canonicalWindowKey.lowercased())
        }
        snapshots[namespacedSnapshotKey(namespace, canonicalWindowKey)] = cachedSnapshot
        storedKeys.append(namespacedSnapshotKey(namespace, canonicalWindowKey))
        if let windowIndex {
            let canonicalWindowIndexKey = snapshotCanonicalWindowIndexKey(windowIndex)
            if !isExplicitSnapshotNamespace(namespace) {
                snapshots[canonicalWindowIndexKey.lowercased()] = cachedSnapshot
                storedKeys.append(canonicalWindowIndexKey.lowercased())
            }
            snapshots[namespacedSnapshotKey(namespace, canonicalWindowIndexKey)] = cachedSnapshot
            storedKeys.append(namespacedSnapshotKey(namespace, canonicalWindowIndexKey))
        }
        for key in keys {
            if !isExplicitSnapshotNamespace(namespace) {
                snapshots[key] = cachedSnapshot
                storedKeys.append(key)
                snapshots[snapshotWindowKey(key, cachedSnapshot.windowId)] = cachedSnapshot
                storedKeys.append(snapshotWindowKey(key, cachedSnapshot.windowId))
                if let windowIndex {
                    snapshots[snapshotWindowIndexKey(key, windowIndex)] = cachedSnapshot
                    storedKeys.append(snapshotWindowIndexKey(key, windowIndex))
                }
            }
            snapshots[namespacedSnapshotKey(namespace, key)] = cachedSnapshot
            storedKeys.append(namespacedSnapshotKey(namespace, key))
            let namespacedWindowKey = namespacedSnapshotKey(
                namespace,
                snapshotWindowKey(key, cachedSnapshot.windowId)
            )
            snapshots[namespacedWindowKey] = cachedSnapshot
            storedKeys.append(namespacedWindowKey)
            if let windowIndex {
                let namespacedWindowIndexKey = namespacedSnapshotKey(
                    namespace,
                    snapshotWindowIndexKey(key, windowIndex)
                )
                snapshots[namespacedWindowIndexKey] = cachedSnapshot
                storedKeys.append(namespacedWindowIndexKey)
            }
        }
        snapshotEntries.append(
            CachedSnapshotEntry(snapshotId: cachedSnapshot.id, keys: storedKeys, createdAt: Date())
        )
        pruneSnapshotCache()
    }

    private func pruneSnapshotCache() {
        let now = Date()
        while let oldest = snapshotEntries.first,
              ComputerSnapshotCachePolicy.shouldPrune(
                  entryCount: snapshotEntries.count,
                  createdAt: oldest.createdAt,
                  now: now
              ) {
            let expired = snapshotEntries.removeFirst()
            for key in expired.keys where snapshots[key]?.id == expired.snapshotId {
                snapshots.removeValue(forKey: key)
            }
        }
    }

    private func currentSnapshot(params: [String: JSONValue]) throws -> Snapshot {
        pruneSnapshotCache()
        let cached = try cachedSnapshot(params: params)
        // Why: cached AX frames can be stale after a window move or resize, and
        // stale geometry can turn an intended action into a misclick.
        let snapshot = try observe(params: params.merging(["noScreenshot": .bool(true)]) { _, replacement in replacement })
        try validateRequestedElements(cached: cached, current: snapshot, params: params)
        return snapshot
    }

    private func currentKeyboardSnapshot(params: [String: JSONValue]) throws -> Snapshot {
        // Why: AX text replacement/select-all do not post global input, so only
        // synthetic fallback paths require the target window to be focused.
        try currentSnapshot(params: params.merging(["noScreenshot": .bool(true)]) { _, replacement in replacement })
    }

    private func cachedSnapshot(params: [String: JSONValue]) throws -> Snapshot? {
        guard let query = params["app"]?.string, !query.isEmpty else { return nil }
        let namespace = snapshotNamespace(params)
        if let targetWindowId = try requestedWindowId(params) {
            let canonicalKey = snapshotCanonicalWindowIdKey(targetWindowId)
            if let cached = snapshots[namespacedSnapshotKey(namespace, canonicalKey)] {
                return cached
            }
            if !isExplicitSnapshotNamespace(namespace), let cached = snapshots[canonicalKey.lowercased()] {
                return cached
            }
            let windowKey = snapshotWindowKey(query.lowercased(), targetWindowId)
            if let cached = snapshots[namespacedSnapshotKey(namespace, windowKey)] {
                return cached
            }
            if !isExplicitSnapshotNamespace(namespace), let cached = snapshots[windowKey] {
                return cached
            }
            return nil
        }
        if let targetWindowIndex = try requestedWindowIndex(params) {
            let canonicalKey = snapshotCanonicalWindowIndexKey(targetWindowIndex)
            if let cached = snapshots[namespacedSnapshotKey(namespace, canonicalKey)] {
                return cached
            }
            if !isExplicitSnapshotNamespace(namespace), let cached = snapshots[canonicalKey.lowercased()] {
                return cached
            }
            let windowKey = snapshotWindowIndexKey(query.lowercased(), targetWindowIndex)
            if let cached = snapshots[namespacedSnapshotKey(namespace, windowKey)] {
                return cached
            }
            if !isExplicitSnapshotNamespace(namespace), let cached = snapshots[windowKey] {
                return cached
            }
            return nil
        }
        let key = query.lowercased()
        return snapshots[namespacedSnapshotKey(namespace, key)] ??
            (isExplicitSnapshotNamespace(namespace) ? nil : snapshots[key])
    }

    private func validateRequestedElements(cached: Snapshot?, current: Snapshot, params: [String: JSONValue]) throws {
        // A click by a mark's number: the pin is the proof, so no cached
        // snapshot is needed — the marked look may be filed anywhere.
        if let pin = try markPin(params) {
            let index = try requiredInteger(params, "elementIndex")
            let record = current.elements[index]
            guard MarkPin.holds(
                expectedSignature: pin.signature,
                expectedName: pin.name,
                expectedContext: pin.context,
                expectedFrame: pin.frame,
                actualSignature: record?.signature,
                actualName: record?.words,
                actualContext: record?.context,
                actualFrame: record?.localFrame,
                tolerance: pin.tolerance
            ) else {
                throw ProviderError.coded(
                    "element_not_found",
                    "element \(index) is no longer the control its mark was drawn on (it moved or changed since that look); look again with --marks"
                )
            }
            return
        }
        let requestedIndexes = try ["elementIndex", "fromElementIndex", "toElementIndex"].compactMap { key -> Int? in
            guard params[key]?.number != nil else { return nil }
            return try optionalInteger(params, key)
        }
        guard !requestedIndexes.isEmpty else { return }
        guard let cached else {
            throw ProviderError.coded("element_not_found", "element indexes require a fresh get-app-state snapshot for this app/window")
        }
        for index in requestedIndexes {
            guard let expected = cached.elements[index], let actual = current.elements[index] else {
                throw ProviderError.coded("element_not_found", "element \(index) is stale; run get-app-state again and use a fresh element index")
            }
            guard expected.signature == actual.signature else {
                throw ProviderError.coded("element_not_found", "element \(index) changed since the last snapshot; run get-app-state again and use a fresh element index")
            }
        }
    }

    /// A request's pin, whole or refused (`MarkPin.parse`).
    private func markPin(_ params: [String: JSONValue]) throws -> MarkPin.Parsed? {
        let keys = [MarkPin.signatureKey, MarkPin.nameKey, MarkPin.contextKey, MarkPin.frameKey, MarkPin.toleranceKey]
        var frame: [String: Double]?
        if case let .object(box)? = params[MarkPin.frameKey] {
            frame = box.compactMapValues(\.number)
        }
        do {
            return try MarkPin.parse(
                signature: params[MarkPin.signatureKey]?.string,
                name: params[MarkPin.nameKey]?.string,
                context: params[MarkPin.contextKey]?.string,
                frame: frame,
                tolerance: params[MarkPin.toleranceKey]?.number,
                present: keys.filter { params[$0] != nil }.count
            )
        } catch MarkPin.ParseError.invalid(let message) {
            throw ProviderError.coded("invalid_argument", message)
        } catch {
            throw ProviderError.coded("invalid_argument", "a mark's pin needs \(keys.joined(separator: ", ")) together")
        }
    }

    private func ensureWindowStillAvailable(_ snapshot: Snapshot) throws {
        guard WindowCapture.candidates(pid: snapshot.app.pid).contains(where: { $0.windowId == snapshot.windowId }) else {
            throw ProviderError.coded("window_stale", "window \(Int(snapshot.windowId)) is no longer available; run get-app-state again to refresh the target window")
        }
    }

    private func listApps() -> [AppDescriptor] {
        var seen = Set<String>()
        return NSWorkspace.shared.runningApplications
            .filter { !$0.isTerminated && $0.activationPolicy == .regular }
            .compactMap { app in
                guard let name = app.localizedName, !name.isEmpty else { return nil }
                let pid = app.processIdentifier
                guard pid > 0, pidIsLive(pid) else { return nil }
                let key = (app.bundleIdentifier ?? "pid:\(pid)").lowercased()
                guard seen.insert(key).inserted else { return nil }
                return AppDescriptor(name: name, bundleId: app.bundleIdentifier, pid: pid, app: app)
            }
            .sorted { lhs, rhs in
                if lhs.app.isActive != rhs.app.isActive {
                    return lhs.app.isActive && !rhs.app.isActive
                }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
    }

    private func renderListedApp(_ app: AppDescriptor) -> [String: Any] {
        [
            "name": app.name,
            "bundleId": jsonNullable(app.bundleId),
            "pid": Int(app.pid),
            "isRunning": true,
            "lastUsedAt": NSNull(),
            "useCount": NSNull(),
        ]
    }

    private func providerHandshake() -> [String: Any] {
        [
            "platform": "darwin",
            "provider": providerName,
            "providerVersion": providerVersion,
            "protocolVersion": providerProtocolVersion,
            "supports": [
                "apps": [
                    "list": true,
                    "bundleIds": true,
                    "pids": true,
                ],
                "windows": [
                    "list": true,
                    "targetById": true,
                    "targetByIndex": true,
                    "focus": false,
                    "moveResize": false,
                ],
                "observation": [
                    "screenshot": true,
                    "annotatedScreenshot": false,
                    "elementFrames": true,
                    "ocr": false,
                ],
                "actions": [
                    "click": true,
                    "typeText": true,
                    "pressKey": true,
                    "hotkey": true,
                    "pasteText": true,
                    "scroll": true,
                    "drag": true,
                    "setValue": true,
                    "performAction": true,
                ],
                "surfaces": [
                    "menus": false,
                    "dialogs": false,
                    "dock": false,
                    "menubar": false,
                ],
                "desktop": [
                    "screenshot": true,
                    "zoom": true,
                    "sound": true,
                    "mouse": true,
                    "keys": true,
                    "displays": true,
                    "apps": true,
                    "windows": true,
                    "clipboard": true,
                    "find": true,
                    "ocr": true,
                    "read": true,
                    "guard": [
                        "hotkey": "control+option+escape",
                        "signal": "SIGUSR1",
                        "budget": OperatorGuardHost.renderedBudget(),
                    ],
                    // What a reflex run here reads; whether one may run is the window's table.
                    "reflex": ReflexRuntimeHost.handshake(),
                ],
            ],
        ]
    }

    private func resolveApp(_ query: String) throws -> AppDescriptor {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw ProviderError.coded("invalid_argument", "app query must not be empty")
        }
        if let pid = parsePid(trimmed) {
            if let app = appByPid(pid) {
                try rejectBlockedApp(app)
                return app
            }
            throw ProviderError.coded("app_not_found", "app '\(trimmed)' not found")
        }
        if BlockedApps.bundleIds.contains(trimmed) {
            throw ProviderError.coded("app_blocked", "app '\(trimmed)' is blocked for safety")
        }
        if let app = listApps().first(where: { matches($0, query: trimmed) }) {
            try rejectBlockedApp(app)
            return app
        }
        throw ProviderError.coded("app_not_found", "app '\(trimmed)' not found")
    }

    private func refuseOwnTarget(_ params: [String: JSONValue]) throws {
        if let query = params["app"]?.string, let app = try? resolveApp(query) {
            try refuseOwn(app, named: query)
        }
        if let id = params["windowId"]?.number, let owner = DesktopWindows.ownerPid(windowId: CGWindowID(id)), isTrustedZeroCodeApplication(owner) {
            throw ProviderError.coded("app_blocked", "window \(Int(id)) is ZeroCode's own; the operator does not drive the app it lives in")
        }
    }

    /// ZeroCode's own app is never the operator's target (§1.6).
    private func refuseOwn(_ app: AppDescriptor, named query: String) throws {
        if isTrustedZeroCodeApplication(app.pid) {
            throw ProviderError.coded("app_blocked", "'\(query)' is ZeroCode itself; the operator does not drive the app it lives in")
        }
    }

    /// What a reflex plan acts in: its scope's target, resolved the way every
    /// verb names an app (`resolveApp`, blocked apps refused), and never
    /// ZeroCode itself — once, when the run starts; every press is held to it
    /// (`DesktopRunBoundary`).
    private func actingScope(_ scope: ReflexScope) throws -> ReflexActingScope {
        let app = try resolveApp(scope.target)
        try refuseOwn(app, named: scope.target)
        return ReflexActingScope(surface: scope.surface, target: scope.target, pid: app.pid)
    }

    private func rejectBlockedApp(_ app: AppDescriptor) throws {
        if let bundle = app.bundleId, BlockedApps.bundleIds.contains(bundle) {
            throw ProviderError.coded("app_blocked", "app '\(bundle)' is blocked for safety")
        }
    }

    private func listWindows(params: [String: JSONValue]) throws -> [String: Any] {
        let app = try resolveApp(try requiredString(params, "app"))
        let windows = WindowCapture.candidates(pid: app.pid)
            .filter { $0.layer == 0 }
            .enumerated()
            .map { index, candidate -> [String: Any] in
                [
                    "index": index,
                    "app": [
                        "name": app.name,
                        "bundleId": jsonNullable(app.bundleId),
                        "pid": Int(app.pid),
                    ],
                    "id": Int(candidate.windowId),
                    "title": candidate.title ?? "",
                    "x": Int(candidate.bounds.origin.x.rounded()),
                    "y": Int(candidate.bounds.origin.y.rounded()),
                    "width": Int(candidate.bounds.width.rounded()),
                    "height": Int(candidate.bounds.height.rounded()),
                    "isMinimized": false,
                    "isOffscreen": !candidate.isOnScreen,
                    "screenIndex": jsonNullable(screenIndex(for: candidate.bounds)),
                    "isMain": NSNull(),
                    "platform": [
                        "layer": candidate.layer,
                        "alpha": candidate.alpha,
                    ],
                ]
            }
        return [
            "app": renderListedApp(app),
            "windows": windows,
        ]
    }

    private func appByPid(_ pid: pid_t) -> AppDescriptor? {
        guard let app = NSRunningApplication(processIdentifier: pid),
              !app.isTerminated,
              let name = app.localizedName
        else {
            return nil
        }
        return AppDescriptor(name: name, bundleId: app.bundleIdentifier, pid: pid, app: app)
    }

    private func buildSnapshot(
        app: AppDescriptor,
        includeScreenshot: Bool,
        windowId: CGWindowID?,
        windowIndex: Int?,
        restoreWindow: Bool
    ) throws -> Snapshot {
        guard accessibilityTrustedSettled() else {
            // Why: agents retry failed observations. Only the explicit setup flow
            // should open macOS privacy prompts/settings; runtime calls stay quiet.
            throw ProviderError.coded(
                "permission_denied",
                "Accessibility permission is required for ZeroCode Computer Use. Run `zerocode computer permissions` or open Settings > Computer Use, grant Accessibility to ZeroCode Computer Use, then retry."
            )
        }
        let appElement = AXUIElementCreateApplication(app.pid)
        enableManualAccessibilityIfNeeded(appElement, app: app)
        let windowCandidates = WindowCapture.candidates(pid: app.pid)
        let focused = try focusedWindow(
            appElement: appElement,
            app: app,
            visibleWindowCount: windowCandidates.count,
            allowRecovery: restoreWindow
        )
        let focusedTitle = stringAttribute(focused, kAXTitleAttribute as String) ?? app.name
        let canCaptureScreenshot = includeScreenshot && screenCaptureTrustedSettled()
        guard let capture = WindowCapture.resolve(
            candidates: windowCandidates,
            titleHint: focusedTitle,
            windowId: windowId,
            windowIndex: windowIndex,
            captureImage: canCaptureScreenshot
        ) else {
            throw ProviderError.coded("window_not_found", "app '\(app.name)' has no on-screen window")
        }
        guard let window = matchingWindow(appElement: appElement, capture: capture, focused: focused, explicitTarget: windowId != nil || windowIndex != nil) else {
            throw ProviderError.coded("window_not_found", "could not match accessibility window to requested window; run get-app-state again or retry without a window selector")
        }
        let title = stringAttribute(window, kAXTitleAttribute as String) ?? capture.title ?? app.name
        let renderer = TreeRenderer(
            windowBounds: capture.bounds,
            focused: focusedElement(appElement: appElement),
            compactBrowserTabs: app.isKnownBrowser
        )
        renderer.render(window)
        let screenshot = includeScreenshot ? capture.screenshotPayload() : nil
        let screenshotStatus: ScreenshotStatus = if screenshot != nil {
            .captured
        } else if includeScreenshot && !canCaptureScreenshot {
            .failed("Screen Recording permission is required for ZeroCode Computer Use; grant permission or pass --no-screenshot to inspect accessibility state only.")
        } else if includeScreenshot {
            .failed("window screenshot capture returned no image; retry with --no-screenshot if accessibility state is sufficient.")
        } else {
            .skipped
        }
        return Snapshot(
            id: UUID().uuidString,
            app: app,
            windowTitle: title,
            windowBounds: capture.bounds,
            windowId: capture.windowId,
            windowLayer: capture.layer,
            treeText: renderTreeText(app: app, title: title, bounds: capture.bounds, lines: renderer.lines, focused: renderer.focusedSummary),
            focusedElementId: renderer.focusedElementId,
            screenshot: screenshot,
            screenshotStatus: screenshotStatus,
            screenshotScale: screenshotScale(screenshot: screenshot, bounds: capture.bounds),
            screenshotEngine: capture.image?.engine,
            elements: renderer.records,
            truncated: renderer.truncated,
            maxDepthReached: renderer.maxDepthReached
        )
    }

    private func renderSnapshot(_ snapshot: Snapshot, elementFrames: Bool = false) -> [String: Any] {
        var screenshot: Any = NSNull()
        if let payload = snapshot.screenshot {
            screenshot = [
                "data": payload.data,
                "format": "png",
                "width": payload.width,
                "height": payload.height,
                "scale": payload.scale,
            ]
        }
        return [
            "snapshot": [
                "id": snapshot.id,
                "app": [
                    "name": snapshot.app.name,
                    "bundleId": jsonNullable(snapshot.app.bundleId),
                    "pid": Int(snapshot.app.pid),
                ],
                "window": [
                    "id": Int(snapshot.windowId),
                    "title": snapshot.windowTitle,
                    "x": Int(snapshot.windowBounds.origin.x.rounded()),
                    "y": Int(snapshot.windowBounds.origin.y.rounded()),
                    "width": Int(snapshot.windowBounds.width.rounded()),
                    "height": Int(snapshot.windowBounds.height.rounded()),
                    "isMinimized": false,
                    "isOffscreen": false,
                    "screenIndex": jsonNullable(screenIndex(for: snapshot.windowBounds)),
                    "platform": [
                        "layer": snapshot.windowLayer,
                    ],
                ],
                "coordinateSpace": "window",
                "treeText": snapshot.treeText,
                "elementCount": snapshot.elements.count,
                "focusedElementId": snapshot.focusedElementId as Any,
                "truncation": [
                    "truncated": snapshot.truncated,
                    "maxNodes": TreeRenderer.maxNodes,
                    "maxDepth": TreeRenderer.maxDepth,
                    "maxDepthReached": snapshot.maxDepthReached,
                ],
            ].merging(elementFrames ? ["elements": renderElementFaces(snapshot)] : [:]) { current, _ in current },
            "screenshot": screenshot,
            "screenshotStatus": renderScreenshotStatus(snapshot.screenshotStatus, snapshot: snapshot),
        ]
    }

    /// Every indexed element with a frame of positive area, by index: the
    /// faces the window plans a marked look from (the core's `ElementFace`).
    private func renderElementFaces(_ snapshot: Snapshot) -> [[String: Any]] {
        snapshot.elements.keys.sorted().compactMap { index in
            guard let record = snapshot.elements[index],
                  let frame = record.localFrame,
                  frame.width > 0, frame.height > 0
            else { return nil }
            var face: [String: Any] = [
                "index": index,
                "role": record.role,
                "traits": record.traits,
                "actions": record.actions,
                "x": frame.origin.x,
                "y": frame.origin.y,
                "width": frame.width,
                "height": frame.height,
                "signature": record.signature,
            ]
            if let name = record.name { face["name"] = name }
            if let placeholder = record.placeholder { face["placeholder"] = placeholder }
            if let context = record.context { face["context"] = context }
            if let visible = record.visible {
                face["visible"] = ["x": visible.origin.x, "y": visible.origin.y, "width": visible.width, "height": visible.height]
            }
            return face
        }
    }

    private func renderActionResult(action: [String: Any], snapshot: Snapshot) -> [String: Any] {
        var result = renderSnapshot(snapshot)
        var metadata = action
        metadata["targetWindowId"] = Int(snapshot.windowId)
        result["action"] = metadata
        return result
    }

    private func click(params: [String: JSONValue], snapshot: Snapshot) throws -> [String: Any] {
        let button = try mouseButton(params["mouseButton"]?.string)
        let count = try positiveInteger(params["clickCount"]?.number, defaultValue: 1, name: "clickCount")
        guard count <= SyntheticMouseClickDelivery.maxClickCount else {
            throw ProviderError.coded(
                "invalid_argument",
                "clickCount must be at most \(SyntheticMouseClickDelivery.maxClickCount)"
            )
        }
        let modifiers = try KeyMap.parseModifiers(params["modifiers"]?.string)
        // Why: agents expect a click into a target app to make the next
        // keyboard action safe, even when the click uses an AX action path.
        recoverWindow(snapshot.app, windowId: snapshot.windowId, windowBounds: snapshot.windowBounds)
        // A control named by what it reads is chosen on this dispatch's fresh
        // tree — after the steps before it in a batch changed that tree.
        var named = try optionalInteger(params, "elementIndex")
        let query = readingQuery(params)
        if named == nil, !query.isEmpty {
            named = try chosenIndex(query, in: snapshot)
        }
        if let elementIndex = named {
            let record = try element(snapshot, elementIndex)
            // A click by a mark's number presses only what a person would hit:
            // the control, on top at its centre, now that its window is in front.
            if try markPin(params) != nil {
                try requireOnTop(record, in: snapshot)
            }
            let plain = modifiers.isEmpty && count <= 1
            // A click into a field is where the typing goes: focus it, else
            // press it — never its confirm action, which is a Return.
            if plain, button == .left, MarkPin.textEntryRoles.contains(record.role) {
                if AXUIElementSetAttributeValue(record.element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success {
                    return actionMetadata(path: "accessibility", actionName: "AXFocused")
                }
                if performAction(record.element, "AXPress") {
                    return actionMetadata(path: "accessibility", actionName: "AXPress")
                }
            } else if plain,
               button.hasAccessibilityAction,
               let actionName = try performClickAction(record: record, mouseButton: button) {
                return actionMetadata(path: "accessibility", actionName: actionName)
            }
            if let point = center(record.localFrame, in: snapshot.windowBounds) {
                try Input.click(
                    at: point,
                    button: button,
                    count: count,
                    modifiers: modifiers,
                    targetWindow: snapshot
                )
                return actionMetadata(
                    path: "synthetic",
                    fallbackReason: "actionUnsupported",
                    verification: unverifiedAction(reason: "synthetic_input")
                )
            }
            throw ProviderError.coded("element_not_clickable", "element \(record.index) has no clickable frame")
        }
        let point = try coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot)
        try Input.click(
            at: point,
            button: button,
            count: count,
            modifiers: modifiers,
            targetWindow: snapshot
        )
        return actionMetadata(
            path: "synthetic",
            verification: unverifiedAction(reason: "synthetic_input")
        )
    }

    /// The words a click names its control by: `find`'s query, from the same flags.
    private func readingQuery(_ params: [String: JSONValue]) -> ElementQuery {
        ElementQuery(text: params["text"]?.string, role: params["role"]?.string, label: params["label"]?.string)
    }

    /// The control a click by reading presses (§2.5): one match, or the one
    /// that reads the words exactly; none or several is answered by name so
    /// the caller narrows with --role/--label or presses by index — the press
    /// never guesses between two controls.
    private func chosenIndex(_ query: ElementQuery, in snapshot: Snapshot) throws -> Int {
        let faces = snapshot.elements.keys.sorted().compactMap { index -> ElementFace? in
            guard let record = snapshot.elements[index] else { return nil }
            let face = elementFace(record.element)
            return ElementFace(index: index, role: face.role, label: face.label, value: face.value, description: face.description)
        }
        switch query.choose(among: faces) {
        case .one(let index):
            return index
        case .none:
            throw ProviderError.coded(
                "element_not_found",
                "no control in \(snapshot.app.name)'s window «\(snapshot.windowTitle)» reads \(query.words); look with --marks, or find"
            )
        case .many(let some):
            let named = some.prefix(5).map(\.said).joined(separator: "; ")
            throw ProviderError.coded(
                "ambiguous_target",
                "\(some.count) controls read \(query.words): \(named) — add --role or --label, or press one by --element-index"
            )
        }
    }

    /// What is on top at a marked control's centre must be the control, inside
    /// it, or around it: a mark never presses, or points at, something a
    /// person could not hit there (a row scrolled under a header, a control
    /// under a menu or an in-page dialog).
    private func requireOnTop(_ record: ElementRecord, in snapshot: Snapshot) throws {
        guard let point = center(record.localFrame, in: snapshot.windowBounds) else {
            throw ProviderError.coded("element_not_clickable", "element \(record.index) has no clickable frame")
        }
        var hit: AXUIElement?
        guard AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(), Float(point.x), Float(point.y), &hit) == .success,
              let hit
        else {
            throw ProviderError.coded("element_not_found", "nothing answers at the centre of element \(record.index); look again with --marks")
        }
        if reaches(hit, record.element) || reaches(record.element, hit) {
            return
        }
        let covering = [stringAttribute(hit, kAXRoleAttribute as String), stringAttribute(hit, kAXTitleAttribute as String) ?? stringAttribute(hit, kAXDescriptionAttribute as String)]
            .compactMap { $0 }
            .joined(separator: " ")
        throw ProviderError.coded(
            "element_not_found",
            "element \(record.index) is covered at its centre\(covering.isEmpty ? "" : " by \(covering)"); look again with --marks"
        )
    }

    /// Whether walking up from `from` meets `to` within `MarkPin.hitDepth` parents.
    private func reaches(_ from: AXUIElement, _ to: AXUIElement) -> Bool {
        var node: AXUIElement? = from
        for _ in 0..<MarkPin.hitDepth {
            guard let current = node else { return false }
            if CFEqual(current, to) { return true }
            node = copyElement(current, kAXParentAttribute as String)
        }
        return false
    }

    private func performClickAction(record: ElementRecord, mouseButton: MouseButtonSelection) throws -> String? {
        if mouseButton == .right {
            return performAction(record.element, "AXShowMenu") ? "AXShowMenu" : nil
        }
        for action in ["AXPress", "AXConfirm", "AXOpen"] {
            if performAction(record.element, action) {
                return action
            }
        }
        return nil
    }

    private func performSecondaryAction(params: [String: JSONValue], snapshot: Snapshot) throws -> [String: Any] {
        let record = try element(snapshot, try requiredInteger(params, "elementIndex"))
        let requested = try requiredString(params, "action")
        let action = record.actions.first { SnapshotRenderHeuristics.prettyAction($0).caseInsensitiveCompare(requested) == .orderedSame || $0.caseInsensitiveCompare(requested) == .orderedSame }
        guard let action else {
            throw ProviderError.coded("action_not_supported", "'\(requested)' is not a valid secondary action for element \(record.index)")
        }
        guard performAction(record.element, action) else {
            throw ProviderError.coded("accessibility_error", "AXUIElementPerformAction(\(action)) failed")
        }
        return actionMetadata(path: "accessibility", actionName: action)
    }

    private func setValue(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentSnapshot(params: params)
        let record = try element(snapshot, try requiredInteger(params, "elementIndex"))
        let expected = try requiredStringAllowingEmpty(params, "value")
        guard isSettable(record.element, kAXValueAttribute as String) else {
            throw ProviderError.coded("value_not_settable", "element \(record.index) is not settable")
        }
        let current = rawAttributeValue(record.element, kAXValueAttribute as String)
        let coercion = AttributeValueCoercion(existingValue: current, requested: expected)
        let result: AXError
        switch coercion.writeValue {
        case .string:
            result = AXUIElementSetAttributeValue(record.element, kAXValueAttribute as CFString, expected as CFString)
        case let .integer(value):
            result = AXUIElementSetAttributeValue(record.element, kAXValueAttribute as CFString, NSNumber(value: value))
        case let .double(value):
            result = AXUIElementSetAttributeValue(record.element, kAXValueAttribute as CFString, NSNumber(value: value))
        case let .boolean(value):
            result = AXUIElementSetAttributeValue(record.element, kAXValueAttribute as CFString, value ? kCFBooleanTrue : kCFBooleanFalse)
        }
        guard result == .success else {
            throw ProviderError.coded("accessibility_error", "AXUIElementSetAttributeValue failed with \(result.rawValue)")
        }
        // A password field is never read back, and what was written is not
        // repeated in the answer.
        if isSecretField(record.element) {
            return actionMetadata(path: "accessibility", actionName: "AXSetValue", verification: unverifiedAction(reason: "secret_field"))
        }
        let verification: [String: Any]
        switch coercion.compare(readback: rawAttributeValue(record.element, kAXValueAttribute as String)) {
        case let .match(actualPreview):
            verification = verifiedAction(property: "value", expected: expected, actualPreview: actualPreview)
        case let .mismatch(actualPreview):
            verification = unverifiedAction(reason: "value_mismatch", expected: expected, actualPreview: actualPreview)
        case .unsupported:
            verification = unverifiedAction(reason: "readback_unsupported", expected: expected)
        }
        return actionMetadata(path: "accessibility", actionName: "AXSetValue", verification: verification)
    }

    private func typeText(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentKeyboardSnapshot(params: params)
        let text = try requiredString(params, "text")
        let focused = focusedRecord(snapshot)?.element
        let secureInput = try gateSecretEntry(writesText: true, pastes: false, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        if let focused, case .applied(let verification) = TextInput.replaceSelection(focused, with: text) {
            return withSecureInput(actionMetadata(path: "accessibility", actionName: "AXReplaceSelection", verification: verification), secureInput)
        }
        try requireTargetWindowFocused(snapshot, restoreWindowRequested: params["restoreWindow"]?.bool == true)
        let (verification, typedSecureInput) = try typeWatched(text, pid: snapshot.app.pid, receiver: snapshot.app.pid, config: keyboardGuardConfig(params))
        return withSecureInput(actionMetadata(path: "synthetic", actionName: "typeText", verification: verification), typedSecureInput)
    }

    private func pressKey(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentKeyboardSnapshot(params: params)
        let key = try requiredString(params, "key")
        let writes = KeyboardInputSafety.chordWrites(key)
        _ = try gateSecretEntry(writesText: writes == .character, pastes: writes == .clipboard, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        try requireTargetWindowFocused(snapshot, restoreWindowRequested: params["restoreWindow"]?.bool == true)
        // The window was raised: ask again of the focus the keys now hit.
        let secureInput = try gateSecretEntry(writesText: writes == .character, pastes: writes == .clipboard, focused: [systemWideFocusedElement()].compactMap { $0 }, receiver: snapshot.app.pid)
        try Input.pressKey(key, pid: snapshot.app.pid)
        return withSecureInput(actionMetadata(
            path: "synthetic",
            actionName: "pressKey",
            verification: unverifiedAction(reason: "synthetic_input")
        ), secureInput)
    }

    private func hotkey(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentKeyboardSnapshot(params: params)
        let key = try requiredString(params, "key")
        let writes = KeyboardInputSafety.chordWrites(key)
        _ = try gateSecretEntry(writesText: writes == .character, pastes: writes == .clipboard, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        if isSelectAllHotkey(key), let focused = focusedRecord(snapshot), TextInput.selectAll(focused.element) {
            return actionMetadata(
                path: "accessibility",
                actionName: "AXSelectAll",
                verification: TextInput.selectionVerification(focused.element)
            )
        }
        try requireTargetWindowFocused(snapshot, restoreWindowRequested: params["restoreWindow"]?.bool == true)
        let secureInput = try gateSecretEntry(writesText: writes == .character, pastes: writes == .clipboard, focused: [systemWideFocusedElement()].compactMap { $0 }, receiver: snapshot.app.pid)
        try Input.pressKey(key, pid: snapshot.app.pid)
        return withSecureInput(actionMetadata(
            path: "synthetic",
            actionName: "hotkey",
            verification: unverifiedAction(reason: "synthetic_input")
        ), secureInput)
    }

    private func pasteText(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentKeyboardSnapshot(params: params)
        let text = try requiredString(params, "text")
        let focused = focusedRecord(snapshot)?.element
        _ = try gateSecretEntry(writesText: true, pastes: false, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        if let focused, case .applied(let verification) = TextInput.replaceSelection(focused, with: text) {
            return withSecureInput(actionMetadata(path: "accessibility", actionName: "AXReplaceSelection", verification: verification), SecureInputState.now())
        }
        try requireTargetWindowFocused(snapshot, restoreWindowRequested: params["restoreWindow"]?.bool == true)
        let secureInput = try gateSecretEntry(writesText: true, pastes: false, focused: [systemWideFocusedElement()].compactMap { $0 }, receiver: snapshot.app.pid)
        try Input.pasteText(text, pid: snapshot.app.pid)
        return withSecureInput(actionMetadata(
            path: "clipboard",
            actionName: "paste",
            verification: unverifiedAction(reason: "clipboard_paste")
        ), secureInput)
    }

    /// Keys never write a secret (B3, the shared core's `secret_entry`):
    /// text — or the paste chord — is refused before anything is posted when
    /// any element the keys may reach is a password field, or when the app
    /// the keys go to holds the keyboard's secure mode (or it is held while
    /// the focus cannot be read), and the person types it. Answers the
    /// keyboard's secure mode, for the report.
    private func gateSecretEntry(writesText: Bool, pastes: Bool, focused: [AXUIElement], receiver: pid_t?, typed: (Int, Int)? = nil) throws -> SecureInputState {
        let state = SecureInputState.now()
        let verdict = KeyboardInputSafety.secretEntry(
            writesText: writesText,
            pastes: pastes,
            secureField: focused.contains { isSecretField($0) },
            secureInputOn: state.on,
            secureInputByReceiver: state.holderPid != nil && state.holderPid == receiver,
            focusRead: !focused.isEmpty
        )
        guard verdict != .personsEntry else {
            var message = "secret_field: \(focusedAppName(focused.first))"
            if let (done, total) = typed {
                message += "; typed \(done) of \(total)"
            }
            throw ProviderError.coded("secure_input", message)
        }
        return state
    }

    /// Every element an app verb's keys may reach: the snapshot's focused
    /// record (what the model saw — the tree may not have reached it) and the
    /// app's live focus (what the keys hit).
    private func keyTargets(_ snapshot: Snapshot) -> [AXUIElement] {
        [focusedRecord(snapshot)?.element, appFocusedElement(snapshot.app.pid)].compactMap { $0 }
    }

    /// A desktop chord, gated by what it writes into whatever has the focus.
    private func gateDesktopChord(_ key: String) throws -> SecureInputState {
        let writes = KeyboardInputSafety.chordWrites(key)
        let focused = systemWideFocusedElement()
        return try gateSecretEntry(
            writesText: writes == .character,
            pastes: writes == .clipboard,
            focused: [focused].compactMap { $0 },
            receiver: focused.flatMap(pidAttribute) ?? NSWorkspace.shared.frontmostApplication?.processIdentifier
        )
    }

    /// Keys typed as synthetic input by the shared plan (`text_entry_plan`):
    /// into a multi-line text area as one piece; anywhere else a line break is
    /// refused (it would press unasked — `key return` is the press), and the
    /// text is cut after each Tab, the secret-field check running again —
    /// before every piece — on the focus once it has moved and settled:
    /// `user\tpass` never carries `pass` into a password field. Text that stays
    /// in one field is read back after the settle and judged (the core's
    /// `judge_typed`). Both need the window's keyboard table. Answers the
    /// verification and the keyboard's secure mode.
    private func typeWatched(_ text: String, pid: pid_t, receiver: pid_t?, config: KeyboardGuardConfig?) throws -> ([String: Any], SecureInputState) {
        var focus = systemWideFocusedElement()
        let pieces: [String]
        switch KeyboardInputSafety.textEntryPlan(text, multiLine: focus.map(isMultiLineText) ?? false) {
        case let .success(planned):
            pieces = planned
        case let .failure(refusal):
            throw ProviderError.coded("invalid_argument", "\(refusal.rawValue): \(focusedAppName(focus))")
        }
        guard pieces.count <= 1 || config != nil else {
            throw ProviderError.coded("invalid_argument", "text that moves the focus is typed by the window's keyboard table, which this request does not carry")
        }
        let total = text.unicodeScalars.count
        var typed = 0
        var state = SecureInputState.now()
        var landing: TextInput.Landing?
        for (at, piece) in pieces.enumerated() {
            if at > 0, let config {
                focus = settledFocus(after: focus, config: config)
            }
            state = try gateSecretEntry(
                writesText: true,
                pastes: false,
                focused: [focus].compactMap { $0 },
                receiver: receiver ?? focus.flatMap(pidAttribute),
                typed: at > 0 ? (typed, total) : nil
            )
            if pieces.count == 1, let config, let focus {
                landing = TextInput.landingStart(focus, typing: text, maxChars: config.maxChars)
            }
            try Input.typeText(piece, pid: pid)
            typed += piece.unicodeScalars.count
        }
        guard let config, let landing else {
            return (unverifiedAction(reason: "synthetic_input"), state)
        }
        let (outcome, shows) = TextInput.landingFinish(landing, config: config)
        return (TextInput.verification(outcome, expected: text, shows: shows, hidden: looksLikeSecret(landing.element)), state)
    }

    /// Where the focus settled after a piece that moved it: read every poll
    /// until it is somewhere new and two reads agree, or the settle passes —
    /// a browser's focus follows its renderer, a beat behind the keys.
    private func settledFocus(after before: AXUIElement?, config: KeyboardGuardConfig) -> AXUIElement? {
        var waited = 0
        var last: AXUIElement?
        while waited < config.settleMs {
            usleep(useconds_t(config.pollMs * 1_000))
            waited += config.pollMs
            guard let now = systemWideFocusedElement(), !(before.map { CFEqual($0, now) } ?? false) else {
                last = nil
                continue
            }
            if let last, CFEqual(last, now) {
                return now
            }
            last = now
        }
        return systemWideFocusedElement()
    }

    private func scroll(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentSnapshot(params: params)
        let direction = try scrollDirection(try requiredString(params, "direction"))
        let pages = try positiveNumber(params["pages"]?.number, defaultValue: 1, name: "pages")
        if let elementIndex = try optionalInteger(params, "elementIndex") {
            let record = try element(snapshot, elementIndex)
            let action = "AXScroll\(direction.capitalized)ByPage"
            if pages.rounded() == pages, let pageCount = boundedInteger(pages, as: Int.self),
               record.actions.contains(action) {
                for _ in 0..<max(1, pageCount) {
                    _ = performAction(record.element, action)
                }
                return actionMetadata(path: "accessibility", actionName: action)
            }
            guard let point = center(record.localFrame, in: snapshot.windowBounds) else {
                throw ProviderError.coded("element_not_found", "element \(record.index) has no scrollable frame")
            }
            try Input.scroll(pid: snapshot.app.pid, at: point, direction: direction, pages: pages)
            return actionMetadata(path: "synthetic", fallbackReason: "actionUnsupported")
        }
        let point = try coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot)
        try Input.scroll(pid: snapshot.app.pid, at: point, direction: direction, pages: pages)
        return actionMetadata(path: "synthetic")
    }

    private func drag(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentSnapshot(params: params)
        let start: CGPoint
        let end: CGPoint
        if let fromIndex = try optionalInteger(params, "fromElementIndex"),
           let toIndex = try optionalInteger(params, "toElementIndex") {
            let from = try element(snapshot, fromIndex)
            let to = try element(snapshot, toIndex)
            guard let fromPoint = center(from.localFrame, in: snapshot.windowBounds),
                  let toPoint = center(to.localFrame, in: snapshot.windowBounds)
            else {
                throw ProviderError.coded("element_not_found", "drag element has no frame")
            }
            start = fromPoint
            end = toPoint
        } else {
            start = try coordinatePoint(params: params, xKey: "fromX", yKey: "fromY", snapshot: snapshot)
            end = try coordinatePoint(params: params, xKey: "toX", yKey: "toY", snapshot: snapshot)
        }
        try Input.drag(pid: snapshot.app.pid, from: start, to: end)
        return actionMetadata(path: "synthetic")
    }

    private func element(_ snapshot: Snapshot, _ index: Int) throws -> ElementRecord {
        guard let record = snapshot.elements[index] else {
            throw ProviderError.coded("element_not_found", "element \(index) is not in the current cached snapshot for \(snapshot.app.name); run get-app-state again and use a fresh element index")
        }
        return record
    }

    private func focusedRecord(_ snapshot: Snapshot) -> ElementRecord? {
        guard let focusedElementId = snapshot.focusedElementId else {
            return nil
        }
        return snapshot.elements[focusedElementId]
    }
}

/// The apps the operator never drives. A static rather than a top-level
/// constant: main.swift's constants are set only as the helper's own start
/// runs through them, so code that a test reaches (`resolveApp`) must not
/// read one.
private enum BlockedApps {
    static let bundleIds: Set<String> = [
        "com.1password.1password",
        "com.1password.safari",
        "com.bitwarden.desktop",
        "com.dashlane.dashlanephonefinal",
        "com.lastpass.LastPass",
        "com.nordsec.nordpass",
        "me.proton.pass.electron",
        "me.proton.pass.catalyst",
    ]
}

private func requiredString(_ params: [String: JSONValue], _ key: String) throws -> String {
    guard let value = params[key]?.string, !value.isEmpty else {
        throw ProviderError.coded("invalid_argument", "missing \(key)")
    }
    return value
}

private func requiredStringAllowingEmpty(_ params: [String: JSONValue], _ key: String) throws -> String {
    guard let value = params[key]?.string else {
        throw ProviderError.coded("invalid_argument", "missing \(key)")
    }
    return value
}

private func requiredNumber(_ params: [String: JSONValue], _ key: String) throws -> Double {
    guard let value = params[key]?.number, value.isFinite else {
        throw ProviderError.coded("invalid_argument", "missing \(key)")
    }
    return value
}

private func requiredInteger(_ params: [String: JSONValue], _ key: String) throws -> Int {
    guard let value = boundedInteger(try requiredNumber(params, key), as: Int.self) else {
        throw ProviderError.coded("invalid_argument", "\(key) is out of range")
    }
    return value
}

private func optionalInteger(_ params: [String: JSONValue], _ key: String) throws -> Int? {
    guard let raw = params[key]?.number else { return nil }
    guard let value = boundedInteger(raw, as: Int.self) else {
        throw ProviderError.coded("invalid_argument", "\(key) is out of range")
    }
    return value
}

private func positiveInteger(_ value: Double?, defaultValue: Int, name: String) throws -> Int {
    switch ActionArgumentValidation.positiveInteger(value, defaultValue: defaultValue, name: name) {
    case let .success(value):
        return value
    case let .failure(error):
        throw ProviderError.coded("invalid_argument", error.message)
    }
}

private func positiveNumber(_ value: Double?, defaultValue: Double, name: String) throws -> Double {
    switch ActionArgumentValidation.positiveNumber(value, defaultValue: defaultValue, name: name) {
    case let .success(value):
        return value
    case let .failure(error):
        throw ProviderError.coded("invalid_argument", error.message)
    }
}

private func scrollDirection(_ value: String) throws -> String {
    switch ActionArgumentValidation.scrollDirection(value) {
    case let .success(value):
        return value
    case let .failure(error):
        throw ProviderError.coded("invalid_argument", error.message)
    }
}

private func parsePid(_ query: String) -> pid_t? {
    guard query.hasPrefix("pid:") else { return nil }
    guard let pid = Int32(query.dropFirst(4)), pid > 0 else { return nil }
    return pid
}

/// The core's `identity::matches`, as the core module spells it in Swift (a
/// source contract holds it): the localized name, the identifier, or the
/// bundle's own names and executable. `launch --app Calculator` answered
/// name=계산기 (2026-09-21) and `observe --app Calculator` then found nothing;
/// every verb that names a running app — `launch`'s lookup included — asks
/// through this one rule.
private func matches(_ app: AppDescriptor, query: String) -> Bool {
    applicationAnswers(to: query, name: app.name, bundleId: app.bundleId, otherNames: app.otherNames)
}

private func pidIsLive(_ pid: pid_t) -> Bool {
    kill(pid, 0) == 0
}

private func accessibilityTrusted() -> Bool {
    AXIsProcessTrusted()
}

private func accessibilityTrustedSettled() -> Bool {
    // Fresh helper processes can receive transient TCC preflight denials before the real grant settles.
    PermissionTrustSettling.settle(probe: accessibilityTrusted).settled
}

/// The eye's table as `eyeStart` carries it (`eye_table` in the core).
private func eyeConfig(_ params: [String: JSONValue]) -> EyeConfig? {
    EyeConfig(
        framesPerSecond: params["framesPerSecond"]?.number,
        changesKept: params["changesKept"]?.number,
        idleStopMs: params["idleStopMs"]?.number,
        firstFrameMs: params["firstFrameMs"]?.number,
        ocrCellPoints: params["ocrCellPoints"]?.number,
        ocrMarginPoints: params["ocrMarginPoints"]?.number,
        ocrMaxShare: params["ocrMaxShare"]?.number,
        ocrMaxPieces: params["ocrMaxPieces"]?.number
    )
}

/// The ears' table as `listenStart` carries it (`sound_table` in the core).
private func soundSenseConfig(_ params: [String: JSONValue]) -> SoundSenseConfig? {
    var ignored: [String]?
    if case let .array(labels)? = params["ignoredLabels"] {
        ignored = labels.compactMap(\.string)
    }
    return SoundSenseConfig(
        windowSeconds: params["windowSeconds"]?.number,
        hopSeconds: params["hopSeconds"]?.number,
        sampleRate: params["sampleRate"]?.number,
        minConfidence: params["minConfidence"]?.number,
        eventsMax: params["eventsMax"]?.number,
        debounceMs: params["debounceMs"]?.number,
        idleStopMs: params["idleStopMs"]?.number,
        ignoredLabels: ignored
    )
}

func screenCaptureTrusted() -> Bool {
    CGPreflightScreenCaptureAccess()
}

private func screenCaptureTrustedSettled() -> Bool {
    PermissionTrustSettling.settle(timeoutMs: 2_000, probe: screenCaptureTrusted).settled
}

private func permissionStatusSnapshotSettled() -> PermissionStatusSnapshot {
    PermissionStatusSnapshotProbe.capture(
        accessibilityProbe: accessibilityTrustedSettled,
        screenshotsProbe: screenCaptureTrustedSettled
    )
}

private func requestScreenCaptureAccess() -> Bool {
    CGRequestScreenCaptureAccess()
}

private func requestAccessibilityAccess() -> Bool {
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
    return AXIsProcessTrustedWithOptions(options)
}

private func permissionSettingsURL(_ id: String) -> String? {
    guard let table = Bundle.main.object(forInfoDictionaryKey: "ZeroCodePermissionTargets") as? String,
          let data = table.data(using: .utf8) else { return nil }
    return PermissionSettingsTarget.settingsURL(id: id, data: data)
}

private func openAccessibilitySettings() {
    if let url = permissionSettingsURL("accessibility") { openSystemSettings(url) }
}

private func openScreenRecordingSettings() {
    if let url = permissionSettingsURL("screenshots") { openSystemSettings(url) }
}

private func openSystemSettings(_ value: String) {
    guard let url = URL(string: value) else { return }
    NSWorkspace.shared.open(url)
}

private func enableManualAccessibilityIfNeeded(_ appElement: AXUIElement, app: AppDescriptor) {
    guard app.needsManualAccessibilityMode else {
        return
    }
    _ = AXUIElementSetAttributeValue(appElement, "AXManualAccessibility" as CFString, kCFBooleanTrue)
    _ = AXUIElementSetAttributeValue(appElement, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
}

private func focusedWindow(appElement: AXUIElement, app: AppDescriptor, visibleWindowCount: Int, allowRecovery: Bool) throws -> AXUIElement {
    let systemWide = AXUIElementCreateSystemWide()
    if let window = lookupUsableWindow(systemWide: systemWide, appElement: appElement, app: app) {
        return window
    }
    if allowRecovery {
        recoverWindow(app)
        if let window = lookupUsableWindow(systemWide: systemWide, appElement: appElement, app: app) {
            return window
        }
    }
    if visibleWindowCount > 0 {
        var settledWindow: AXUIElement?
        let outcome = PermissionTrustSettling.settle {
            settledWindow = lookupUsableWindow(
                systemWide: systemWide,
                appElement: appElement,
                app: app
            )
            return settledWindow != nil
        }
        if let window = settledWindow, outcome.settled {
            return window
        }
        throw ProviderError.coded("permission_denied", "app '\(app.name)' has visible windows but no accessibility window (AX reads stayed blocked for \(outcome.waitedMs)ms after retries). macOS Accessibility may need ZeroCode Computer Use toggled off and on again in System Settings.")
    }
    throw ProviderError.coded("window_not_found", "app '\(app.name)' has no accessibility window; make sure the app has a visible window, then retry with --restore-window.")
}

private func lookupUsableWindow(
    systemWide: AXUIElement,
    appElement: AXUIElement,
    app: AppDescriptor
) -> AXUIElement? {
    if let window = focusedSystemWindow(systemWide: systemWide, app: app) {
        return window
    }
    if let window = copyElement(appElement, kAXFocusedWindowAttribute as String), usableWindow(window) {
        return window
    }
    if let windows = copyArray(appElement, kAXWindowsAttribute as String) {
        return windows.first(where: usableWindow)
    }
    return nil
}

private func focusedSystemWindow(systemWide: AXUIElement, app: AppDescriptor) -> AXUIElement? {
    guard let focusedApp = copyElement(systemWide, kAXFocusedApplicationAttribute as String),
          pidAttribute(focusedApp) == app.pid
    else {
        return nil
    }
    if let window = copyElement(systemWide, kAXFocusedWindowAttribute as String), usableWindow(window) {
        return window
    }
    if let window = copyElement(focusedApp, kAXFocusedWindowAttribute as String), usableWindow(window) {
        return window
    }
    if let windows = copyArray(focusedApp, kAXWindowsAttribute as String) {
        return windows.first(where: usableWindow)
    }
    return nil
}

private func isTargetWindowFocused(_ snapshot: Snapshot) -> Bool {
    guard let focusedWindow = focusedSystemWindow(systemWide: AXUIElementCreateSystemWide(), app: snapshot.app) else {
        return false
    }
    if windowNumber(focusedWindow) == snapshot.windowId {
        return true
    }
    guard let frame = absoluteFrame(focusedWindow) else {
        return false
    }
    let intersection = frame.intersection(snapshot.windowBounds)
    return !intersection.isNull && intersection.area >= min(frame.area, snapshot.windowBounds.area) * 0.75
}

private enum AXElementProbe {
    case value(AXUIElement)
    case absent
    case unavailable
}

private func copyElementProbe(_ element: AXUIElement, _ attribute: String) -> AXElementProbe {
    var value: CFTypeRef?
    switch AXUIElementCopyAttributeValue(element, attribute as CFString, &value) {
    case .success:
        guard let value else { return .unavailable }
        return .value(value as! AXUIElement)
    case .noValue:
        return .absent
    default:
        return .unavailable
    }
}

private func currentSyntheticClickRecipient(
    snapshot: Snapshot,
    point: CGPoint
) -> SyntheticMouseClickDelivery.RecipientObservation {
    let target = syntheticClickRecipient(pid: snapshot.app.pid, windowId: snapshot.windowId)
    var cachedTargetCandidates: [WindowCandidate]?
    func targetCandidates() -> [WindowCandidate] {
        if let cachedTargetCandidates { return cachedTargetCandidates }
        let candidates = WindowCapture.candidates(pid: snapshot.app.pid)
        cachedTargetCandidates = candidates
        return candidates
    }
    switch focusedSyntheticClickRecipient(
        targetPID: snapshot.app.pid,
        targetCandidates: targetCandidates
    ) {
    case let .focused(focused):
        guard focused == target else { return .focused(focused) }
        switch hitTestSyntheticClickRecipient(
            at: point,
            targetPID: snapshot.app.pid,
            targetCandidates: targetCandidates
        ) {
        case let .focused(recipient):
            return .focused(recipient)
        case .dismissed, .unavailable:
            return .unavailable
        }
    case .dismissed:
        return .dismissed
    case .unavailable:
        return .unavailable
    }
}

private func focusedSyntheticClickRecipient(
    targetPID: pid_t,
    targetCandidates: () -> [WindowCandidate]
) -> SyntheticMouseClickDelivery.RecipientObservation {
    let systemWide = AXUIElementCreateSystemWide()
    let focusedApp: AXUIElement
    switch copyElementProbe(systemWide, kAXFocusedApplicationAttribute as String) {
    case let .value(value):
        focusedApp = value
    case .absent:
        return .dismissed
    case .unavailable:
        return .unavailable
    }
    guard let ownerPID = pidAttribute(focusedApp) else { return .unavailable }

    let focusedWindow: AXUIElement
    switch copyElementProbe(focusedApp, kAXFocusedWindowAttribute as String) {
    case let .value(value):
        focusedWindow = value
    case .absent:
        guard ownerPID == targetPID else { return .unavailable }
        return .dismissed
    case .unavailable:
        return .unavailable
    }

    if let windowId = windowNumber(focusedWindow) {
        return .focused(syntheticClickRecipient(pid: ownerPID, windowId: windowId))
    }
    guard ownerPID == targetPID else { return .unavailable }
    guard let frame = absoluteFrame(focusedWindow),
          let candidate = SyntheticMouseClickDelivery.uniqueWindowCandidate(
            from: targetCandidates(),
            matching: {
              windowFramesMatch($0.bounds, frame)
            }
          )
    else { return .unavailable }
    return .focused(syntheticClickRecipient(pid: ownerPID, windowId: candidate.windowId))
}

private func hitTestSyntheticClickRecipient(
    at point: CGPoint,
    targetPID: pid_t,
    targetCandidates: () -> [WindowCandidate]
) -> SyntheticMouseClickDelivery.RecipientObservation {
    let systemWide = AXUIElementCreateSystemWide()
    var hitElement: AXUIElement?
    switch AXUIElementCopyElementAtPosition(
        systemWide,
        Float(point.x),
        Float(point.y),
        &hitElement
    ) {
    case .success:
        break
    case .noValue:
        return .dismissed
    default:
        return .unavailable
    }
    guard let hitElement,
          let ownerPID = pidAttribute(hitElement),
          let window = containingWindow(hitElement)
    else { return .unavailable }
    if let windowId = windowNumber(window) {
        return .focused(syntheticClickRecipient(pid: ownerPID, windowId: windowId))
    }
    guard ownerPID == targetPID else { return .unavailable }
    guard let frame = absoluteFrame(window),
          let candidate = SyntheticMouseClickDelivery.uniqueWindowCandidate(
            from: targetCandidates(),
            matching: {
              windowFramesMatch($0.bounds, frame)
            }
          )
    else { return .unavailable }
    return .focused(syntheticClickRecipient(pid: ownerPID, windowId: candidate.windowId))
}

private func containingWindow(_ element: AXUIElement) -> AXUIElement? {
    var current = element
    for _ in 0..<64 {
        if stringAttribute(current, kAXRoleAttribute as String) == kAXWindowRole as String {
            return current
        }
        if let window = copyElement(current, kAXWindowAttribute as String) {
            return window
        }
        guard let parent = copyElement(current, kAXParentAttribute as String) else {
            return nil
        }
        current = parent
    }
    return nil
}

private func syntheticClickRecipient(
    pid: pid_t,
    windowId: CGWindowID
) -> SyntheticMouseClickDelivery.Recipient {
    SyntheticMouseClickDelivery.Recipient(ownerPID: pid, windowID: windowId)
}

private func requireTargetWindowFocused(_ snapshot: Snapshot, restoreWindowRequested: Bool) throws {
    guard let failure = KeyboardInputSafety.syntheticInputFocusFailure(
        targetWindowFocused: isTargetWindowFocused(snapshot),
        restoreWindowRequested: restoreWindowRequested
    ) else {
        return
    }
    switch failure {
    case .targetNotFocused:
        throw ProviderError.coded("window_not_focused", "keyboard input requires the target \(snapshot.app.name) window to be focused; retry with --restore-window or use set-value for editable elements")
    case .targetNotFocusedAfterRestore:
        throw ProviderError.coded("window_not_focused", "keyboard input requires the target \(snapshot.app.name) window to be focused; --restore-window was requested but the target is still not focused; bring it forward manually or check Accessibility permissions")
    }
}

private func matchingWindow(appElement: AXUIElement, capture: WindowCapture, focused: AXUIElement, explicitTarget: Bool) -> AXUIElement? {
    guard let windows = copyArray(appElement, kAXWindowsAttribute as String) else {
        return nil
    }
    if let byNumber = windows.first(where: { windowNumber($0) == capture.windowId }) {
        return byNumber
    }
    if let byBounds = windows.first(where: { window in
        guard usableWindow(window), let frame = absoluteFrame(window) else { return false }
        let intersection = frame.intersection(capture.bounds)
        return !intersection.isNull && intersection.area >= min(frame.area, capture.bounds.area) * 0.75
    }) {
        return byBounds
    }
    if explicitTarget {
        return nil
    }
    guard let titleHint = capture.title, !titleHint.isEmpty else {
        return focused
    }
    return windows.first {
        usableWindow($0) && stringAttribute($0, kAXTitleAttribute as String) == titleHint
    } ?? focused
}

private func recoverWindow(
    _ app: AppDescriptor,
    windowId: CGWindowID? = nil,
    windowBounds: CGRect? = nil
) {
    // Use the same fresh focus proof as synthetic delivery. Some apps omit
    // AXWindowNumber; the existing fallback accepts only a unique exact-frame
    // CG window owned by the focused PID, never an overlapping or ambiguous one.
    if let windowId, let windowBounds, app.app.isActive, !app.app.isHidden {
        let candidates = WindowCapture.candidates(pid: app.pid)
        if focusedSyntheticClickRecipient(targetPID: app.pid, targetCandidates: { candidates })
               == .focused(syntheticClickRecipient(pid: app.pid, windowId: windowId)),
           candidates.contains(where: {
               $0.windowId == windowId && $0.isOnScreen && windowFramesMatch($0.bounds, windowBounds)
           }) {
            return
        }
    }
    _ = app.app.unhide()
    _ = app.app.activate(options: [.activateAllWindows])
    if let bundleId = app.bundleId {
        openBundle(bundleId)
    }
    let appElement = AXUIElementCreateApplication(app.pid)
    let focusedWindow = copyElement(appElement, kAXFocusedWindowAttribute as String)
    var cachedWindows: [AXUIElement]?
    func windows() -> [AXUIElement] {
        if let cachedWindows { return cachedWindows }
        let value = copyArray(appElement, kAXWindowsAttribute as String) ?? []
        cachedWindows = value
        return value
    }
    let targetWindow: AXUIElement?
    if let focusedWindow,
       (windowId == nil && windowBounds == nil || windowMatchesCapture(
           focusedWindow,
           windowId: windowId,
           windowBounds: windowBounds
       )) {
        targetWindow = focusedWindow
    } else {
        let exactWindow = windowId.flatMap { targetId in
            windows().first { windowNumber($0) == targetId }
        }
        targetWindow = exactWindow ?? windowBounds.flatMap { targetBounds in
            windows().first { window in
                absoluteFrame(window).map { windowFramesMatch($0, targetBounds) } == true
            }
        }
    }
    if let window = targetWindow ?? focusedWindow ?? windows().first {
        _ = AXUIElementSetAttributeValue(window, kAXMinimizedAttribute as CFString, kCFBooleanFalse)
        _ = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        _ = AXUIElementSetAttributeValue(window, kAXMainAttribute as CFString, kCFBooleanTrue)
        _ = AXUIElementSetAttributeValue(window, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }
    Thread.sleep(forTimeInterval: 0.4)
}

private func windowMatchesCapture(
    _ window: AXUIElement,
    windowId: CGWindowID?,
    windowBounds: CGRect?
) -> Bool {
    if let windowId, windowNumber(window) == windowId { return true }
    guard let windowBounds, let frame = absoluteFrame(window) else { return false }
    return windowFramesMatch(frame, windowBounds)
}

private func windowFramesMatch(_ lhs: CGRect, _ rhs: CGRect) -> Bool {
    let tolerance: CGFloat = 2
    return abs(lhs.minX - rhs.minX) <= tolerance &&
        abs(lhs.minY - rhs.minY) <= tolerance &&
        abs(lhs.width - rhs.width) <= tolerance &&
        abs(lhs.height - rhs.height) <= tolerance
}

private func openBundle(_ bundleId: String) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    process.arguments = ["-b", bundleId]
    process.standardOutput = Pipe()
    process.standardError = Pipe()
    try? process.run()
    process.waitUntilExit()
}

private func hasRequestedWindowSelector(_ params: [String: JSONValue]) -> Bool {
    params["windowId"] != nil || params["windowIndex"] != nil
}

private func requestedWindowId(_ params: [String: JSONValue]) throws -> CGWindowID? {
    guard let raw = params["windowId"] else { return nil }
    guard let value = raw.number, value >= 0, let id = boundedInteger(value, as: UInt32.self) else {
        throw ProviderError.coded("invalid_argument", "windowId is out of range")
    }
    return CGWindowID(id)
}

private func snapshotWindowKey(_ query: String, _ windowId: CGWindowID) -> String {
    "\(query.lowercased())#window:\(Int(windowId))"
}

private func snapshotCanonicalWindowIdKey(_ windowId: CGWindowID) -> String {
    "window-id:\(Int(windowId))"
}

private func snapshotWindowIndexKey(_ query: String, _ windowIndex: Int) -> String {
    "\(query.lowercased())#windowIndex:\(windowIndex)"
}

private func snapshotCanonicalWindowIndexKey(_ windowIndex: Int) -> String {
    "window-index:\(windowIndex)"
}

private func snapshotNamespace(_ params: [String: JSONValue]) -> String {
    if let session = params["session"]?.string, !session.isEmpty {
        return "session:\(session)"
    }
    if let worktree = params["worktree"]?.string, !worktree.isEmpty {
        return "worktree:\(worktree)"
    }
    return "default"
}

private func namespacedSnapshotKey(_ namespace: String, _ key: String) -> String {
    "\(namespace):\(key.lowercased())"
}

private func isExplicitSnapshotNamespace(_ namespace: String) -> Bool {
    namespace != "default"
}

private func requestedWindowIndex(_ params: [String: JSONValue]) throws -> Int? {
    guard let raw = params["windowIndex"] else { return nil }
    guard let value = raw.number, value >= 0, let index = boundedInteger(value, as: Int.self) else {
        throw ProviderError.coded("invalid_argument", "windowIndex is out of range")
    }
    return index
}

private func usableWindow(_ element: AXUIElement) -> Bool {
    stringAttribute(element, kAXRoleAttribute as String) == kAXWindowRole as String &&
        boolAttribute(element, kAXMinimizedAttribute as String) != true
}

private func focusedElement(appElement: AXUIElement) -> AXUIElement? {
    copyElement(appElement, kAXFocusedUIElementAttribute as String)
}

private func copyElement(_ element: AXUIElement, _ attribute: String) -> AXUIElement? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success, let value else {
        return nil
    }
    return (value as! AXUIElement)
}

private func copyArray(_ element: AXUIElement, _ attribute: String) -> [AXUIElement]? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success, let value else {
        return nil
    }
    return value as? [AXUIElement]
}

private func pidAttribute(_ element: AXUIElement) -> pid_t? {
    var pid: pid_t = 0
    guard AXUIElementGetPid(element, &pid) == .success else {
        return nil
    }
    return pid
}

private func stringAttribute(_ element: AXUIElement, _ attribute: String) -> String? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success, let value else {
        return nil
    }
    if CFGetTypeID(value) == CFStringGetTypeID(), let string = value as? String {
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
    if CFGetTypeID(value) == CFURLGetTypeID(), let url = value as? URL {
        return url.absoluteString
    }
    return nil
}

private func rawStringAttribute(_ element: AXUIElement, _ attribute: String) -> String? {
    guard let value = rawAttributeValue(element, attribute), CFGetTypeID(value) == CFStringGetTypeID()
    else {
        return nil
    }
    return value as? String
}

private func rawAttributeValue(_ element: AXUIElement, _ attribute: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else {
        return nil
    }
    return value
}

private func boolAttribute(_ element: AXUIElement, _ attribute: String) -> Bool? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else {
        return nil
    }
    return value as? Bool
}

private func numberAttribute(_ element: AXUIElement, _ attribute: String) -> NSNumber? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else {
        return nil
    }
    return value as? NSNumber
}

private func windowNumber(_ element: AXUIElement) -> CGWindowID? {
    guard let number = numberAttribute(element, "AXWindowNumber") else {
        return nil
    }
    return CGWindowID(number.uint32Value)
}

private func absoluteFrame(_ element: AXUIElement) -> CGRect? {
    var positionValue: CFTypeRef?
    var sizeValue: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue) == .success,
          AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue) == .success,
          let positionValue,
          let sizeValue
    else {
        return nil
    }
    var point = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(positionValue as! AXValue, .cgPoint, &point),
          AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
    else {
        return nil
    }
    return CGRect(origin: point, size: size)
}

private extension CGRect {
    var area: CGFloat {
        max(width, 0) * max(height, 0)
    }
}

private func actions(_ element: AXUIElement) -> [String] {
    var value: CFArray?
    guard AXUIElementCopyActionNames(element, &value) == .success, let value else {
        return []
    }
    return value as? [String] ?? []
}

private func performAction(_ element: AXUIElement, _ action: String) -> Bool {
    actions(element).contains(where: { $0.caseInsensitiveCompare(action) == .orderedSame }) &&
        AXUIElementPerformAction(element, action as CFString) == .success
}

private func isSettable(_ element: AXUIElement, _ attribute: String) -> Bool {
    var settable = DarwinBoolean(false)
    return AXUIElementIsAttributeSettable(element, attribute as CFString, &settable) == .success && settable.boolValue
}

private func frame(_ element: AXUIElement, windowBounds: CGRect) -> CGRect? {
    var positionValue: CFTypeRef?
    var sizeValue: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue) == .success,
          AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue) == .success,
          let positionValue,
          let sizeValue
    else {
        return nil
    }
    var point = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(positionValue as! AXValue, .cgPoint, &point),
          AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
    else {
        return nil
    }
    return CGRect(x: point.x - windowBounds.minX, y: point.y - windowBounds.minY, width: size.width, height: size.height)
}

private func elementSignature(_ node: SnapshotRenderNode) -> String {
    // Why: cached element validation should prove identity, not reject text
    // controls because their value/placeholder/summary changed after focus.
    [
        node.role,
        node.roleDescription ?? "",
        node.title ?? "",
        node.label ?? "",
        node.linkText ?? "",
        node.url ?? "",
        SnapshotRenderHeuristics.meaningfulActions(node.rawActions, role: node.role).joined(separator: ","),
    ].joined(separator: "\u{1f}")
}

private func center(_ localFrame: CGRect?, in windowBounds: CGRect) -> CGPoint? {
    guard let localFrame else { return nil }
    return CGPoint(x: windowBounds.minX + localFrame.midX, y: windowBounds.minY + localFrame.midY)
}

private func screenIndex(for bounds: CGRect) -> Int? {
    guard let index = NSScreen.screens.firstIndex(where: { $0.frame.intersects(bounds) }) else {
        return nil
    }
    return index
}

private func coordinatePoint(params: [String: JSONValue], xKey: String, yKey: String, snapshot: Snapshot) throws -> CGPoint {
    let x = try requiredNumber(params, xKey)
    let y = try requiredNumber(params, yKey)
    return CGPoint(
        x: snapshot.windowBounds.minX + x,
        y: snapshot.windowBounds.minY + y
    )
}

private func screenshotScale(screenshot: ScreenshotPayload?, bounds: CGRect) -> CGSize {
    guard let screenshot, bounds.width > 0, bounds.height > 0 else {
        return CGSize(width: 1, height: 1)
    }
    return CGSize(
        width: CGFloat(screenshot.width) / bounds.width,
        height: CGFloat(screenshot.height) / bounds.height
    )
}

extension MouseButtonSelection {
    // Why: macOS has no dedicated middle-button event family; it rides `otherMouse*`
    // with the button number carried by `mouseButton:` on the event constructor.
    var cgButton: CGMouseButton {
        switch self {
        case .left:
            return .left
        case .right:
            return .right
        case .middle:
            return .center
        }
    }

    var downEvent: CGEventType {
        switch self {
        case .left:
            return .leftMouseDown
        case .right:
            return .rightMouseDown
        case .middle:
            return .otherMouseDown
        }
    }

    var upEvent: CGEventType {
        switch self {
        case .left:
            return .leftMouseUp
        case .right:
            return .rightMouseUp
        case .middle:
            return .otherMouseUp
        }
    }

    var dragEvent: CGEventType {
        switch self {
        case .left:
            return .leftMouseDragged
        case .right:
            return .rightMouseDragged
        case .middle:
            return .otherMouseDragged
        }
    }
}

private func mouseButton(_ raw: String?) throws -> MouseButtonSelection {
    switch ActionArgumentValidation.mouseButton(raw) {
    case let .success(button):
        return button
    case let .failure(error):
        throw ProviderError.coded("invalid_argument", error.message)
    }
}

private func renderTreeText(app: AppDescriptor, title: String, bounds: CGRect, lines: [String], focused: String?) -> String {
    var output = [
        "App=\(app.bundleId ?? app.name.replacingOccurrences(of: " ", with: "_")) (pid \(app.pid))",
        "Window: \"\(sanitize(title))\", App: \(sanitize(app.name)).",
        "",
    ]
    output.append(contentsOf: lines)
    output.append("")
    output.append(focused.map { "The focused UI element is \($0)." } ?? "No UI element is currently focused.")
    return output.joined(separator: "\n")
}

private func renderScreenshotStatus(_ status: ScreenshotStatus, snapshot: Snapshot) -> [String: Any] {
    let metadata: [String: Any] = [
        "engine": snapshot.screenshotEngine ?? "unknown",
        "windowId": Int(snapshot.windowId),
    ]
    switch status {
    case .captured:
        return ["state": "captured", "metadata": metadata]
    case .skipped:
        return ["state": "skipped", "reason": "no_screenshot_flag"]
    case let .failed(message):
        return ["state": "failed", "code": "screenshot_failed", "message": message, "metadata": metadata]
    }
}

private final class TreeRenderer {
    let windowBounds: CGRect
    let focused: AXUIElement?
    let compactBrowserTabs: Bool
    var lines: [String] = []
    var records: [Int: ElementRecord] = [:]
    var focusedSummary: String?
    var focusedElementId: Int?
    var truncated = false
    var maxDepthReached = false
    private let reader = AXSnapshotReader()
    private var nextIndex = 0
    static let maxNodes = 1200
    static let maxDepth = 64

    init(windowBounds: CGRect, focused: AXUIElement?, compactBrowserTabs: Bool) {
        self.windowBounds = windowBounds
        self.focused = focused
        self.compactBrowserTabs = compactBrowserTabs
    }

    func render(
        _ element: AXUIElement,
        depth: Int = 0,
        ancestors: [AXUIElement] = [],
        clip: CGRect? = nil,
        context: String? = nil
    ) {
        guard nextIndex < Self.maxNodes else {
            truncated = true
            return
        }
        guard depth < Self.maxDepth else {
            truncated = true
            maxDepthReached = true
            return
        }
        guard !ancestors.contains(where: { CFEqual($0, element) }) else { return }

        let role = reader.stringAttribute(element, kAXRoleAttribute as String) ?? "AXUnknown"
        let children = reader.primaryChildren(element, role: role, windowBounds: windowBounds)
        let value = reader.valueString(element, role: role)
        let placeholder = reader.placeholderString(element)
        let rawActions = reader.actions(element)
        let rowSummary = reader.rowTextSummary(element, role: role)
        let roleDescription = reader.stringAttribute(element, kAXRoleDescriptionAttribute as String)
        let title = reader.stringAttribute(element, kAXTitleAttribute as String)
        let label = reader.stringAttribute(element, kAXDescriptionAttribute as String)
        let url = reader.stringAttribute(element, kAXURLAttribute as String)
        let linkText = role == "AXLink" ? reader.descendantTextSnippets(element, limit: 2, maxDepth: 3).first : nil
        let baseNode = SnapshotRenderNode(
            role: role,
            roleDescription: roleDescription,
            title: title,
            label: label,
            linkText: linkText,
            value: value,
            placeholder: placeholder,
            url: url,
            traits: [],
            rawActions: rawActions,
            childCount: children.count,
            rowSummary: rowSummary
        )
        let name = SnapshotRenderHeuristics.displayName(baseNode)
        let meaningful = SnapshotRenderHeuristics.meaningfulActions(rawActions, role: role)
        let localFrame = reader.frame(element, windowBounds: windowBounds)
        let traits = reader.traitsFor(element, role: role)
        let webAreaDepth = reader.webAreaDepth(role: role, ancestors: ancestors)
        let summary = reader.genericTextSummary(element, role: role, name: name, actions: meaningful, traits: traits)
        let node = SnapshotRenderNode(
            role: role,
            roleDescription: roleDescription,
            title: title,
            label: label,
            linkText: linkText,
            value: value,
            placeholder: placeholder,
            url: url,
            traits: traits,
            rawActions: rawActions,
            childCount: children.count,
            summary: summary,
            rowSummary: rowSummary,
            webAreaDepth: webAreaDepth
        )
        if SnapshotRenderHeuristics.shouldElide(node) {
            for child in children {
                render(child, depth: depth, ancestors: ancestors + [element], clip: clip, context: context)
            }
            return
        }
        // What a child inherits: the clip this container makes, and the
        // nearest name above it (the core's walk carries the same two).
        let visible = localFrame.map { MarkPin.clipped($0, by: clip) }
        let childClip = localFrame.flatMap { frame in
            MarkPin.clipRoles.contains(role) ? MarkPin.clipped(frame, by: clip) : nil
        } ?? clip
        let childContext = name ?? context

        let index = nextIndex
        nextIndex += 1
        let line = SnapshotRenderHeuristics.line(index: index, node: node)
        lines.append(String(repeating: "\t", count: depth) + line)
        records[index] = ElementRecord(
            index: index,
            element: element,
            localFrame: localFrame,
            actions: rawActions,
            signature: elementSignature(node),
            role: role,
            name: name,
            placeholder: placeholder.flatMap { $0.isEmpty ? nil : SnapshotRenderHeuristics.sanitize($0) },
            traits: traits,
            visible: visible,
            context: context
        )
        if let focused, CFEqual(focused, element) {
            focusedElementId = index
            focusedSummary = line
        }
        if summary != nil || SnapshotRenderHeuristics.shouldSuppressChildren(node) {
            return
        }
        if compactBrowserTabs, let tabStripCompaction = tabStripCompaction(parent: node, children: children) {
            for (childIndex, child) in children.enumerated() where tabStripCompaction.retainedIndexes.contains(childIndex) {
                render(child, depth: depth + 1, ancestors: ancestors + [element], clip: childClip, context: childContext)
            }
            lines.append(
                String(repeating: "\t", count: depth + 1) +
                    "... \(tabStripCompaction.omittedCount) inactive browser tabs omitted"
            )
            return
        }
        let childLineStart = lines.count
        for child in children {
            render(child, depth: depth + 1, ancestors: ancestors + [element], clip: childClip, context: childContext)
        }
        if compactBrowserTabs {
            compactRenderedBrowserTabs(parent: node, startLine: childLineStart, depth: depth + 1)
        }
    }

    private func tabStripCompaction(parent: SnapshotRenderNode, children: [AXUIElement]) -> SnapshotTabStripCompaction? {
        let childNodes = children.map { child in
            let role = reader.stringAttribute(child, kAXRoleAttribute as String) ?? "AXUnknown"
            return SnapshotRenderNode(
                role: role,
                roleDescription: reader.stringAttribute(child, kAXRoleDescriptionAttribute as String),
                title: reader.stringAttribute(child, kAXTitleAttribute as String),
                label: reader.stringAttribute(child, kAXDescriptionAttribute as String),
                value: reader.valueString(child, role: role),
                traits: reader.traitsFor(child, role: role)
            )
        }
        // Why: browsers expose every open tab through AX; retaining only the active
        // tab keeps snapshots focused on the current page instead of stale tab titles.
        return SnapshotRenderHeuristics.tabStripCompaction(parent: parent, children: childNodes)
    }

    private func compactRenderedBrowserTabs(parent: SnapshotRenderNode, startLine: Int, depth: Int) {
        guard SnapshotRenderHeuristics.roleText(parent) == "scroll area" else { return }
        let indent = String(repeating: "\t", count: depth)
        let tabLineIndexes = lines.indices.dropFirst(startLine).filter { lineIndex in
            isDirectRenderedBrowserTabLine(lines[lineIndex], indent: indent)
        }
        guard tabLineIndexes.count >= 10 else { return }
        let activeLineIndexes = Set(tabLineIndexes.filter { lineIndex in
            isActiveRenderedBrowserTabLine(lines[lineIndex])
        })
        guard !activeLineIndexes.isEmpty else { return }

        let insertionIndex = tabLineIndexes.first!
        var omittedCount = 0
        for lineIndex in tabLineIndexes.reversed() where !activeLineIndexes.contains(lineIndex) {
            if let recordIndex = renderedElementIndex(lines[lineIndex], indent: indent) {
                records.removeValue(forKey: recordIndex)
                if focusedElementId == recordIndex {
                    focusedElementId = nil
                    focusedSummary = nil
                }
            }
            lines.remove(at: lineIndex)
            omittedCount += 1
        }
        guard omittedCount > 0 else { return }
        lines.insert("\(indent)... \(omittedCount) inactive browser tabs omitted", at: insertionIndex)
    }
}

/// Reads one window's Accessibility tree, a node at a time.
///
/// A node's attributes arrive together: `SnapshotNodeAttributes.all` names
/// them and `AXUIElementCopyMultipleAttributeValues` fetches the lot in one
/// cross-process call, so reading a node afterwards touches the observed app
/// no further. An attribute the table does not name still answers — through
/// the single read below — which is why the table is free to be a statement
/// about cost alone.
private final class AXSnapshotReader {
    private enum CachedAttribute {
        case missing
        case found(CFTypeRef)

        var value: CFTypeRef? {
            switch self {
            case .missing:
                return nil
            case let .found(value):
                return value
            }
        }
    }

    private final class ElementCache {
        let element: AXUIElement
        var prefetchedTable = false
        var loadedAttributeNames = false
        var advertisedAttributes: Set<String>?
        var attributes: [String: CachedAttribute] = [:]
        var actions: [String]?
        var settable: [String: Bool] = [:]

        init(element: AXUIElement) {
            self.element = element
        }
    }

    private var elementsByHash: [CFHashCode: [ElementCache]] = [:]

    func stringAttribute(_ element: AXUIElement, _ attribute: String) -> String? {
        guard let value = copyAttribute(element, attribute) else { return nil }
        if CFGetTypeID(value) == CFStringGetTypeID(), let string = value as? String {
            let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }
        if CFGetTypeID(value) == CFURLGetTypeID(), let url = value as? URL {
            return url.absoluteString
        }
        return nil
    }

    func boolAttribute(_ element: AXUIElement, _ attribute: String) -> Bool? {
        copyAttribute(element, attribute) as? Bool
    }

    func numberAttribute(_ element: AXUIElement, _ attribute: String) -> NSNumber? {
        copyAttribute(element, attribute) as? NSNumber
    }

    func copyArray(_ element: AXUIElement, _ attribute: String) -> [AXUIElement]? {
        copyAttribute(element, attribute) as? [AXUIElement]
    }

    func actions(_ element: AXUIElement) -> [String] {
        let cache = cache(for: element)
        if let actions = cache.actions {
            return actions
        }
        var value: CFArray?
        let actions = AXUIElementCopyActionNames(element, &value) == .success ? value as? [String] ?? [] : []
        cache.actions = actions
        return actions
    }

    func isSettable(_ element: AXUIElement, _ attribute: String) -> Bool {
        let cache = cache(for: element)
        if let cached = cache.settable[attribute] {
            return cached
        }
        var settable = DarwinBoolean(false)
        let value = AXUIElementIsAttributeSettable(element, attribute as CFString, &settable) == .success && settable.boolValue
        cache.settable[attribute] = value
        return value
    }

    func frame(_ element: AXUIElement, windowBounds: CGRect) -> CGRect? {
        guard let absolute = absoluteFrame(element) else { return nil }
        return CGRect(
            x: absolute.minX - windowBounds.minX,
            y: absolute.minY - windowBounds.minY,
            width: absolute.width,
            height: absolute.height
        )
    }

    func primaryChildren(_ element: AXUIElement, role: String, windowBounds: CGRect) -> [AXUIElement] {
        if usesRowsAsPrimaryChildren(role: role), let rows = copyArray(element, kAXRowsAttribute as String), !rows.isEmpty {
            return visibleRows(rows, parent: element, windowBounds: windowBounds)
        }
        return copyArray(element, kAXChildrenAttribute as String) ?? []
    }

    func valueString(_ element: AXUIElement, role: String) -> String? {
        if isSecureTextElement(element, role: role) {
            return "[redacted]"
        }
        if let string = stringAttribute(element, kAXValueAttribute as String) {
            return string
        }
        if let number = numberAttribute(element, kAXValueAttribute as String) {
            return number.stringValue
        }
        return nil
    }

    func placeholderString(_ element: AXUIElement) -> String? {
        stringAttribute(element, "AXPlaceholderValue") ?? stringAttribute(element, "AXPlaceholder")
    }

    func traitsFor(_ element: AXUIElement, role: String) -> [String] {
        var traits: [String] = []
        if boolAttribute(element, kAXSelectedAttribute as String) == true { traits.append("selected") }
        if boolAttribute(element, kAXExpandedAttribute as String) == true { traits.append("expanded") }
        if boolAttribute(element, kAXEnabledAttribute as String) == false { traits.append("disabled") }
        if valueSettableRoles.contains(role), isSettable(element, kAXValueAttribute as String) { traits.append("settable") }
        return traits
    }

    func genericTextSummary(
        _ element: AXUIElement,
        role: String,
        name: String?,
        actions: [String],
        traits: [String]
    ) -> String? {
        guard (role == kAXGroupRole as String || role == kAXUnknownRole as String),
              name == nil,
              actions.isEmpty,
              traits.isEmpty,
              isPlainTextSubtree(element, maxDepth: 4)
        else {
            return nil
        }
        let texts = descendantTextSnippets(element, limit: 8, maxDepth: 4)
        guard texts.count >= 2 else { return nil }
        let summary = texts.joined(separator: " ")
        guard summary.count <= 220 else { return nil }
        return summary
    }

    func rowTextSummary(_ element: AXUIElement, role: String) -> String? {
        guard ["AXRow", "AXCell", "AXOutlineRow"].contains(role) else { return nil }
        let texts = descendantTextSnippets(element, limit: 6, maxDepth: 3)
        guard !texts.isEmpty else { return nil }
        return texts.joined(separator: " ")
    }

    func descendantTextSnippets(_ element: AXUIElement, limit: Int, maxDepth: Int) -> [String] {
        var values: [String] = []
        var seen = Set<String>()

        func collect(_ node: AXUIElement, depth: Int) {
            guard values.count < limit, depth <= maxDepth else { return }
            let role = stringAttribute(node, kAXRoleAttribute as String) ?? ""
            if role == kAXStaticTextRole as String || role == "AXLink" {
                for candidate in [
                    stringAttribute(node, kAXValueAttribute as String),
                    stringAttribute(node, kAXTitleAttribute as String),
                    stringAttribute(node, kAXDescriptionAttribute as String),
                ] {
                    guard let candidate else { continue }
                    let text = preview(candidate, maxLength: 80)
                    guard !text.isEmpty, seen.insert(text).inserted else { continue }
                    values.append(text)
                    if values.count >= limit { return }
                }
            }
            for child in copyArray(node, kAXChildrenAttribute as String) ?? [] {
                collect(child, depth: depth + 1)
                if values.count >= limit { return }
            }
        }

        collect(element, depth: 0)
        return values
    }

    func webAreaDepth(role: String, ancestors: [AXUIElement]) -> Int? {
        if role == "AXWebArea" { return 0 }
        guard let index = ancestors.firstIndex(where: { stringAttribute($0, kAXRoleAttribute as String) == "AXWebArea" }) else {
            return nil
        }
        return ancestors.count - index
    }

    private func copyAttribute(_ element: AXUIElement, _ attribute: String) -> CFTypeRef? {
        let cache = cache(for: element)
        if let cached = cache.attributes[attribute] {
            return cached.value
        }
        prefetchTable(cache)
        if let cached = cache.attributes[attribute] {
            return cached.value
        }
        return copyOneAttribute(cache, attribute)
    }

    /// The node's whole table, once, in a single call into the observed app.
    ///
    /// The batch asks the element outright, without first reading its
    /// attribute list. That list is the app's advertisement and some apps
    /// keep it short: KakaoTalk answers `AXEnabled: false` for plain
    /// containers it never lists, so the snapshot now says `(disabled)` on
    /// four rows it used to fold away — the app's own word, and the Windows
    /// provider already reads this way (`computer_use/windows/uia.rs` caches
    /// what the renderer asks for and never asks an element what it
    /// supports). Nothing a hand can reach moves: the same 26 controls take
    /// the same numbers on that window, measured, because a bare container
    /// was never a mark candidate.
    ///
    /// A read that does not come back whole — the call failed, or the answer
    /// does not line up with the request — caches nothing, and every
    /// attribute then falls back to the single read below, which is what the
    /// walk did before this existed.
    private func prefetchTable(_ cache: ElementCache) {
        guard !cache.prefetchedTable else { return }
        cache.prefetchedTable = true
        let attributes = SnapshotNodeAttributes.all
        var answered: CFArray?
        // No `.stopOnError`: one attribute the element cannot answer must
        // come back as one absent value, not cut the other fifteen short.
        guard AXUIElementCopyMultipleAttributeValues(
            cache.element,
            attributes as CFArray,
            [],
            &answered
        ) == .success,
            let values = answered as? [CFTypeRef],
            let decoded = SnapshotAttributeBatch.decode(attributes: attributes, values: values)
        else {
            return
        }
        for (attribute, value) in decoded {
            cache.attributes[attribute] = value.map(CachedAttribute.found) ?? .missing
        }
    }

    private func copyOneAttribute(_ cache: ElementCache, _ attribute: String) -> CFTypeRef? {
        if let advertisedAttributes = advertisedAttributes(cache),
           !SnapshotRenderHeuristics.supportsAttribute(attribute, advertisedAttributes: advertisedAttributes) {
            cache.attributes[attribute] = .missing
            return nil
        }
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(cache.element, attribute as CFString, &value) == .success,
              let value
        else {
            cache.attributes[attribute] = .missing
            return nil
        }
        cache.attributes[attribute] = .found(value)
        return value
    }

    private func advertisedAttributes(_ cache: ElementCache) -> Set<String>? {
        if cache.loadedAttributeNames {
            return cache.advertisedAttributes
        }
        cache.loadedAttributeNames = true
        var value: CFArray?
        guard AXUIElementCopyAttributeNames(cache.element, &value) == .success, let attributes = value as? [String] else {
            return nil
        }
        cache.advertisedAttributes = Set(attributes)
        return cache.advertisedAttributes
    }

    private func absoluteFrame(_ element: AXUIElement) -> CGRect? {
        guard let positionValue = copyAttribute(element, kAXPositionAttribute as String),
              let sizeValue = copyAttribute(element, kAXSizeAttribute as String)
        else {
            return nil
        }
        var point = CGPoint.zero
        var size = CGSize.zero
        guard AXValueGetValue(positionValue as! AXValue, .cgPoint, &point),
              AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
        else {
            return nil
        }
        return CGRect(origin: point, size: size)
    }

    private func visibleRows(_ rows: [AXUIElement], parent: AXUIElement, windowBounds: CGRect) -> [AXUIElement] {
        guard let parentFrame = frame(parent, windowBounds: windowBounds) else {
            return Array(rows.prefix(20))
        }
        let visible = rows.filter { row in
            guard let rowFrame = frame(row, windowBounds: windowBounds) else { return false }
            return rowFrame.intersects(parentFrame)
        }
        return Array((visible.isEmpty ? rows : visible).prefix(20))
    }

    private func isSecureTextElement(_ element: AXUIElement, role: String) -> Bool {
        guard SnapshotRenderHeuristics.shouldProbeSecureTextMetadata(role: role) else {
            return false
        }
        let haystack = [
            role,
            stringAttribute(element, kAXSubroleAttribute as String) ?? "",
            stringAttribute(element, kAXTitleAttribute as String) ?? "",
            stringAttribute(element, kAXDescriptionAttribute as String) ?? "",
            placeholderString(element) ?? "",
        ].joined(separator: " ")
        return SnapshotRenderHeuristics.looksLikeSecureText(haystack)
    }

    private func isPlainTextSubtree(_ element: AXUIElement, maxDepth: Int) -> Bool {
        var sawText = false
        let allowedContainerRoles: Set<String> = [
            kAXGroupRole as String,
            kAXUnknownRole as String,
            kAXStaticTextRole as String,
            "AXLink",
            "AXImage",
        ]

        func visit(_ node: AXUIElement, depth: Int) -> Bool {
            guard depth <= maxDepth else { return false }
            let role = stringAttribute(node, kAXRoleAttribute as String) ?? "AXUnknown"
            guard allowedContainerRoles.contains(role) else { return false }
            if role == kAXStaticTextRole as String || role == "AXLink" {
                sawText = true
            }
            guard SnapshotRenderHeuristics.meaningfulActions(actions(node), role: role).isEmpty else { return false }
            for child in copyArray(node, kAXChildrenAttribute as String) ?? [] {
                guard visit(child, depth: depth + 1) else { return false }
            }
            return true
        }

        return visit(element, depth: 0) && sawText
    }

    private func cache(for element: AXUIElement) -> ElementCache {
        let hash = CFHash(element)
        if let cache = elementsByHash[hash]?.first(where: { CFEqual($0.element, element) }) {
            return cache
        }
        let cache = ElementCache(element: element)
        elementsByHash[hash, default: []].append(cache)
        return cache
    }
}

private let valueSettableRoles: Set<String> = [
    kAXCheckBoxRole as String,
    kAXComboBoxRole as String,
    kAXRadioButtonRole as String,
    "AXSearchField",
    kAXSliderRole as String,
    kAXTextAreaRole as String,
    kAXTextFieldRole as String,
]

private func usesRowsAsPrimaryChildren(role: String) -> Bool {
    [
        kAXBrowserRole as String,
        kAXListRole as String,
        kAXOutlineRole as String,
        kAXTableRole as String,
    ].contains(role)
}

private func isDirectRenderedBrowserTabLine(_ line: String, indent: String) -> Bool {
    guard line.hasPrefix(indent), !line.dropFirst(indent.count).hasPrefix("\t") else {
        return false
    }
    let text = String(line.dropFirst(indent.count))
    return text.range(of: #"^\d+ tab($| \(|,)"#, options: .regularExpression) != nil
}

private func isActiveRenderedBrowserTabLine(_ line: String) -> Bool {
    line.contains("(selected") || line.contains("Value: 1")
}

private func renderedElementIndex(_ line: String, indent: String) -> Int? {
    let text = line.dropFirst(indent.count)
    let digits = text.prefix { character in
        character >= "0" && character <= "9"
    }
    return Int(digits)
}

private func sanitize(_ value: String) -> String {
    value.replacingOccurrences(of: "\n", with: " ").replacingOccurrences(of: "\r", with: " ")
}

private func preview(_ value: String, maxLength: Int = 120) -> String {
    let clean = sanitize(value)
    if clean.count <= maxLength {
        return clean
    }
    return String(clean.prefix(maxLength)) + "..."
}

private func actionMetadata(
    path: String,
    actionName: String? = nil,
    fallbackReason: String? = nil,
    verification: [String: Any]? = nil
) -> [String: Any] {
    var metadata: [String: Any] = [
        "path": path,
        "actionName": jsonNullable(actionName),
        "fallbackReason": jsonNullable(fallbackReason),
    ]
    if let verification {
        metadata["verification"] = verification
    }
    return metadata
}

private func verifiedAction(property: String, expected: String? = nil, actualPreview: String? = nil) -> [String: Any] {
    [
        "state": "verified",
        "property": property,
        "expected": jsonNullable(expected),
        "actualPreview": jsonNullable(actualPreview),
    ]
}

private func unverifiedAction(reason: String, expected: String? = nil, actualPreview: String? = nil) -> [String: Any] {
    [
        "state": "unverified",
        "reason": reason,
        "expected": jsonNullable(expected),
        "actualPreview": jsonNullable(actualPreview),
    ]
}

/// The keyboard's secure mode (TN2150): while a process holds it — a password
/// prompt, Terminal's Secure Keyboard Entry — keyboard events reach only the
/// focused app. Read, never set.
struct SecureInputState {
    /// The session dictionary's key for the holder's pid.
    static let holderKey = "kCGSSessionSecureInputPID"

    let on: Bool
    let holderPid: pid_t?
    let holder: String?

    static func now() -> SecureInputState {
        guard IsSecureEventInputEnabled() else {
            return SecureInputState(on: false, holderPid: nil, holder: nil)
        }
        let session = CGSessionCopyCurrentDictionary() as? [String: Any]
        let pid = (session?[holderKey] as? NSNumber).map { pid_t($0.int32Value) }.flatMap { $0 > 0 ? $0 : nil }
        return SecureInputState(
            on: true,
            holderPid: pid,
            holder: pid.flatMap { NSRunningApplication(processIdentifier: $0)?.localizedName }
        )
    }

    var rendered: Any {
        guard on else { return NSNull() }
        return ["on": true, "holderPid": holderPid.map { Int($0) } ?? NSNull(), "holder": jsonNullable(holder)] as [String: Any]
    }
}

/// An answer that says who holds the keyboard's secure mode, when anyone does.
private func withSecureInput(_ answer: [String: Any], _ state: SecureInputState) -> [String: Any] {
    guard state.on else { return answer }
    var answer = answer
    answer["secureInput"] = state.rendered
    return answer
}

/// The element that has the keyboard focus, whatever app it is in.
private func systemWideFocusedElement() -> AXUIElement? {
    copyElement(AXUIElementCreateSystemWide(), kAXFocusedUIElementAttribute as String)
}

/// The element that has the keyboard focus inside one app, read live.
private func appFocusedElement(_ pid: pid_t) -> AXUIElement? {
    copyElement(AXUIElementCreateApplication(pid), kAXFocusedUIElementAttribute as String)
}

/// Whether the focus is a multi-line text area, where Tabs and line breaks
/// are content, not moves.
private func isMultiLineText(_ element: AXUIElement) -> Bool {
    rawStringAttribute(element, kAXRoleAttribute as String) == (kAXTextAreaRole as String)
}

/// Whether an element's role, title, label or placeholder says it holds a
/// secret (the snapshot's own redaction words): what it shows is never
/// repeated in an answer.
private func looksLikeSecret(_ element: AXUIElement) -> Bool {
    let words = [kAXRoleAttribute, kAXSubroleAttribute, kAXTitleAttribute, kAXDescriptionAttribute, kAXHelpAttribute, "AXPlaceholderValue"]
        .compactMap { rawStringAttribute(element, $0 as String) }
        .joined(separator: " ")
    return isSecretField(element) || SnapshotRenderHeuristics.looksLikeSecureText(words)
}

/// The name of the app an element belongs to — the frontmost app's when the
/// element cannot say.
private func focusedAppName(_ element: AXUIElement?) -> String {
    let pid = element.flatMap(pidAttribute) ?? NSWorkspace.shared.frontmostApplication?.processIdentifier
    return pid.flatMap { NSRunningApplication(processIdentifier: $0)?.localizedName } ?? "the focused app"
}

/// Whether an element is the platform's own secret field — the hard signal
/// that refuses typed text, never a label's word.
private func isSecretField(_ element: AXUIElement?) -> Bool {
    guard let element else { return false }
    return rawStringAttribute(element, kAXSubroleAttribute as String) == (kAXSecureTextFieldSubrole as String)
}

/// The window's keyboard table, when the request carries it.
private func keyboardGuardConfig(_ params: [String: JSONValue]) -> KeyboardGuardConfig? {
    guard case let .object(table)? = params["keyboardGuard"] else { return nil }
    return KeyboardGuardConfig(settleMs: table["settleMs"]?.number, pollMs: table["pollMs"]?.number, maxChars: table["maxChars"]?.number)
}

private func jsonNullable<T>(_ value: T?) -> Any {
    value ?? NSNull()
}

private struct WindowCandidate {
    let windowId: CGWindowID
    let layer: Int
    let bounds: CGRect
    let title: String?
    let alpha: CGFloat
    let isOnScreen: Bool
    let sharingState: Int?

    var score: Int {
        var value = Int(bounds.width * bounds.height)
        if layer == 0 { value += 1_000_000_000 }
        if title != nil && title?.isEmpty == false { value += 10_000_000 }
        if isOnScreen { value += 1_000_000 }
        if alpha >= 0.99 { value += 100_000 }
        return value
    }
}

private struct WindowCapture {
    let windowId: CGWindowID
    let layer: Int
    let bounds: CGRect
    let title: String?
    let image: CapturedImage?

    static func resolve(
        pid: pid_t,
        titleHint: String?,
        windowId: CGWindowID?,
        windowIndex: Int?,
        captureImage: Bool
    ) -> WindowCapture? {
        resolve(
            candidates: candidates(pid: pid),
            titleHint: titleHint,
            windowId: windowId,
            windowIndex: windowIndex,
            captureImage: captureImage
        )
    }

    static func resolve(
        candidates: [WindowCandidate],
        titleHint: String?,
        windowId: CGWindowID?,
        windowIndex: Int?,
        captureImage: Bool
    ) -> WindowCapture? {
        if let windowId {
            guard let candidate = candidates.first(where: { $0.windowId == windowId }) else { return nil }
            return WindowCapture(candidate: candidate, captureImage: captureImage)
        }
        if let windowIndex {
            let visibleWindows = candidates.filter { $0.layer == 0 }
            guard visibleWindows.indices.contains(windowIndex) else { return nil }
            return WindowCapture(candidate: visibleWindows[windowIndex], captureImage: captureImage)
        }
        guard let best = candidates.sorted(by: { lhs, rhs in
            if let titleHint, lhs.title == titleHint, rhs.title != titleHint { return true }
            if let titleHint, rhs.title == titleHint, lhs.title != titleHint { return false }
            return lhs.score > rhs.score
        }).first else {
            return nil
        }
        return WindowCapture(candidate: best, captureImage: captureImage)
    }

    private init(candidate: WindowCandidate, captureImage: Bool) {
        self.windowId = candidate.windowId
        self.layer = candidate.layer
        self.bounds = candidate.bounds
        self.title = candidate.title
        // Why: probing image APIs before TCC preflight can raise Screen
        // Recording prompts, even for --no-screenshot calls.
        if captureImage {
            self.image = Self.captureImage(windowId: candidate.windowId, bounds: candidate.bounds)
        } else {
            self.image = nil
        }
    }

    static func candidates(pid: pid_t) -> [WindowCandidate] {
        guard let infos = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
            return []
        }
        return infos.compactMap { info in
            guard let ownerPid = info[kCGWindowOwnerPID as String] as? pid_t, ownerPid == pid,
                  let number = info[kCGWindowNumber as String] as? NSNumber,
                  let layer = info[kCGWindowLayer as String] as? Int,
                  let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDictionary),
                  bounds.width >= 48,
                  bounds.height >= 48
            else {
                return nil
            }
            let alpha = info[kCGWindowAlpha as String] as? CGFloat ?? 1
            guard alpha > 0.01 else { return nil }
            let sharing = (info[kCGWindowSharingState as String] as? NSNumber).map { $0.intValue }
            if sharing == 0 { return nil }
            let isOnScreen = (info[kCGWindowIsOnscreen as String] as? Bool) ?? true
            return WindowCandidate(
                windowId: CGWindowID(number.uint32Value),
                layer: layer,
                bounds: bounds,
                title: info[kCGWindowName as String] as? String,
                alpha: alpha,
                isOnScreen: isOnScreen,
                sharingState: sharing
            )
        }
        .sorted { lhs, rhs in
            lhs.score > rhs.score
        }
    }

    func screenshotPayload() -> ScreenshotPayload? {
        guard let image, let bounded = boundedPngData(image.image) else { return nil }
        return ScreenshotPayload(
            data: bounded.data.base64EncodedString(),
            width: bounded.width,
            height: bounded.height,
            scale: Double(bounded.width) / max(Double(bounds.width), 1)
        )
    }

    static func captureImage(windowId: CGWindowID, bounds: CGRect) -> CapturedImage? {
        if ProcessInfo.processInfo.environment["ZEROCODE_COMPUTER_USE_SCK_SCREENSHOTS"] == "1",
           let image = captureImageWithScreenCaptureKit(windowId: windowId, bounds: bounds) {
            return CapturedImage(image: image, engine: "screenCaptureKit")
        }
        if let image = CGWindowListCreateImage(.null, [.optionIncludingWindow], windowId, [.boundsIgnoreFraming, .bestResolution]) {
            return CapturedImage(image: image, engine: "cgWindowList")
        }
        return nil
    }

    private static func captureImageWithScreenCaptureKit(windowId: CGWindowID, bounds: CGRect) -> CGImage? {
        try? BlockingAsync.run(timeout: 3) {
            let content = try await SCShareableContent.current
            guard let window = content.windows.first(where: { $0.windowID == windowId }) else {
                return nil
            }
            let filter = SCContentFilter(desktopIndependentWindow: window)
            let configuration = SCStreamConfiguration()
            let scale = NSScreen.screens.first(where: { $0.frame.intersects(bounds) })?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
            configuration.width = size_t(max(1, Int((bounds.width * scale).rounded(.up))))
            configuration.height = size_t(max(1, Int((bounds.height * scale).rounded(.up))))
            configuration.scalesToFit = true
            configuration.preservesAspectRatio = true
            configuration.showsCursor = false
            configuration.ignoreShadowsSingleWindow = true
            configuration.ignoreGlobalClipSingleWindow = true
            return try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
        }
    }
}

private final class AsyncBox<T>: @unchecked Sendable {
    var result: Result<T, Error>?
}

enum BlockingAsync {
    /// Run `operation` to its end on this thread, or refuse as `action_timeout`
    /// naming `what` when it takes longer than `timeout`.
    static func run<T>(
        timeout: TimeInterval,
        what: String = "screenshot capture",
        operation: @escaping @Sendable () async throws -> T
    ) throws -> T {
        let semaphore = DispatchSemaphore(value: 0)
        let box = AsyncBox<T>()
        let task = Task.detached {
            do {
                box.result = .success(try await operation())
            } catch {
                box.result = .failure(error)
            }
            semaphore.signal()
        }
        guard semaphore.wait(timeout: .now() + timeout) == .success else {
            task.cancel()
            throw ProviderError.coded("action_timeout", "\(what) timed out")
        }
        return try box.result!.get()
    }
}

struct BoundedPNG {
    let data: Data
    let width: Int
    let height: Int
}

/// The ladder's rungs in order, each encoded once, the first that fits kept —
/// the smallest rung when none does (`ScreenshotBudget`).
func boundedPngData(_ image: CGImage) -> BoundedPNG? {
    var best: BoundedPNG?
    for scale in ScreenshotBudget.ladder(width: image.width, height: image.height) {
        guard let rung = scale < 1 ? resizePng(image, scale: scale) : encodePng(image) else {
            break
        }
        best = rung
        if rung.data.count <= ScreenshotBudget.maxPngBytes {
            return rung
        }
    }
    return best
}

private func encodePng(_ image: CGImage) -> BoundedPNG? {
    guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else {
        return nil
    }
    return BoundedPNG(data: data, width: image.width, height: image.height)
}

private func resizePng(_ image: CGImage, scale: CGFloat) -> BoundedPNG? {
    // Rounded, as the Rust mirror rounds: 3024 x (1280 / 3024) is 1280, not 1279.
    let width = max(1, Int((CGFloat(image.width) * scale).rounded()))
    let height = max(1, Int((CGFloat(image.height) * scale).rounded()))
    guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        return nil
    }
    context.interpolationQuality = .medium
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    guard let resized = context.makeImage() else { return nil }
    return encodePng(resized)
}

private enum Input {
    static func click(
        at point: CGPoint,
        button: MouseButtonSelection,
        count: Int,
        modifiers: [KeyModifier],
        targetWindow: Snapshot
    ) throws {
        let flags = modifiers.reduce(into: CGEventFlags()) { result, modifier in
            result.insert(modifier.flag)
        }
        let target = syntheticClickRecipient(pid: targetWindow.app.pid, windowId: targetWindow.windowId)
        do {
            // The fenced click (STA-3433) on the one hand: every event through
            // it, and the pause a wait a stop ends at once.
            try SyntheticMouseClickDelivery.deliver(
                clickCount: count,
                target: target,
                currentObservation: {
                    currentSyntheticClickRecipient(snapshot: targetWindow, point: point)
                },
                makeEvent: { step in
                    let clickState = SyntheticMouseClickDelivery.clickState(for: step)
                    let kind: HandEvent.Kind
                    switch step {
                    case .move:
                        kind = .pointerMove
                    case .buttonDown:
                        kind = .buttonDown(button, clickState: clickState)
                    case .buttonUp:
                        kind = .buttonUp(button, clickState: clickState)
                    }
                    return HandEvent(kind, x: point.x, y: point.y, flags: flags.rawValue, route: .desktop, source: .session)
                },
                post: { try OperatorHandHost.post($0) },
                pause: { try OperatorHandHost.sleep(nanoseconds: UInt64($0) * 1_000) }
            )
        } catch let failure as SyntheticMouseClickDelivery.FenceFailure {
            switch failure {
            case let .recipientChanged(expected, actual, deliveredPresses):
                let actualDescription = actual.map {
                    "pid \($0.ownerPID) window \($0.windowID)"
                } ?? "no focused window"
                let recovery = deliveredPresses == 0
                    ? "bring the target window forward, run get-app-state again, and retry"
                    : "\(deliveredPresses) press(es) may already have been delivered; run get-app-state and verify state before retrying"
                throw ProviderError.coded(
                    "window_not_focused",
                    "coordinate click aborted because target pid \(expected.ownerPID) window \(expected.windowID) is no longer the focused topmost recipient (current: \(actualDescription)); \(recovery)"
                )
            }
        }
    }

    static func scroll(pid: pid_t, at point: CGPoint, direction: String, pages: Double) throws {
        guard let delta = boundedInteger(max(1, (12 * pages).rounded()), as: Int32.self) else {
            throw ProviderError.coded("invalid_argument", "pages is out of range")
        }
        let wheel1: Int32 = direction == "up" ? delta : direction == "down" ? -delta : 0
        let wheel2: Int32 = direction == "left" ? delta : direction == "right" ? -delta : 0
        try OperatorHandHost.post(HandEvent(.scroll(wheel1: wheel1, wheel2: wheel2), x: point.x, y: point.y, route: .process(pid)))
    }

    static func drag(pid: pid_t, from start: CGPoint, to end: CGPoint) throws {
        try mouse(.pointerMove, at: start, pid: pid)
        try mouse(.buttonDown(.left, clickState: 0), at: start, pid: pid)
        var reached = start
        do {
            for step in 1...10 {
                let progress = CGFloat(step) / 10
                let point = CGPoint(x: start.x + (end.x - start.x) * progress, y: start.y + (end.y - start.y) * progress)
                try mouse(.buttonDrag(.left, clickState: 0), at: point, pid: pid)
                reached = point
            }
        } catch {
            // A stop mid-drag has already let go (`OperatorHand.stop`); any
            // other failure lets go where the pointer is, then reports.
            try? mouse(.buttonUp(.left, clickState: 0), at: reached, pid: pid)
            throw error
        }
        try mouse(.buttonUp(.left, clickState: 0), at: end, pid: pid)
    }

    static func typeText(_ text: String, pid: pid_t) throws {
        for unit in text.utf16 {
            // A stop lands between two characters; one inside a press lets
            // go of it at once.
            try OperatorHandHost.post(HandEvent(.text(unit: unit, down: true)))
            try OperatorHandHost.post(HandEvent(.text(unit: unit, down: false)))
        }
    }

    static func pressKey(_ key: String, pid: pid_t) throws {
        let parsed = try KeyMap.parse(key)
        var flags = CGEventFlags()
        var pressedModifiers: [KeyModifier] = []
        defer {
            for modifier in pressedModifiers.reversed() {
                flags.remove(modifier.flag)
                try? keyEvent(modifier.keyCode, down: false, flags: flags, pid: pid)
            }
        }
        for modifier in parsed.modifiers {
            flags.insert(modifier.flag)
            try keyEvent(modifier.keyCode, down: true, flags: flags, pid: pid, holding: modifier.flag)
            pressedModifiers.append(modifier)
        }
        try keyEvent(parsed.keyCode, down: true, flags: flags, pid: pid)
        try keyEvent(parsed.keyCode, down: false, flags: flags, pid: pid)
    }

    /// How long a posted ⌘V is given to be consumed before the previous
    /// clipboard is put back. The key event is asynchronous: restoring in the
    /// same breath handed the target the OLD clipboard (2026-09-10, a terminal
    /// pane pasted a stale URL in place of the text).
    static let pasteSettleSeconds: TimeInterval = 0.25

    static func pasteText(_ text: String, pid: pid_t) throws {
        let pasteboard = NSPasteboard.general
        let previousItems: [NSPasteboardItem] = pasteboard.pasteboardItems?.map { item in
            let copy = NSPasteboardItem()
            for type in item.types {
                if let data = item.data(forType: type) {
                    copy.setData(data, forType: type)
                }
            }
            return copy
        } ?? []
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        defer {
            pasteboard.clearContents()
            if !previousItems.isEmpty {
                pasteboard.writeObjects(previousItems)
            }
        }
        try pressKey("cmd+v", pid: pid)
        Thread.sleep(forTimeInterval: pasteSettleSeconds)
    }

    /// A mouse event for one process, made with the combined session state
    /// as the app verbs always made it.
    private static func mouse(_ kind: HandEvent.Kind, at point: CGPoint, flags: CGEventFlags = [], pid: pid_t) throws {
        try OperatorHandHost.post(HandEvent(kind, x: point.x, y: point.y, flags: flags.rawValue, route: .process(pid), source: .session))
    }

    /// A key on the desktop; a modifier key names the flag it holds while down.
    private static func keyEvent(_ keyCode: CGKeyCode, down: Bool, flags: CGEventFlags, pid: pid_t, holding modifier: CGEventFlags = []) throws {
        try OperatorHandHost.post(HandEvent(.key(code: keyCode, down: down, modifier: modifier.rawValue), flags: flags.rawValue))
    }
}

/// What `TextInput.replaceSelection` did with the text.
private enum ReplaceSelection {
    /// Nothing landed — the caller may type or paste instead.
    case unavailable
    /// The text landed once; `verification` says whether the field still
    /// reads it back (`verified`) or holds something else (`unverified`,
    /// `value_mismatch` / `readback_unsupported` — the shared core's words). Either way a synthetic fallback would be a second copy.
    case applied(verification: [String: Any])
}

private enum TextInput {
    static func replaceSelection(_ element: AXUIElement, with text: String) -> ReplaceSelection {
        guard isSettable(element, kAXValueAttribute as String),
              let current = rawStringAttribute(element, kAXValueAttribute as String)
        else {
            return .unavailable
        }
        let selectedRange = selectedTextRange(element) ?? CFRange(location: current.utf16.count, length: 0)
        let startOffset = max(0, min(selectedRange.location, current.utf16.count))
        let endOffset = max(startOffset, min(startOffset + selectedRange.length, current.utf16.count))
        let start = String.Index(utf16Offset: startOffset, in: current)
        let end = String.Index(utf16Offset: endOffset, in: current)
        let next = String(current[..<start]) + text + String(current[end...])
        let setSucceeded = AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, next as CFString) == .success
        if setSucceeded {
            setSelectedTextRange(element, CFRange(location: startOffset + text.utf16.count, length: 0))
        }
        let readback = setSucceeded ? rawStringAttribute(element, kAXValueAttribute as String) : nil
        let outcome = TextReplaceOutcome.judge(setSucceeded: setSucceeded, expected: next, readback: readback)
        // A terminal's key sink drains the insert to the pty and clears
        // itself; an autocomplete rewrites. Anything but unavailable landed once.
        guard outcome != .unavailable else { return .unavailable }
        return .applied(verification: verification(outcome, expected: text, shows: readback, hidden: looksLikeSecret(element)))
    }

    /// The verification an action reports for what a text write did: the
    /// outcome's own words, the text asked for, and what the field shows —
    /// never what a field that looks like a secret's holds.
    static func verification(_ outcome: TextReplaceOutcome, expected: String, shows: String?, hidden: Bool) -> [String: Any] {
        let shown = hidden ? nil : shows.map { preview($0) }
        guard let reason = outcome.unverifiedReason else {
            return verifiedAction(property: "focusedText", expected: expected, actualPreview: shown)
        }
        return unverifiedAction(reason: reason, expected: expected, actualPreview: shown)
    }

    /// What a field holds before keys are typed into it, and what it should
    /// hold after — read only when its value is short enough to read back.
    struct Landing {
        let element: AXUIElement
        let before: String
        let expected: String
    }

    static func landingStart(_ element: AXUIElement, typing text: String, maxChars: Int) -> Landing? {
        guard let current = rawStringAttribute(element, kAXValueAttribute as String), current.count <= maxChars else {
            return nil
        }
        let selectedRange = selectedTextRange(element) ?? CFRange(location: current.utf16.count, length: 0)
        let startOffset = max(0, min(selectedRange.location, current.utf16.count))
        let endOffset = max(startOffset, min(startOffset + selectedRange.length, current.utf16.count))
        let start = String.Index(utf16Offset: startOffset, in: current)
        let end = String.Index(utf16Offset: endOffset, in: current)
        return Landing(element: element, before: current, expected: String(current[..<start]) + text + String(current[end...]))
    }

    /// The field read until it shows the typed keys or the settle passes; the
    /// judgement and what it showed last.
    static func landingFinish(_ landing: Landing, config: KeyboardGuardConfig) -> (TextReplaceOutcome, String?) {
        var waited = 0
        var shows = rawStringAttribute(landing.element, kAXValueAttribute as String)
        while shows != landing.expected && waited < config.settleMs {
            usleep(useconds_t(config.pollMs * 1_000))
            waited += config.pollMs
            shows = rawStringAttribute(landing.element, kAXValueAttribute as String)
        }
        return (TextReplaceOutcome.judgeTyped(before: landing.before, expected: landing.expected, after: shows), shows)
    }

    static func selectAll(_ element: AXUIElement) -> Bool {
        guard let current = rawStringAttribute(element, kAXValueAttribute as String) else {
            return false
        }
        return setSelectedTextRange(element, CFRange(location: 0, length: current.utf16.count))
    }

    static func selectionVerification(_ element: AXUIElement) -> [String: Any] {
        guard let current = rawStringAttribute(element, kAXValueAttribute as String),
              let selectedRange = selectedTextRange(element),
              selectedRange.location == 0,
              selectedRange.length == current.utf16.count
        else {
            return unverifiedAction(reason: "provider_unavailable")
        }
        return verifiedAction(property: "selection", actualPreview: preview(current))
    }

    private static func selectedTextRange(_ element: AXUIElement) -> CFRange? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &value) == .success,
              let value,
              CFGetTypeID(value) == AXValueGetTypeID()
        else {
            return nil
        }
        var range = CFRange(location: 0, length: 0)
        guard AXValueGetValue(value as! AXValue, .cfRange, &range) else {
            return nil
        }
        return range
    }

    @discardableResult
    private static func setSelectedTextRange(_ element: AXUIElement, _ range: CFRange) -> Bool {
        var mutableRange = range
        guard let value = AXValueCreate(.cfRange, &mutableRange) else {
            return false
        }
        return AXUIElementSetAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, value) == .success
    }
}

private func isSelectAllHotkey(_ key: String) -> Bool {
    isPrimaryHotkey(key, letter: "a")
}

/// A letter with the platform's primary modifier held (the core's
/// `is_primary_hotkey`), by the one table of what names ⌘.
private func isPrimaryHotkey(_ key: String, letter: String) -> Bool {
    let parts = key
        .lowercased()
        .replacingOccurrences(of: " ", with: "")
        .replacingOccurrences(of: "-", with: "+")
        .split(separator: "+")
        .map(String.init)
    guard parts.last == letter else {
        return false
    }
    return parts.dropLast().contains { KeyboardInputSafety.primaryModifierNames.contains($0) }
}

private struct KeyModifier {
    let keyCode: CGKeyCode
    let flag: CGEventFlags
}

private struct ParsedKey {
    let keyCode: CGKeyCode
    let modifiers: [KeyModifier]
}

private enum KeyMap {
    static func parse(_ spec: String) throws -> ParsedKey {
        let parts = spec.split(separator: "+").map { String($0).lowercased() }
        var modifiers: [KeyModifier] = []
        var keyName: String?
        for part in parts {
            if let modifier = modifier(part) {
                modifiers.append(modifier)
            } else {
                keyName = part
            }
        }
        guard let keyName, let keyCode = codes[keyName] else {
            throw ProviderError.coded("invalid_argument", "unsupported key '\(spec)'")
        }
        return ParsedKey(keyCode: keyCode, modifiers: modifiers)
    }

    static func parseModifiers(_ spec: String?) throws -> [KeyModifier] {
        guard let spec else {
            return []
        }
        let parts = spec.split(separator: "+", omittingEmptySubsequences: false)
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
        guard !parts.isEmpty, !parts.contains(where: \.isEmpty) else {
            throw ProviderError.coded("invalid_argument", "click modifiers require modifier keys only")
        }
        return try parts.map { part in
            guard let modifier = modifier(part) else {
                throw ProviderError.coded("invalid_argument", "unsupported click modifier '\(part)'")
            }
            return modifier
        }
    }

    private static func modifier(_ part: String) -> KeyModifier? {
        switch part {
        case "cmd", "command", "meta", "super", "win", "cmdorctrl", "commandorcontrol":
            return KeyModifier(keyCode: 55, flag: .maskCommand)
        case "ctrl", "control":
            return KeyModifier(keyCode: 59, flag: .maskControl)
        case "alt", "option":
            return KeyModifier(keyCode: 58, flag: .maskAlternate)
        case "shift":
            return KeyModifier(keyCode: 56, flag: .maskShift)
        default:
            return nil
        }
    }

    private static let codes: [String: CGKeyCode] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
        "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
        "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28,
        "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "return": 36,
        "enter": 36, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43,
        "/": 44, "n": 45, "m": 46, ".": 47, "tab": 48, "space": 49, "`": 50,
        "backspace": 51, "delete": 51, "escape": 53, "esc": 53, "left": 123, "right": 124,
        "down": 125, "up": 126, "insert": 114, "home": 115, "pageup": 116, "page_up": 116,
        "forwarddelete": 117, "end": 119, "pagedown": 121, "page_down": 121,
    ]
}

/// What the helper stands up before it answers anyone (realtime v1 §6): the
/// operator's stop and count, then the perception kernel its reflex runs read
/// with — installed here, once, so a helper that answers can run a plan.
enum HelperLaunch {
    static func install(operatorGuard: () -> Void = OperatorGuardHost.install) {
        operatorGuard()
        ReflexRuntimeHost.install(kernel: PerceptionKernel())
    }
}

extension StopReason {
    /// The window's session closed (`OperatorGuardHost.sessionClosed`).
    static let sessionClosed = "session_closed"
}

private final class AgentRuntime: NSObject, NSApplicationDelegate {
    private static let unclaimedSessionDeadline: TimeInterval = 30

    private let socketPath: String
    private let token: String?
    private var listener: SocketListener?
    private var unclaimedSessionTimeout: DispatchWorkItem?

    init(socketPath: String, token: String?) {
        self.socketPath = socketPath
        self.token = token
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        HelperLaunch.install()
        do {
            let timeout = DispatchWorkItem {
                fputs("computer-use agent received no authenticated session before its deadline\n", stderr)
                NSApp.terminate(nil)
            }
            unclaimedSessionTimeout = timeout
            let listener = try SocketListener(
                socketPath: socketPath,
                token: token,
                onSessionClaimed: {
                    timeout.cancel()
                },
                onSessionClosed: {
                    OperatorGuardHost.sessionClosed {
                        DispatchQueue.main.async {
                            NSApp.terminate(nil)
                        }
                    }
                }
            )
            self.listener = listener
            listener.start()
            DispatchQueue.main.asyncAfter(
                deadline: .now() + Self.unclaimedSessionDeadline,
                execute: timeout
            )
        } catch {
            fputs("failed to start computer-use socket: \(error)\n", stderr)
            NSApp.terminate(nil)
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        unclaimedSessionTimeout?.cancel()
        unclaimedSessionTimeout = nil
        listener?.stop()
    }
}

private final class PermissionRuntime: NSObject, NSApplicationDelegate {
    private let initialPermission: PermissionKind?
    private var windowController: PermissionWindowController?

    init(initialPermission: PermissionKind?) {
        self.initialPermission = initialPermission
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        windowController = PermissionWindowController(
            initialPermission: initialPermission,
            terminateWhenDragAssistantCloses: initialPermission != nil
        )
        if let initialPermission {
            windowController?.openPermission(initialPermission)
        } else {
            windowController?.showWindow(nil)
            windowController?.window?.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        windowController?.refreshPermissions()
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        windowController?.refreshPermissions()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        initialPermission == nil
    }
}

private final class PermissionWindowController: NSWindowController {
    private var dragAssistant: PermissionDragAssistantController?
    private var dragAssistantPermission: PermissionKind?
    private let initialPermission: PermissionKind?
    private let terminateWhenDragAssistantCloses: Bool
    private var permissionStatusRefresh: PermissionStatusRefreshCoordinator?

    convenience init(initialPermission: PermissionKind? = nil, terminateWhenDragAssistantCloses: Bool = false) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 315),
            styleMask: [.titled, .closable, .miniaturizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Enable ZeroCode Computer Use"
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.backgroundColor = PermissionPalette.background
        window.center()
        window.isReleasedWhenClosed = false
        self.init(window: window, initialPermission: initialPermission, terminateWhenDragAssistantCloses: terminateWhenDragAssistantCloses)
        window.contentView = PermissionView(
            frame: window.contentView?.bounds ?? .zero,
            showDragAssistant: { [weak self] permission in
                self?.showDragAssistant(for: permission)
            },
            close: { [weak self] in
                self?.closePermissionWindow()
            }
        )
    }

    init(window: NSWindow?, initialPermission: PermissionKind?, terminateWhenDragAssistantCloses: Bool) {
        self.initialPermission = initialPermission
        self.terminateWhenDragAssistantCloses = terminateWhenDragAssistantCloses
        super.init(window: window)
        permissionStatusRefresh = PermissionStatusRefreshCoordinator(
            probe: permissionStatusSnapshotSettled,
            handler: { [weak self] snapshot in
                Task { @MainActor in
                    self?.applyPermissionStatus(snapshot)
                }
            }
        )
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func showDragAssistant(for permission: PermissionKind) {
        dragAssistant?.close()
        dragAssistantPermission = permission
        dragAssistant = PermissionDragAssistantController(
            permission: permission,
            fallbackVisibleFrame: window?.screen?.visibleFrame,
            onRefreshPermissions: { [weak self] in
                self?.refreshPermissions()
            },
            onClose: { [weak self] in
                if self?.terminateWhenDragAssistantCloses == true {
                    NSApp.terminate(nil)
                }
            }
        )
        dragAssistant?.showWhenReady()
    }

    private func closeDragAssistant() {
        dragAssistant?.close()
        dragAssistant = nil
        dragAssistantPermission = nil
    }

    private func completeDragAssistant() {
        guard let dragAssistant else {
            dragAssistantPermission = nil
            if terminateWhenDragAssistantCloses {
                NSApp.terminate(nil)
            }
            return
        }
        self.dragAssistant = nil
        dragAssistantPermission = nil
        dragAssistant.complete()
    }

    private func closePermissionWindow() {
        // Why: the floating assistant is a separate retained window controller and
        // can keep the helper app alive after the main permission window closes.
        closeDragAssistant()
        window?.close()
    }

    func openPermission(_ permission: PermissionKind) {
        permission.requestAndOpenSettings()
        showDragAssistant(for: permission)
    }

    func refreshPermissions() {
        permissionStatusRefresh?.refresh()
    }

    private func applyPermissionStatus(_ snapshot: PermissionStatusSnapshot) {
        if let initialPermission, initialPermission.isGranted(in: snapshot) {
            // Why: targeted permission helpers should finish once the requested
            // grant lands, even if other Computer Use permissions remain unset.
            completeDragAssistant()
            return
        }
        if dragAssistantPermission?.isGranted(in: snapshot) == true {
            // Why: after one grant in full setup, the remaining missing permission
            // needs fresh guidance instead of the old assistant's instructions.
            closeDragAssistant()
        }
        if PermissionKind.allCases.allSatisfy({ $0.isGranted(in: snapshot) }) {
            closeDragAssistant()
        }
        (window?.contentView as? PermissionView)?.refreshPermissions(snapshot)
    }
}

private enum PermissionKind: CaseIterable {
    case accessibility
    case screenshots

    static func parse(_ value: String?) -> PermissionKind? {
        switch value {
        case "accessibility":
            .accessibility
        case "screenshots", "screen", "screen-recording":
            .screenshots
        default:
            nil
        }
    }

    var dragInstruction: String {
        switch self {
        case .accessibility:
            "Drag ZeroCode Computer Use into the list above to allow Accessibility."
        case .screenshots:
            "Drag ZeroCode into the list above to allow Screen Recording — that list judges the app, not this helper."
        }
    }

    var title: String {
        switch self {
        case .accessibility:
            "Accessibility"
        case .screenshots:
            "Screenshots"
        }
    }

    var detail: String {
        switch self {
        case .accessibility:
            "Read and control app interfaces"
        case .screenshots:
            "Capture windows for visual state"
        }
    }

    var icon: NSImage {
        switch self {
        case .accessibility:
            NSImage(systemSymbolName: "figure", accessibilityDescription: "Accessibility") ?? NSImage()
        case .screenshots:
            NSImage(systemSymbolName: "camera.viewfinder", accessibilityDescription: "Screen Recording") ?? NSImage()
        }
    }

    func isGranted(in snapshot: PermissionStatusSnapshot) -> Bool {
        switch self {
        case .accessibility:
            snapshot.accessibilityGranted
        case .screenshots:
            snapshot.screenshotsGranted
        }
    }

    func requestAccess() {
        switch self {
        case .accessibility: _ = requestAccessibilityAccess()
        case .screenshots: _ = requestScreenCaptureAccess()
        }
    }

    func requestAndOpenSettings() {
        requestAccess()
        switch self {
        case .accessibility:
            openAccessibilitySettings()
        case .screenshots:
            openScreenRecordingSettings()
        }
    }
}

private final class PermissionView: NSView {
    private let appURL = Bundle.main.bundleURL
    private let showDragAssistant: (PermissionKind) -> Void
    private let close: () -> Void
    private var contentStack: NSStackView?
    private var contentConstraints: [NSLayoutConstraint] = []
    private var permissionStatus: PermissionStatusSnapshot?

    init(frame frameRect: NSRect, showDragAssistant: @escaping (PermissionKind) -> Void, close: @escaping () -> Void) {
        self.showDragAssistant = showDragAssistant
        self.close = close
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.backgroundColor = PermissionPalette.background.cgColor
        build()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func build() {
        NSLayoutConstraint.deactivate(contentConstraints)
        contentStack?.removeFromSuperview()
        contentConstraints = []

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.distribution = .gravityAreas
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        contentStack = stack

        let icon = NSImageView(image: NSWorkspace.shared.icon(forFile: appURL.path))
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 58),
            icon.heightAnchor.constraint(equalToConstant: 58)
        ])

        let missingPermissions = permissionStatus.map { snapshot in
            PermissionKind.allCases.filter { !$0.isGranted(in: snapshot) }
        } ?? []
        let checking = permissionStatus == nil
        let ready = !checking && missingPermissions.isEmpty

        let titleText = checking
            ? "Checking Computer Use"
            : (ready ? "Computer Use is Ready" : "Enable ZeroCode Computer Use")
        let title = label(titleText, size: 22, weight: .bold)
        let subtitle = label(
            checking
                ? "Checking Accessibility and Screenshots."
                : (ready
                    ? "ZeroCode can use local apps when you ask."
                    : "Grant permissions so ZeroCode can use apps when you ask."),
            size: 12,
            weight: .regular
        )
        subtitle.textColor = PermissionPalette.secondaryText
        subtitle.alignment = .center
        subtitle.maximumNumberOfLines = 3

        let header = NSStackView(views: [icon, title, subtitle])
        header.orientation = .vertical
        header.alignment = .centerX
        header.spacing = 6
        header.translatesAutoresizingMaskIntoConstraints = false
        stack.addArrangedSubview(header)
        header.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        subtitle.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -10).isActive = true

        if checking {
            let progress = NSProgressIndicator()
            progress.style = .spinning
            progress.controlSize = .small
            progress.translatesAutoresizingMaskIntoConstraints = false
            progress.startAnimation(nil)
            stack.addArrangedSubview(progress)
            progress.setContentHuggingPriority(.required, for: .horizontal)
            progress.centerXAnchor.constraint(equalTo: stack.centerXAnchor).isActive = true
        } else if ready {
            stack.addArrangedSubview(doneButton())
        } else {
            for permission in missingPermissions {
                stack.addArrangedSubview(permissionRow(permission: permission) { [weak self] in
                    permission.requestAndOpenSettings()
                    self?.showDragAssistant(permission)
                })
            }
        }

        contentConstraints = [
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -18),
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 22),
            stack.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor, constant: -20)
        ]
        NSLayoutConstraint.activate(contentConstraints)
    }

    func refreshPermissions(_ snapshot: PermissionStatusSnapshot) {
        guard permissionStatus != snapshot else { return }
        permissionStatus = snapshot
        build()
    }

    private func permissionRow(permission: PermissionKind, action: @escaping () -> Void) -> NSView {
        let row = NSView()
        row.wantsLayer = true
        row.layer?.cornerRadius = 14
        row.layer?.borderWidth = 1
        row.layer?.borderColor = PermissionPalette.border.cgColor
        row.layer?.backgroundColor = PermissionPalette.card.cgColor
        row.translatesAutoresizingMaskIntoConstraints = false

        let iconView = NSImageView(image: permission.icon)
        iconView.symbolConfiguration = NSImage.SymbolConfiguration(pointSize: 30, weight: .regular)
        iconView.contentTintColor = .controlAccentColor
        iconView.translatesAutoresizingMaskIntoConstraints = false

        let titleLabel = label(permission.title, size: 13, weight: .bold)
        let detailLabel = label(permission.detail, size: 11, weight: .regular)
        detailLabel.textColor = PermissionPalette.secondaryText
        let textStack = NSStackView(views: [titleLabel, detailLabel])
        textStack.orientation = .vertical
        textStack.alignment = .leading
        textStack.spacing = 4
        textStack.translatesAutoresizingMaskIntoConstraints = false

        let button = NSButton(title: "Allow", target: nil, action: nil)
        button.bezelStyle = .rounded
        button.controlSize = .regular
        button.font = NSFont.systemFont(ofSize: 13, weight: .semibold)
        button.contentTintColor = .white
        button.bezelColor = .controlAccentColor
        let buttonTitleAttributes: [NSAttributedString.Key: Any] = [
            .foregroundColor: NSColor.white,
            .font: NSFont.systemFont(ofSize: 13, weight: .semibold)
        ]
        button.attributedTitle = NSAttributedString(string: "Allow", attributes: buttonTitleAttributes)
        button.attributedAlternateTitle = NSAttributedString(string: "Allow", attributes: buttonTitleAttributes)
        let target = ButtonTarget(action)
        button.target = target
        button.action = #selector(ButtonTarget.run)
        objc_setAssociatedObject(button, "zerocode-action", target, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
        button.translatesAutoresizingMaskIntoConstraints = false

        row.addSubview(iconView)
        row.addSubview(textStack)
        row.addSubview(button)
        NSLayoutConstraint.activate([
            row.heightAnchor.constraint(equalToConstant: 62),
            iconView.leadingAnchor.constraint(equalTo: row.leadingAnchor, constant: 12),
            iconView.centerYAnchor.constraint(equalTo: row.centerYAnchor),
            iconView.widthAnchor.constraint(equalToConstant: 34),
            iconView.heightAnchor.constraint(equalToConstant: 34),
            textStack.leadingAnchor.constraint(equalTo: iconView.trailingAnchor, constant: 10),
            textStack.centerYAnchor.constraint(equalTo: row.centerYAnchor),
            button.widthAnchor.constraint(greaterThanOrEqualToConstant: 52),
            button.heightAnchor.constraint(equalToConstant: 30),
            button.trailingAnchor.constraint(equalTo: row.trailingAnchor, constant: -12),
            button.centerYAnchor.constraint(equalTo: row.centerYAnchor),
            textStack.trailingAnchor.constraint(lessThanOrEqualTo: button.leadingAnchor, constant: -12)
        ])
        return row
    }

    private func doneButton() -> NSView {
        let button = NSButton(title: "Done", target: nil, action: nil)
        button.bezelStyle = .rounded
        button.controlSize = .regular
        button.font = NSFont.systemFont(ofSize: 13, weight: .semibold)
        button.contentTintColor = .white
        button.bezelColor = .controlAccentColor
        let buttonTitleAttributes: [NSAttributedString.Key: Any] = [
            .foregroundColor: NSColor.white,
            .font: NSFont.systemFont(ofSize: 13, weight: .semibold)
        ]
        button.attributedTitle = NSAttributedString(string: "Done", attributes: buttonTitleAttributes)
        button.attributedAlternateTitle = NSAttributedString(string: "Done", attributes: buttonTitleAttributes)
        let target = ButtonTarget(close)
        button.target = target
        button.action = #selector(ButtonTarget.run)
        objc_setAssociatedObject(button, "zerocode-action", target, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
        button.translatesAutoresizingMaskIntoConstraints = false
        button.widthAnchor.constraint(greaterThanOrEqualToConstant: 82).isActive = true
        button.heightAnchor.constraint(equalToConstant: 32).isActive = true
        return button
    }

    private func label(_ text: String, size: CGFloat, weight: NSFont.Weight) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = NSFont.systemFont(ofSize: size, weight: weight)
        label.lineBreakMode = .byWordWrapping
        label.textColor = PermissionPalette.primaryText
        label.translatesAutoresizingMaskIntoConstraints = false
        return label
    }
}

private final class PermissionDragAssistantController: NSWindowController {
    private struct SettingsWindowState {
        let frame: NSRect
        let isVisible: Bool
    }

    private let fallbackVisibleFrame: NSRect?
    private let onRefreshPermissions: () -> Void
    private let onClose: () -> Void
    private var hasSeenSettingsWindow = false
    private var followTimer: Timer?
    private var isDismissed = false
    private var scheduledShowWorkItems: [DispatchWorkItem] = []
    /// Which app each window's owning pid belongs to, remembered for this
    /// assistant's life — the window walk runs three times a second and one
    /// app owns many windows. Short-lived by construction: the controller
    /// goes when the setup does, so a recycled pid never outlives it.
    private var settingsOwnerCache: [pid_t: String] = [:]

    convenience init(
        permission: PermissionKind,
        fallbackVisibleFrame: NSRect?,
        onRefreshPermissions: @escaping () -> Void,
        onClose: @escaping () -> Void
    ) {
        let window = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 390, height: 92),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        window.title = "Drag ZeroCode Computer Use"
        window.backgroundColor = .clear
        window.isOpaque = false
        window.isReleasedWhenClosed = false
        window.level = .floating
        window.hidesOnDeactivate = false
        window.isFloatingPanel = true
        window.becomesKeyOnlyIfNeeded = true
        // Why: the assistant belongs to System Settings' position; user drags
        // should either start the app-file drag or do nothing, not move the panel.
        window.isMovable = false
        window.isMovableByWindowBackground = false
        window.hasShadow = true
        self.init(
            window: window,
            fallbackVisibleFrame: fallbackVisibleFrame,
            onRefreshPermissions: onRefreshPermissions,
            onClose: onClose
        )
        let subject: PermissionSubject = permission == .screenshots ? .app : .helper
        let appURL = PermissionSubjectResolver.judgedBundleURL(subject: subject, helperURL: Bundle.main.bundleURL)
        window.contentView = PermissionDragAssistantView(permission: permission, appURL: appURL) { [weak self] in
            self?.dismissFromCloseButton()
        }
    }

    init(
        window: NSWindow?,
        fallbackVisibleFrame: NSRect?,
        onRefreshPermissions: @escaping () -> Void,
        onClose: @escaping () -> Void
    ) {
        self.fallbackVisibleFrame = fallbackVisibleFrame
        self.onRefreshPermissions = onRefreshPermissions
        self.onClose = onClose
        super.init(window: window)
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func showWhenReady() {
        guard !isDismissed else { return }
        startFollowingSettingsWindow()
        schedulePositionAndShow()
    }

    override func close() {
        isDismissed = true
        scheduledShowWorkItems.forEach { $0.cancel() }
        scheduledShowWorkItems.removeAll()
        followTimer?.invalidate()
        followTimer = nil
        super.close()
    }

    private func dismissFromCloseButton() {
        // Why: closing the NSWindow directly skips this controller's timer cleanup,
        // letting the assistant reappear while System Settings remains visible.
        complete()
    }

    func complete() {
        close()
        onClose()
    }

    private func schedulePositionAndShow() {
        scheduledShowWorkItems.forEach { $0.cancel() }
        scheduledShowWorkItems.removeAll()
        let delays = [0.12, 0.25, 0.4, 0.65, 0.95, 1.35, 1.8, 2.5]
        for (index, delay) in delays.enumerated() {
            let workItem = DispatchWorkItem { [weak self] in
                guard let self, !self.isDismissed, self.window?.isVisible != true else { return }
                if let settingsWindow = self.systemSettingsWindowState(), settingsWindow.isVisible {
                    self.positionNearSettingsWindow(settingsWindow.frame)
                    guard !self.isDismissed else { return }
                    self.showWindow(nil)
                    self.window?.orderFrontRegardless()
                } else if index == delays.count - 1 && self.systemSettingsIsFrontmost() {
                    self.positionFallback()
                    guard !self.isDismissed else { return }
                    self.showWindow(nil)
                    self.window?.orderFrontRegardless()
                }
            }
            scheduledShowWorkItems.append(workItem)
            DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: workItem)
        }
    }

    private func startFollowingSettingsWindow() {
        followTimer?.invalidate()
        followTimer = Timer.scheduledTimer(withTimeInterval: 0.35, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, !self.isDismissed else { return }
                self.syncVisibilityWithSettingsWindow()
            }
        }
    }

    private func syncVisibilityWithSettingsWindow() {
        guard !isDismissed else {
            followTimer?.invalidate()
            followTimer = nil
            return
        }
        guard let window else {
            followTimer?.invalidate()
            followTimer = nil
            return
        }
        let settingsWindow = systemSettingsWindowState()
        if settingsWindow != nil {
            hasSeenSettingsWindow = true
        } else if hasSeenSettingsWindow {
            NSApp.terminate(nil)
            return
        }
        // Why: System Settings can stay visible on one display while the user
        // works on another; follow actual occlusion instead of app focus.
        if let settingsWindow, settingsWindow.isVisible {
            onRefreshPermissions()
            guard !isDismissed else { return }
            positionNearSettingsWindow(settingsWindow.frame)
            guard !isDismissed else { return }
            if !window.isVisible {
                showWindow(nil)
            }
            guard !isDismissed else { return }
            window.orderFrontRegardless()
        } else if window.isVisible {
            window.orderOut(nil)
        }
    }

    private func systemSettingsIsFrontmost() -> Bool {
        let bundleId = NSWorkspace.shared.frontmostApplication?.bundleIdentifier
        return bundleId == "com.apple.systempreferences" || bundleId == "com.apple.SystemSettings"
    }

    private func positionNearSettingsWindow(_ settingsFrame: NSRect) {
        guard let window else { return }
        let visibleFrame = visibleFrameContaining(settingsFrame)
        let x = settingsFrame.maxX - window.frame.width - 18
        let y = settingsFrame.minY + 18
        window.setFrameOrigin(clampedOrigin(x: x, y: y, window: window, visibleFrame: visibleFrame))
    }

    private func positionFallback() {
        guard let window else { return }
        let visibleFrame = fallbackVisibleFrame ?? NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 900, height: 700)
        let origin = NSPoint(
            x: visibleFrame.midX - window.frame.width / 2,
            y: visibleFrame.minY + 24
        )
        window.setFrameOrigin(clampedOrigin(x: origin.x, y: origin.y, window: window, visibleFrame: visibleFrame))
    }

    private func clampedOrigin(x: CGFloat, y: CGFloat, window: NSWindow, visibleFrame: NSRect) -> NSPoint {
        let inset: CGFloat = 10
        let minX = visibleFrame.minX + inset
        let maxX = visibleFrame.maxX - window.frame.width - inset
        let minY = visibleFrame.minY + inset
        let maxY = visibleFrame.maxY - window.frame.height - inset
        return NSPoint(
            x: min(max(x, minX), maxX),
            y: min(max(y, minY), maxY)
        )
    }

    private func systemSettingsWindowState() -> SettingsWindowState? {
        guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
            return nil
        }
        var occludingFrames: [NSRect] = []
        for windowInfo in windows {
            guard let frame = cgWindowFrame(windowInfo), isNormalWindow(windowInfo), frame.width > 0, frame.height > 0 else {
                continue
            }
            if isSystemSettingsWindow(windowInfo), frame.width > 520, frame.height > 360 {
                return SettingsWindowState(
                    frame: appKitFrameForCGWindowFrame(frame),
                    isVisible: !isWindowFullyCovered(frame, by: occludingFrames)
                )
            }
            occludingFrames.append(frame)
        }
        return nil
    }

    private func isSystemSettingsWindow(_ windowInfo: [String: Any]) -> Bool {
        // The OWNER's bundle identifier, not the owner's name: the name in a
        // window list is localized, and "시스템 설정" never equalled "System
        // Settings" (see `SettingsWindowIdentity`). Memoized per pid because
        // this walks every window on screen three times a second and an app
        // owns many of them.
        let pid = (windowInfo[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value
        let bundleIdentifier = pid.flatMap { owner -> String? in
            if let remembered = settingsOwnerCache[owner] { return remembered }
            let found = NSRunningApplication(processIdentifier: owner)?.bundleIdentifier
            if let found { settingsOwnerCache[owner] = found }
            return found
        }
        return SettingsWindowIdentity.isSettings(
            bundleIdentifier: bundleIdentifier,
            ownerName: windowInfo[kCGWindowOwnerName as String] as? String
        )
    }

    private func isNormalWindow(_ windowInfo: [String: Any]) -> Bool {
        guard (windowInfo[kCGWindowLayer as String] as? Int) == 0 else {
            return false
        }
        if let alpha = windowInfo[kCGWindowAlpha as String] as? CGFloat, alpha <= 0 {
            return false
        }
        return true
    }

    private func cgWindowFrame(_ windowInfo: [String: Any]) -> NSRect? {
        guard
            let bounds = windowInfo[kCGWindowBounds as String] as? [String: CGFloat],
            let x = bounds["X"],
            let y = bounds["Y"],
            let width = bounds["Width"],
            let height = bounds["Height"]
        else {
            return nil
        }
        return NSRect(x: x, y: y, width: width, height: height)
    }

    private func isWindowFullyCovered(_ frame: NSRect, by occludingFrames: [NSRect]) -> Bool {
        frame.remainder(minus: occludingFrames).isEmpty
    }

    private func appKitFrameForCGWindowFrame(_ cgFrame: NSRect) -> NSRect {
        guard let screen = screenContainingCGFrame(cgFrame),
              let screenNumber = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber
        else {
            return cgFrame
        }

        let displayBounds = CGDisplayBounds(CGDirectDisplayID(screenNumber.uint32Value))
        return NSRect(
            x: cgFrame.minX,
            y: screen.frame.maxY - (cgFrame.minY - displayBounds.minY) - cgFrame.height,
            width: cgFrame.width,
            height: cgFrame.height
        )
    }

    private func screenContainingCGFrame(_ frame: NSRect) -> NSScreen? {
        NSScreen.screens.max { lhs, rhs in
            cgDisplayBounds(for: lhs).intersection(frame).width * cgDisplayBounds(for: lhs).intersection(frame).height <
                cgDisplayBounds(for: rhs).intersection(frame).width * cgDisplayBounds(for: rhs).intersection(frame).height
        }
    }

    private func cgDisplayBounds(for screen: NSScreen) -> NSRect {
        guard let screenNumber = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber else {
            return screen.frame
        }
        return CGDisplayBounds(CGDirectDisplayID(screenNumber.uint32Value))
    }

    private func visibleFrameContaining(_ frame: NSRect) -> NSRect {
        let screen = NSScreen.screens.max { lhs, rhs in
            lhs.frame.intersection(frame).width * lhs.frame.intersection(frame).height <
                rhs.frame.intersection(frame).width * rhs.frame.intersection(frame).height
        }
        return screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 900, height: 700)
    }
}

private final class PermissionDragAssistantView: NSView {
    private let permission: PermissionKind
    private let appURL: URL
    private let close: () -> Void

    init(permission: PermissionKind, appURL: URL, close: @escaping () -> Void) {
        self.permission = permission
        self.appURL = appURL
        self.close = close
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = PermissionPalette.background.cgColor
        layer?.cornerRadius = 14
        layer?.borderWidth = 1
        layer?.borderColor = PermissionPalette.border.cgColor
        layer?.masksToBounds = true
        build()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func build() {
        let closeButton = NSButton(title: "", target: nil, action: nil)
        closeButton.bezelStyle = .shadowlessSquare
        closeButton.isBordered = false
        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close")
        closeButton.imagePosition = .imageOnly
        closeButton.contentTintColor = PermissionPalette.secondaryText
        closeButton.controlSize = .small
        closeButton.translatesAutoresizingMaskIntoConstraints = false
        let target = ButtonTarget(close)
        closeButton.target = target
        closeButton.action = #selector(ButtonTarget.run)
        objc_setAssociatedObject(closeButton, "zerocode-action", target, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)

        let instruction = label(permission.dragInstruction, size: 12, weight: .semibold)
        instruction.textColor = PermissionPalette.primaryText
        instruction.maximumNumberOfLines = 2

        let dragTile = DraggableAppTile(appURL: appURL)
        dragTile.translatesAutoresizingMaskIntoConstraints = false

        addSubview(closeButton)
        addSubview(instruction)
        addSubview(dragTile)

        NSLayoutConstraint.activate([
            closeButton.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            closeButton.topAnchor.constraint(equalTo: topAnchor, constant: 10),
            closeButton.widthAnchor.constraint(equalToConstant: 18),
            closeButton.heightAnchor.constraint(equalToConstant: 18),
            instruction.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 36),
            instruction.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            instruction.centerYAnchor.constraint(equalTo: closeButton.centerYAnchor),
            dragTile.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            dragTile.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            dragTile.topAnchor.constraint(equalTo: instruction.bottomAnchor, constant: 8),
            dragTile.heightAnchor.constraint(equalToConstant: 42)
        ])
    }

    private func label(_ text: String, size: CGFloat, weight: NSFont.Weight) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = NSFont.systemFont(ofSize: size, weight: weight)
        label.lineBreakMode = .byWordWrapping
        label.textColor = PermissionPalette.primaryText
        label.translatesAutoresizingMaskIntoConstraints = false
        return label
    }
}

private final class DraggableAppTile: NSView, NSDraggingSource {
    private let appURL: URL

    override var mouseDownCanMoveWindow: Bool { false }

    init(appURL: URL) {
        self.appURL = appURL
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 12
        layer?.borderWidth = 1
        layer?.borderColor = PermissionPalette.border.cgColor
        layer?.backgroundColor = PermissionPalette.card.cgColor
        build()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func mouseDragged(with event: NSEvent) {
        let item = NSPasteboardItem()
        item.setString(appURL.absoluteString, forType: .fileURL)
        item.setString(appURL.path, forType: .string)

        let draggingItem = NSDraggingItem(pasteboardWriter: item)
        let iconSize: CGFloat = 64
        let location = convert(event.locationInWindow, from: nil)
        let dragFrame = NSRect(
            x: location.x - iconSize / 2,
            y: location.y - iconSize / 2,
            width: iconSize,
            height: iconSize
        )
        draggingItem.setDraggingFrame(dragFrame, contents: appIcon(size: iconSize))
        beginDraggingSession(with: [draggingItem], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    private func build() {
        let icon = NSImageView(image: appIcon(size: 34))
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.translatesAutoresizingMaskIntoConstraints = false

        let title = NSTextField(labelWithString: "ZeroCode Computer Use")
        title.font = NSFont.systemFont(ofSize: 15, weight: .semibold)
        title.textColor = PermissionPalette.primaryText
        title.translatesAutoresizingMaskIntoConstraints = false

        addSubview(icon)
        addSubview(title)
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            icon.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 34),
            icon.heightAnchor.constraint(equalToConstant: 34),
            title.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 14),
            title.centerYAnchor.constraint(equalTo: centerYAnchor),
            title.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -20)
        ])
    }

    private func appIcon(size: CGFloat) -> NSImage {
        let icon = NSWorkspace.shared.icon(forFile: appURL.path)
        icon.size = NSSize(width: size, height: size)
        return icon
    }
}

private enum PermissionPalette {
    static let background = adaptiveColor(
        light: NSColor(calibratedWhite: 0.94, alpha: 0.98),
        dark: NSColor(calibratedWhite: 0.20, alpha: 0.98)
    )
    static let card = adaptiveColor(
        light: NSColor(calibratedWhite: 0.99, alpha: 0.98),
        dark: NSColor(calibratedWhite: 0.25, alpha: 0.98)
    )
    static let border = adaptiveColor(
        light: NSColor(calibratedWhite: 0.0, alpha: 0.14),
        dark: NSColor(calibratedWhite: 1.0, alpha: 0.22)
    )
    static let primaryText = NSColor.labelColor
    static let secondaryText = NSColor.secondaryLabelColor

    private static func adaptiveColor(light: NSColor, dark: NSColor) -> NSColor {
        NSColor(name: nil) { appearance in
            let bestMatch = appearance.bestMatch(from: [
                .aqua,
                .darkAqua,
                .accessibilityHighContrastAqua,
                .accessibilityHighContrastDarkAqua
            ])
            return bestMatch == .darkAqua || bestMatch == .accessibilityHighContrastDarkAqua ? dark : light
        }
    }
}

private final class ButtonTarget: NSObject {
    private let actionBlock: () -> Void

    init(_ actionBlock: @escaping () -> Void) {
        self.actionBlock = actionBlock
    }

    @objc func run() {
        actionBlock()
    }
}

private final class SocketListener: @unchecked Sendable {
    private let socketPath: String
    private let token: String?
    private let onSessionClaimed: () -> Void
    private let onSessionClosed: () -> Void
    private let provider = Provider()
    private let providerLock = NSLock()
    private let sessionLock = NSLock()
    private var sessionOwnership = AgentSessionOwnership()
    private var lastConnectionID: UInt64 = 0
    private var socketFd: Int32 = -1
    private var isStopped = false

    init(
        socketPath: String,
        token: String?,
        onSessionClaimed: @escaping () -> Void,
        onSessionClosed: @escaping () -> Void
    ) throws {
        self.socketPath = socketPath
        self.token = token
        self.onSessionClaimed = onSessionClaimed
        self.onSessionClosed = onSessionClosed
        try bindSocket()
    }

    func start() {
        Thread.detachNewThread { [weak self] in
            self?.acceptLoop()
        }
    }

    func stop() {
        isStopped = true
        if socketFd >= 0 {
            close(socketFd)
            socketFd = -1
        }
        // Why: the parent owns the private temp directory cleanup; the helper
        // must not unlink arbitrary caller-supplied paths on shutdown.
    }

    private func bindSocket() throws {
        socketFd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard socketFd >= 0 else {
            throw ProviderError.coded("accessibility_error", "failed to create computer-use socket")
        }

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let maxPathLength = MemoryLayout.size(ofValue: address.sun_path)
        guard socketPath.utf8.count < maxPathLength else {
            throw ProviderError.coded("invalid_argument", "computer-use socket path is too long")
        }
        _ = withUnsafeMutablePointer(to: &address.sun_path) { pointer in
            socketPath.withCString { source in
                strncpy(UnsafeMutableRawPointer(pointer).assumingMemoryBound(to: CChar.self), source, maxPathLength)
            }
        }

        let result = withUnsafePointer(to: &address) { pointer -> Int32 in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockaddrPointer in
                bind(socketFd, sockaddrPointer, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard result == 0 else {
            let bindErrno = errno
            let message = String(cString: strerror(bindErrno))
            close(socketFd)
            socketFd = -1
            if UnixSocketPathSafety.shouldRejectExistingPathAfterBindFailure(
                bindErrno: bindErrno,
                existingMode: existingPathMode(socketPath)
            ) {
                throw ProviderError.coded("invalid_argument", "refusing to replace non-socket file at computer-use socket path")
            }
            throw ProviderError.coded("accessibility_error", "failed to bind computer-use socket: \(message)")
        }
        chmod(socketPath, 0o600)

        guard listen(socketFd, 8) == 0 else {
            let message = String(cString: strerror(errno))
            close(socketFd)
            socketFd = -1
            throw ProviderError.coded("accessibility_error", "failed to listen on computer-use socket: \(message)")
        }
    }

    private func acceptLoop() {
        while !isStopped {
            let fd = accept(socketFd, nil, nil)
            if fd < 0 {
                if !isStopped {
                    fputs("computer-use socket accept failed: \(String(cString: strerror(errno)))\n", stderr)
                }
                continue
            }
            guard let connectionID = allocateConnectionID() else {
                fputs("computer-use socket exhausted connection identities\n", stderr)
                close(fd)
                continue
            }
            Thread.detachNewThread { [weak self] in
                self?.handleConnection(fd, connectionID: connectionID)
            }
        }
    }

    private func allocateConnectionID() -> AgentSessionConnectionID? {
        sessionLock.lock()
        defer { sessionLock.unlock() }
        guard lastConnectionID < UInt64.max else { return nil }
        lastConnectionID += 1
        return AgentSessionConnectionID(rawValue: lastConnectionID)
    }

    private func handleConnection(_ fd: Int32, connectionID: AgentSessionConnectionID) {
        var registeredSession = false
        var hangupMonitor: AuthenticatedConnectionHangupMonitor?
        defer {
            hangupMonitor?.cancel()
            if registeredSession {
                disconnectSession(connectionID)
            }
            close(fd)
        }
        let authorizedPeer = peerProcessId(fd).map(isAuthorizedAgentPeer) == true
        let decoder = JSONDecoder()
        while let line = readLine(from: fd) {
            guard let data = line.data(using: .utf8),
                  let request = try? decoder.decode(Request.self, from: data)
            else {
                continue
            }
            if !registeredSession && isAuthenticatedAgentSession(
                expectedToken: token,
                requestToken: request.token,
                authorizedPeer: authorizedPeer
            ) {
                let monitor: AuthenticatedConnectionHangupMonitor
                do {
                    monitor = try AuthenticatedConnectionHangupMonitor(
                        fileDescriptor: fd,
                        onHangup: { [weak self] in
                            self?.disconnectSession(connectionID)
                        }
                    )
                } catch {
                    fputs("computer-use owner monitor failed: \(error)\n", stderr)
                    return
                }
                sessionLock.lock()
                let registration = sessionOwnership.registerConnection(
                    connectionID,
                    authenticated: true
                )
                sessionLock.unlock()
                guard registration != .rejected else {
                    monitor.cancel()
                    return
                }
                registeredSession = true
                hangupMonitor = monitor
                monitor.start()
                if registration == .claimed {
                    onSessionClaimed()
                }
            }
            let response = handleRequest(
                provider: provider,
                lock: providerLock,
                request: request,
                expectedToken: token,
                authorizedPeer: authorizedPeer
            )
            writeJSON(response, to: fd)
        }
    }

    private func disconnectSession(_ connectionID: AgentSessionConnectionID) {
        sessionLock.lock()
        let shouldTerminate = sessionOwnership.disconnect(connectionID)
        sessionLock.unlock()
        if shouldTerminate {
            onSessionClosed()
        }
    }
}

private func existingPathMode(_ path: String) -> mode_t? {
    var statInfo = stat()
    guard lstat(path, &statInfo) == 0 else {
        return nil
    }
    return statInfo.st_mode
}

private func peerProcessId(_ fd: Int32) -> pid_t? {
    var pid = pid_t(0)
    var length = socklen_t(MemoryLayout<pid_t>.size)
    let result = withUnsafeMutablePointer(to: &pid) { pointer in
        getsockopt(fd, 0, 2, pointer, &length)
    }
    return result == 0 && pid > 0 ? pid : nil
}

private func isAuthorizedAgentPeer(_ pid: pid_t) -> Bool {
    guard let command = processCommand(pid),
          command.contains("zerocode-shell") || command.contains("zerocode_shell-")
    else {
        return false
    }
    if isTrustedZeroCodeApplication(pid) {
        return true
    }
    guard let parentPid = parentProcessId(pid) else { return false }
    if isTrustedZeroCodeApplication(parentPid) {
        return true
    }
    // Development builds are not app bundles and therefore have no
    // NSRunningApplication bundle identity. The Rust owner opts into this
    // narrow fallback only in debug builds; the per-launch 0600 token still
    // authenticates the exact socket session.
    return ProcessInfo.processInfo.environment["ZEROCODE_COMPUTER_USE_ALLOW_UNBUNDLED"] == "1"
        && (command.contains("/target/debug/zerocode-shell")
            || command.contains("/target/release/zerocode-shell")
            || command.contains("/target/debug/deps/zerocode_shell-"))
}

private func isTrustedZeroCodeApplication(_ pid: pid_t) -> Bool {
    guard let app = NSRunningApplication(processIdentifier: pid),
          let bundleId = app.bundleIdentifier
    else {
        return false
    }
    // Why: dev validation runs from per-worktree wrapper apps with stable
    // ZeroCode-owned bundle ids; the sidecar peer check must still authorize them.
    return bundleId == "dev.zerocode.app" || bundleId.hasPrefix("dev.zerocode.app.dev.")
}

private func parentProcessId(_ pid: pid_t) -> pid_t? {
    guard let output = processField(pid: pid, field: "ppid=") else {
        return nil
    }
    let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let parentPid = pid_t(trimmed), parentPid > 1 else {
        return nil
    }
    return parentPid
}

private func processCommand(_ pid: pid_t) -> String? {
    return processField(pid: pid, field: "command=")
}

private func processField(pid: pid_t, field: String) -> String? {
    let process = Process()
    let pipe = Pipe()
    process.executableURL = URL(fileURLWithPath: "/bin/ps")
    process.arguments = ["-p", "\(pid)", "-o", field]
    process.standardOutput = pipe
    process.standardError = Pipe()
    do {
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { return nil }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        return String(data: data, encoding: .utf8)
    } catch {
        return nil
    }
}

@MainActor
private func runAgent(socketPath: String, token: String?) {
    let app = NSApplication.shared
    let delegate = AgentRuntime(socketPath: socketPath, token: token)
    app.delegate = delegate
    // Why: SCK is reliable once this code runs as a signed app with a real TCC identity.
    setenv("ZEROCODE_COMPUTER_USE_SCK_SCREENSHOTS", "1", 1)
    app.run()
}

@MainActor
private func runPermissionCheck(initialPermission: PermissionKind? = nil) {
    let app = NSApplication.shared
    let delegate = PermissionRuntime(initialPermission: initialPermission)
    app.delegate = delegate
    // Why: setup must foreground reliably; the long-running agent path stays accessory-only.
    app.setActivationPolicy(.regular)
    app.run()
}

private func printPermissionStatus() {
    let snapshot = permissionStatusSnapshotSettled()
    let accessibility = snapshot.accessibilityGranted ? "granted" : "not-granted"
    let screenshots = snapshot.screenshotsGranted ? "granted" : "not-granted"
    print(#"{"accessibility":"\#(accessibility)","screenshots":"\#(screenshots)"}"#)
}

private func writePermissionStatus(to path: String, requestedOS: Bool = false) {
    let snapshot = permissionStatusSnapshotSettled()
    let accessibility = snapshot.accessibilityGranted ? "granted" : "not-granted"
    let screenshots = snapshot.screenshotsGranted ? "granted" : "not-granted"
    let text = #"{"accessibility":"\#(accessibility)","screenshots":"\#(screenshots)","requested_os":\#(requestedOS)}"#
    do {
        try text.write(toFile: path, atomically: true, encoding: .utf8)
    } catch {
        fputs("failed to write permission status: \(error)\n", stderr)
        exit(1)
    }
}

private func runStdio() {
    fputs("ZeroCode Computer Use provider must be launched by ZeroCode in app-agent mode.\n", stderr)
    exit(13)
}

func handleRequest(
    provider: Provider,
    lock: NSLock,
    request: Request,
    expectedToken: String?,
    authorizedPeer: Bool
) -> Any {
    if let expectedToken, request.token != expectedToken {
        return ["id": request.id, "ok": false, "error": ["code": "permission_denied", "message": "invalid computer-use agent token"]]
    }
    if expectedToken != nil && !authorizedPeer {
        return ["id": request.id, "ok": false, "error": ["code": "permission_denied", "message": "computer-use agent peer is not authorized"]]
    }
    if request.method == "terminate" {
        DispatchQueue.main.async {
            NSApp.terminate(nil)
        }
        return ["id": request.id, "ok": true, "result": ["ok": true]]
    }
    // The operator's own roads never wait behind an action in flight
    // (realtime v1 §5.8): a stop lets go of what the hand holds on this
    // connection's thread, and a status — the operator's or a reflex run's —
    // is read beside whatever holds the provider lock.
    do {
        if let answer = try unlockedRoad(request.method, params: request.params ?? [:]) {
            return ["id": request.id, "ok": true, "result": answer]
        }
    } catch let error as ProviderError {
        return ["id": request.id, "ok": false, "error": ["code": error.code, "message": error.message]]
    } catch {
        return ["id": request.id, "ok": false, "error": ["code": "invalid_argument", "message": String(describing: error)]]
    }

    do {
        lock.lock()
        defer { lock.unlock() }
        let result = try provider.handle(method: request.method, params: request.params ?? [:])
        return ["id": request.id, "ok": true, "result": result]
    } catch let error as ProviderError {
        return ["id": request.id, "ok": false, "error": ["code": error.code, "message": error.message]]
    } catch {
        return ["id": request.id, "ok": false, "error": ["code": "accessibility_error", "message": String(describing: error)]]
    }
}

/// The requests answered outside the provider lock, or nil for every other.
/// A reflex run's road names the run it means (`run`), and the host compares
/// and answers it under its own lock (`ReflexRuntimeHost`).
func unlockedRoad(_ method: String, params: [String: JSONValue]) throws -> [String: Any]? {
    switch method {
    case "stop":
        OperatorGuardHost.stop(reason: StopReason.request)
        return OperatorGuardHost.status()
    case "status":
        return OperatorGuardHost.status()
    case "reflexStatus":
        return ReflexRuntimeHost.status(run: try reflexRunId(params, "run"))
    case "reflexStop":
        return ReflexRuntimeHost.stop(run: try reflexRunId(params, "run"), reason: StopReason.request)
    case "reflexReceipts":
        return ReflexRuntimeHost.receipts(run: try reflexRunId(params, "run"), after: try receiptNumber(params, "after"))
    case "reflexAck":
        return ReflexRuntimeHost.acknowledge(run: try reflexRunId(params, "run"), through: try receiptNumber(params, "through"))
    default:
        return nil
    }
}

/// A reflex run's id: the plan's identifier alphabet and bound, never empty.
private func reflexRunId(_ params: [String: JSONValue], _ key: String = "runId") throws -> String {
    let runId = try requiredString(params, key)
    guard ReflexContract.identifier(runId) else {
        throw ProviderError.coded("invalid_argument", "\(key) must be 1 to \(ReflexContract.maxIdentifierBytes) of A-Z a-z 0-9 - _")
    }
    return runId
}

/// A receipt's number, as the window's collector names it: a whole number from 0.
private func receiptNumber(_ params: [String: JSONValue], _ key: String) throws -> UInt64 {
    let number = try requiredInteger(params, key)
    guard number >= 0 else { throw ProviderError.coded("invalid_argument", "\(key) is a receipt number from 0") }
    return UInt64(number)
}

private func readLine(from fd: Int32) -> String? {
    var bytes: [UInt8] = []
    var byte: UInt8 = 0
    while true {
        let count = read(fd, &byte, 1)
        if count == 0 {
            return bytes.isEmpty ? nil : String(bytes: bytes, encoding: .utf8)
        }
        if count < 0 {
            return nil
        }
        if byte == 10 {
            return String(bytes: bytes, encoding: .utf8)
        }
        bytes.append(byte)
    }
}

private func writeJSON(_ object: Any, to fd: Int32?) {
    guard JSONSerialization.isValidJSONObject(object),
          let data = try? JSONSerialization.data(withJSONObject: object, options: [.withoutEscapingSlashes])
    else {
        return
    }
    if let fd {
        _ = writeAll(data, to: fd)
        _ = writeAll(Data([10]), to: fd)
    } else {
        guard let text = String(data: data, encoding: .utf8) else {
            return
        }
        print(text)
        fflush(stdout)
    }
}

private func writeAll(_ data: Data, to fd: Int32) -> Bool {
    data.withUnsafeBytes { rawBuffer in
        guard let baseAddress = rawBuffer.baseAddress else {
            return true
        }
        var offset = 0
        while offset < rawBuffer.count {
            let written = write(fd, baseAddress.advanced(by: offset), rawBuffer.count - offset)
            if written < 0 {
                if errno == EINTR {
                    continue
                }
                return false
            }
            if written == 0 {
                return false
            }
            offset += written
        }
        return true
    }
}

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments.first == "--agent" {
    guard arguments.count >= 2 else {
        fputs("usage: zerocode-computer-use-macos --agent <socket-path> --token-file <token-path>\n", stderr)
        exit(2)
    }
    let tokenFileIndex = arguments.firstIndex(of: "--token-file")
    let token = tokenFileIndex.flatMap { index -> String? in
        let valueIndex = index + 1
        guard valueIndex < arguments.count else { return nil }
        let tokenPath = arguments[valueIndex]
        return try? String(contentsOfFile: tokenPath, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
    guard let token, !token.isEmpty else {
        fputs("zerocode-computer-use-macos --agent requires a non-empty --token-file\n", stderr)
        exit(2)
    }
    runAgent(socketPath: arguments[1], token: token)
} else if arguments.first == "--permissions" {
    runPermissionCheck()
} else if arguments.first == "--permission" {
    runPermissionCheck(initialPermission: PermissionKind.parse(arguments.dropFirst().first))
} else if arguments.first == "--permission-status" {
    printPermissionStatus()
 } else if arguments.first == "--permission-request-file" {
    guard arguments.count == 3, let permission = PermissionKind.parse(arguments[2]) else { exit(2) }
    permission.requestAccess()
    writePermissionStatus(to: arguments[1], requestedOS: true)
} else if arguments.first == "--permission-status-file" {
    guard arguments.count >= 2 else {
        fputs("usage: zerocode-computer-use-macos --permission-status-file <path>\n", stderr)
        exit(2)
    }
    writePermissionStatus(to: arguments[1])
} else {
    runStdio()
}

// MARK: - The desktop (docs/design/computer-use-full-operator.md §2.1)

/// A point on the desktop, in Quartz global coordinates (origin at the main
/// display's top-left, y downward) — the space CGEvent posts in and the space
/// a desktop screenshot's `origin` reports.
private func desktopPoint(_ params: [String: JSONValue], _ xKey: String, _ yKey: String) throws -> CGPoint {
    CGPoint(x: try requiredNumber(params, xKey), y: try requiredNumber(params, yKey))
}

enum DesktopScreen {
    struct Display {
        let id: CGDirectDisplayID
        let bounds: CGRect
        let scale: CGFloat
        let main: Bool
    }

    /// Every active display, the main one first — the order `--display N` names.
    static func displays() -> [Display] {
        var count: UInt32 = 0
        var ids = [CGDirectDisplayID](repeating: 0, count: 16)
        CGGetActiveDisplayList(UInt32(ids.count), &ids, &count)
        let found = (0..<Int(count)).map { at -> Display in
            let id = ids[at]
            let screen = NSScreen.screens.first { screen in
                (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id
            }
            return Display(id: id, bounds: CGDisplayBounds(id), scale: screen?.backingScaleFactor ?? 1, main: CGDisplayIsMain(id) != 0)
        }
        return found.sorted { lhs, rhs in lhs.main && !rhs.main }
    }

    /// Where display `id` sits and how fine it is now — its bounds, its mode's
    /// pixels a point and its turn — from CoreGraphics alone, so the eye's
    /// stream queue and a run's evaluating thread can ask it; nil once the
    /// display is gone.
    static func geometry(of id: CGDirectDisplayID) -> EyeGeometry? {
        guard CGDisplayIsActive(id) != 0, let mode = CGDisplayCopyDisplayMode(id), mode.width > 0 else { return nil }
        let bounds = CGDisplayBounds(id)
        let orientation: ReflexOrientation
        switch Int(CGDisplayRotation(id).rounded()) {
        case 90: orientation = .right
        case 180: orientation = .down
        case 270: orientation = .left
        default: orientation = .up
        }
        return EyeGeometry(
            displayId: String(id),
            originX: bounds.minX,
            originY: bounds.minY,
            width: bounds.width,
            height: bounds.height,
            scale: Double(mode.pixelWidth) / Double(mode.width),
            orientation: orientation
        )
    }

    static func render(_ display: Display, index: Int) -> [String: Any] {
        [
            "index": index,
            "id": display.id,
            "main": display.main,
            "scale": display.scale,
            "bounds": ["x": display.bounds.minX, "y": display.bounds.minY, "width": display.bounds.width, "height": display.bounds.height],
        ]
    }

    /// The whole desktop, one display, or one region of it, as a screenshot
    /// payload of the same shape an app observation carries (`data`, `width`,
    /// `height`, `scale`) plus where on the desktop its top-left sits — so a
    /// coordinate read off the picture goes back to the desktop by
    /// `origin + pixel / scale`. A zoom asks for `fullRes`: no bounding.
    /// The pixels of a display, or of a region of it, with where they sit on
    /// the screen — what a screenshot encodes and what OCR reads.
    struct Frame {
        let image: CGImage
        let origin: CGPoint
        let pointsWidth: CGFloat
        let pointsHeight: CGFloat
        let display: Display
        let displayIndex: Int
    }

    static func capture(params: [String: JSONValue]) throws -> [String: Any] {
        let frame = try frame(params: params)
        let png: BoundedPNG
        if params["fullRes"]?.bool == true {
            guard let encoded = encodePng(frame.image) else {
                throw ProviderError.coded("accessibility_error", "encoding the screenshot failed")
            }
            png = encoded
        } else {
            guard let bounded = boundedPngData(frame.image) else {
                throw ProviderError.coded("accessibility_error", "encoding the screenshot failed")
            }
            png = bounded
        }
        return [
            "screenshot": [
                "data": png.data.base64EncodedString(),
                "width": png.width,
                "height": png.height,
                "scale": desktopScreenshotScale(pixelWidth: png.width, pointsWidth: frame.pointsWidth),
            ],
            "origin": ["x": frame.origin.x, "y": frame.origin.y],
            "display": render(frame.display, index: frame.displayIndex),
        ]
    }

    static func frame(params: [String: JSONValue]) throws -> Frame {
        guard screenCaptureTrusted() else {
            throw ProviderError.coded(
                "permission_denied",
                "screen recording is not granted to the Computer Use helper (zerocode-computer permissions --id screenshots)"
            )
        }
        let all = displays()
        guard !all.isEmpty else { throw ProviderError.coded("accessibility_error", "no active display") }
        let region: CGRect? = try params["region"].flatMap { value in
            guard case let .object(fields) = value,
                  let x = fields["x"]?.number, let y = fields["y"]?.number,
                  let width = fields["width"]?.number, let height = fields["height"]?.number
            else {
                throw ProviderError.coded("invalid_argument", "region must carry x, y, width and height")
            }
            return CGRect(x: x, y: y, width: width, height: height)
        }
        let chosen: Display
        if let index = try optionalInteger(params, "display") {
            guard index >= 0, index < all.count else {
                throw ProviderError.coded("invalid_argument", "display \(index) is not one of the \(all.count) active displays")
            }
            chosen = all[index]
        } else if let region,
                  let showing = all.first(where: { $0.bounds.contains(CGPoint(x: region.midX, y: region.midY)) })
                      ?? all.first(where: { $0.bounds.intersects(region) }) {
            // The display that shows the region's centre — a box about a
            // point near an edge belongs to the point's display, not to the
            // first one it grazes.
            chosen = showing
        } else {
            chosen = all[0]
        }
        guard let image = CGDisplayCreateImage(chosen.id) else {
            throw ProviderError.coded("accessibility_error", "capturing display \(chosen.id) failed")
        }
        var picture = image
        var origin = chosen.bounds.origin
        var pointsWidth = chosen.bounds.width
        var pointsHeight = chosen.bounds.height
        if let region {
            guard let pixels = desktopRegionInPixels(region: region, displayBounds: chosen.bounds, pixelWidth: image.width),
                  let cropped = image.cropping(to: pixels)
            else {
                throw ProviderError.coded("invalid_argument", "region does not touch display \(chosen.id)")
            }
            picture = cropped
            let visible = region.intersection(chosen.bounds)
            origin = visible.origin
            pointsWidth = visible.width
            pointsHeight = visible.height
        }
        return Frame(
            image: picture,
            origin: origin,
            pointsWidth: pointsWidth,
            pointsHeight: pointsHeight,
            display: chosen,
            displayIndex: all.firstIndex { $0.id == chosen.id } ?? 0
        )
    }
}

// MARK: - The meaning layer: find, read (docs/design/computer-use-full-operator.md §2.3)

extension Provider {
    /// The most matches a `find` answers with — a person scans a page, not a database.
    static let findMatchLimit = 200

    /// An accessibility element's readable face: role, label, value, description.
    private func elementFace(_ element: AXUIElement) -> (role: String?, label: String?, value: String?, description: String?) {
        (
            stringAttribute(element, kAXRoleAttribute as String),
            stringAttribute(element, kAXTitleAttribute as String),
            stringAttribute(element, kAXValueAttribute as String),
            stringAttribute(element, kAXDescriptionAttribute as String)
        )
    }

    /// Pixels to read: the named app's window, or the display (a region of it).
    private func pixelsToRead(params: [String: JSONValue]) throws -> (image: CGImage, origin: CGPoint, width: CGFloat, height: CGFloat, source: [String: Any]) {
        if params["app"] != nil {
            var quiet = params
            quiet["noScreenshot"] = .bool(true)
            let snapshot = try observe(params: quiet)
            guard screenCaptureTrusted() else {
                throw ProviderError.coded("permission_denied", "screen recording is not granted to the Computer Use helper (zerocode-computer permissions --id screenshots)")
            }
            guard let captured = WindowCapture.captureImage(windowId: snapshot.windowId, bounds: snapshot.windowBounds) else {
                throw ProviderError.coded("accessibility_error", "capturing window \(snapshot.windowId) of '\(snapshot.app.name)' failed")
            }
            return (
                captured.image, snapshot.windowBounds.origin, snapshot.windowBounds.width, snapshot.windowBounds.height,
                ["window": ["id": Int(snapshot.windowId), "title": snapshot.windowTitle], "app": snapshot.app.name]
            )
        }
        let frame = try DesktopScreen.frame(params: params)
        return (frame.image, frame.origin, frame.pointsWidth, frame.pointsHeight, ["display": DesktopScreen.render(frame.display, index: frame.displayIndex)])
    }

    /// The readable lines and their source, including excluded regions.
    private struct Reading {
        let lines: [RecognizedLine]
        let source: [String: Any]
    }

    /// The text an OCR read answers: an app's window read whole; the
    /// desktop read through `DesktopReading`, which keeps what did not
    /// repaint since its last reading and leaves out what ZeroCode's own
    /// windows show (`ownRegion`, sent by the window).
    private func linesToRead(params: [String: JSONValue]) throws -> Reading {
        if params["app"] != nil {
            let pixels = try pixelsToRead(params: params)
            let lines = try recognizeText(in: pixels.image, origin: pixels.origin, pointsWidth: pixels.width, pointsHeight: pixels.height)
            return Reading(lines: lines, source: pixels.source)
        }
        // Where every open eye stands before the pixels are taken: a repaint
        // from here on is one this reading may have missed.
        let standing = ScreenEye.standing()
        let frame = try DesktopScreen.frame(params: params)
        let leftOut = LeftOut(rects: Self.rectangles(params["ownRegion"]))
        let read = try DesktopReading.lines(frame: frame, eye: standing[frame.display.id], leftOut: leftOut)
        var source: [String: Any] = ["display": DesktopScreen.render(frame.display, index: frame.displayIndex)]
        let area = CGRect(x: frame.origin.x, y: frame.origin.y, width: frame.pointsWidth, height: frame.pointsHeight)
        source["excludedRegions"] = leftOut.rects.map { $0.intersection(area) }
            .filter { !$0.isNull && $0.width > 0 && $0.height > 0 }
            .map { ["x": $0.minX, "y": $0.minY, "width": $0.width, "height": $0.height] }
        return Reading(lines: read.filter { !leftOut.shows($0.frame) }, source: source)
    }

    /// `[x, y, width, height]` rectangles, as the eye's answers carry them.
    private static func rectangles(_ value: JSONValue?) -> [CGRect] {
        guard case let .array(rows)? = value else { return [] }
        return rows.compactMap { row in
            guard case let .array(edges) = row, edges.count == 4 else { return nil }
            let numbers = edges.compactMap(\.number)
            guard numbers.count == 4 else { return nil }
            return CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3])
        }
    }

    private func renderLine(_ line: RecognizedLine) -> [String: Any] {
        [
            "text": line.text,
            "confidence": line.confidence,
            "x": line.frame.minX, "y": line.frame.minY, "width": line.frame.width, "height": line.frame.height,
            "centerX": line.frame.midX, "centerY": line.frame.midY,
        ]
    }

    func findElements(params: [String: JSONValue]) throws -> [String: Any] {
        let query = ElementQuery(text: params["text"]?.string, role: params["role"]?.string, label: params["label"]?.string)
        guard !query.isEmpty else {
            throw ProviderError.coded("invalid_argument", "find needs --text, --role or --label")
        }
        if params["ocr"]?.bool == true {
            guard let text = query.text else {
                throw ProviderError.coded("invalid_argument", "an --ocr find looks for --text; roles and labels live in the accessibility tree")
            }
            let reading = try linesToRead(params: params)
            let matches = reading.lines.filter { $0.text.localizedCaseInsensitiveContains(text) }
            var result: [String: Any] = [
                "source": "ocr",
                "coordinateSpace": "screen",
                "matches": Array(matches.prefix(Self.findMatchLimit)).map(renderLine),
                "linesRead": reading.lines.count,
                "truncated": matches.count > Self.findMatchLimit,
            ]
            result.merge(reading.source) { current, _ in current }
            return result
        }
        var quiet = params
        quiet["noScreenshot"] = .bool(true)
        let snapshot = try observe(params: quiet)
        var matches: [[String: Any]] = []
        var seen = 0
        for index in snapshot.elements.keys.sorted() {
            guard let record = snapshot.elements[index] else { continue }
            let face = elementFace(record.element)
            guard query.matches(role: face.role, label: face.label, value: face.value, description: face.description) else { continue }
            seen += 1
            guard matches.count < Self.findMatchLimit else { continue }
            let frame = absoluteFrame(record.element)
            matches.append([
                "index": index,
                "role": jsonNullable(face.role),
                "label": jsonNullable(face.label),
                "value": jsonNullable(face.value),
                "description": jsonNullable(face.description),
                "actions": record.actions,
                "frame": frame.map { ["x": $0.minX, "y": $0.minY, "width": $0.width, "height": $0.height, "centerX": $0.midX, "centerY": $0.midY] } ?? NSNull(),
            ])
        }
        return [
            "source": "accessibility",
            "coordinateSpace": "screen",
            "snapshotId": snapshot.id,
            "app": snapshot.app.name,
            "window": ["id": Int(snapshot.windowId), "title": snapshot.windowTitle],
            "matches": matches,
            "truncated": seen > Self.findMatchLimit,
            "elementCount": snapshot.elements.count,
        ]
    }

    func readText(params: [String: JSONValue]) throws -> [String: Any] {
        if params["ocr"]?.bool == true {
            let reading = try linesToRead(params: params)
            var result: [String: Any] = [
                "source": "ocr",
                "text": reading.lines.map(\.text).joined(separator: "\n"),
                "lines": reading.lines.map(renderLine),
            ]
            result.merge(reading.source) { current, _ in current }
            return result
        }
        var quiet = params
        quiet["noScreenshot"] = .bool(true)
        let snapshot = try observe(params: quiet)
        return [
            "source": "accessibility",
            "snapshotId": snapshot.id,
            "app": snapshot.app.name,
            "window": ["id": Int(snapshot.windowId), "title": snapshot.windowTitle],
            "text": snapshot.treeText,
            "elementCount": snapshot.elements.count,
            "treeTruncated": snapshot.truncated,
        ]
    }
}

extension Input {
    /// A drag's interpolation, when the caller names none: enough steps that a
    /// drop target sees motion, few enough to finish inside a second.
    static let desktopDragSteps = 24
    /// The pause between two drag steps, so a target that reads motion sees it.
    private static let desktopDragStepMicros: UInt32 = 8_000

    static func desktopCursorPosition() -> CGPoint {
        CGEvent(source: nil)?.location ?? .zero
    }

    /// The pause between two stepped waypoints (`mouseMove --steps`, a drag).
    private static var desktopStepNs: UInt64 { UInt64(desktopDragStepMicros) * 1_000 }

    static func desktopMove(to point: CGPoint) throws {
        try desktopPost(.pointerMove, at: point)
    }

    /// A human-paced move: eased intermediate points from where the pointer is,
    /// each due one drag step after the one before, no button held. The path
    /// is pure Core; the hand posts the newest point due and a stop ends it.
    static func desktopMove(to point: CGPoint, steps: Int) throws {
        let start = desktopCursorPosition()
        let path = PointerSchedule.steps(
            from: .init(x: start.x, y: start.y), to: .init(x: point.x, y: point.y),
            count: steps, eased: true, stepNs: desktopStepNs, startNs: OperatorHandHost.hand.nowNs())
        try OperatorHandHost.walk(path) { HandEvent(.pointerMove, x: $0.x, y: $0.y) }
    }

    /// A click at a desktop point: move there, then press and release `count`
    /// times with the click state a double or triple click carries, holding
    /// the modifiers throughout. No app is named, so no window is verified —
    /// the screenshot that follows is the verification.
    static func desktopClick(at point: CGPoint, button: MouseButtonSelection, count: Int, modifiers: [KeyModifier]) throws {
        let flags = modifiers.reduce(into: CGEventFlags()) { result, modifier in result.insert(modifier.flag) }
        try desktopPost(.pointerMove, at: point, flags: flags)
        for state in 1...max(1, count) {
            try desktopPost(.buttonDown(button, clickState: Int64(state)), at: point, flags: flags)
            try desktopPost(.buttonUp(button, clickState: Int64(state)), at: point, flags: flags)
        }
    }

    static func desktopDrag(from start: CGPoint, to end: CGPoint, steps: Int) throws {
        try desktopPost(.pointerMove, at: start)
        try desktopPost(.buttonDown(.left, clickState: 1), at: start)
        let path = PointerSchedule.steps(
            from: .init(x: start.x, y: start.y), to: .init(x: end.x, y: end.y),
            count: steps, eased: false, stepNs: desktopStepNs, startNs: OperatorHandHost.hand.nowNs())
        do {
            try OperatorHandHost.walk(path) { HandEvent(.buttonDrag(.left, clickState: 1), x: $0.x, y: $0.y) }
            // The pointer rests one step on the drop target before the release.
            try OperatorHandHost.sleep(nanoseconds: desktopStepNs)
        } catch {
            // A stop mid-drag has already let go (`OperatorHand.stop`); any
            // other failure lets go where the pointer is, then reports.
            try? desktopPost(.buttonUp(.left, clickState: 1), at: desktopCursorPosition())
            throw error
        }
        try desktopPost(.buttonUp(.left, clickState: 1), at: end)
    }

    /// Lines of scroll at a desktop point: positive `dy` scrolls the content
    /// up (the wheel toward the person), the sign CGEvent's wheel uses.
    static func desktopScroll(at point: CGPoint, dx: Int32, dy: Int32) throws {
        try desktopPost(.pointerMove, at: point)
        try OperatorHandHost.post(HandEvent(.scroll(wheel1: dy, wheel2: dx), x: point.x, y: point.y))
    }

    /// Hold a chord for `milliseconds` on the hand's clock: a stop lets go of
    /// every key it holds at once, on the stopping thread, and ends the hold.
    static func desktopHoldKey(_ key: String, milliseconds: Int) throws {
        let parsed = try KeyMap.parse(key)
        var flags = CGEventFlags()
        for modifier in parsed.modifiers {
            flags.insert(modifier.flag)
            try keyEvent(modifier.keyCode, down: true, flags: flags, pid: 0, holding: modifier.flag)
        }
        try keyEvent(parsed.keyCode, down: true, flags: flags, pid: 0)
        try OperatorHandHost.sleep(nanoseconds: UInt64(max(0, milliseconds)) * 1_000_000)
        try keyEvent(parsed.keyCode, down: false, flags: flags, pid: 0)
        for modifier in parsed.modifiers.reversed() {
            flags.remove(modifier.flag)
            try keyEvent(modifier.keyCode, down: false, flags: flags, pid: 0)
        }
    }

    /// A mouse event on the desktop, made with no source as the desktop verbs
    /// always made it. A release always goes: a stop never leaves a button down.
    private static func desktopPost(_ kind: HandEvent.Kind, at point: CGPoint, flags: CGEventFlags = []) throws {
        try OperatorHandHost.post(HandEvent(kind, x: point.x, y: point.y, flags: flags.rawValue))
    }
}

/// The hand's platform half: makes each event as the verbs made it before the
/// hand — the same source, flags and click state — stamps it with the hand's
/// tag (`eventSourceUserData`) so the input monitor knows it for the hand's
/// own, and posts it to the HID tap or to one process.
struct CGEventHandPoster: HandPoster {
    func post(_ event: HandEvent, tag: Int64) throws {
        let made = try make(event)
        made.setIntegerValueField(.eventSourceUserData, value: tag)
        switch event.route {
        case .desktop:
            made.post(tap: .cghidEventTap)
        case let .process(pid):
            made.postToPid(pid)
        }
    }

    func pointerLocation() -> SmoothPointerPath.Point? {
        guard let location = CGEvent(source: nil)?.location else { return nil }
        return SmoothPointerPath.Point(x: location.x, y: location.y)
    }

    private func make(_ event: HandEvent) throws -> CGEvent {
        var source: CGEventSource?
        if event.source == .session {
            guard let session = CGEventSource(stateID: .combinedSessionState) else {
                throw ProviderError.coded("accessibility_error", "failed to create event source")
            }
            source = session
        }
        let point = CGPoint(x: event.x, y: event.y)
        let flags = CGEventFlags(rawValue: event.flags)
        switch event.kind {
        case .pointerMove:
            return try mouse(.mouseMoved, source, point, .left, flags, clickState: 0)
        case let .buttonDown(button, clickState):
            return try mouse(button.downEvent, source, point, button.cgButton, flags, clickState: clickState)
        case let .buttonUp(button, clickState):
            return try mouse(button.upEvent, source, point, button.cgButton, flags, clickState: clickState)
        case let .buttonDrag(button, clickState):
            return try mouse(button.dragEvent, source, point, button.cgButton, flags, clickState: clickState)
        case let .scroll(wheel1, wheel2):
            guard let made = CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 2, wheel1: wheel1, wheel2: wheel2, wheel3: 0) else {
                throw ProviderError.coded("accessibility_error", "failed to create scroll event")
            }
            made.location = point
            return made
        case let .key(code, down, _):
            guard let made = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else {
                throw ProviderError.coded("accessibility_error", "failed to create key event")
            }
            made.flags = flags
            return made
        case let .text(unit, down):
            guard let made = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else {
                throw ProviderError.coded("accessibility_error", "failed to create keyboard event")
            }
            var char = unit
            made.keyboardSetUnicodeString(stringLength: 1, unicodeString: &char)
            return made
        }
    }

    private func mouse(_ type: CGEventType, _ source: CGEventSource?, _ point: CGPoint, _ button: CGMouseButton, _ flags: CGEventFlags, clickState: Int64) throws -> CGEvent {
        guard let made = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button) else {
            throw ProviderError.coded("accessibility_error", "failed to create mouse event")
        }
        made.flags = flags
        if clickState > 0 {
            made.setIntegerValueField(.mouseEventClickState, value: clickState)
        }
        return made
    }
}

// MARK: - Apps, windows and the system (docs/design/computer-use-full-operator.md §2.2)

private enum DesktopApps {
    /// Where a name is looked for, in order — the same places a person opens.
    static let applicationRoots: [URL] = [
        URL(fileURLWithPath: "/Applications"),
        URL(fileURLWithPath: "/System/Applications"),
        URL(fileURLWithPath: "/System/Applications/Utilities"),
        URL(fileURLWithPath: ("~/Applications" as NSString).expandingTildeInPath),
    ]
    /// How long a launch waits for the process, and for its first window.
    static let launchProcessTimeoutSeconds: TimeInterval = 10
    static let defaultWaitReadyMs = 5_000
    static let readyPollMs: UInt32 = 100
    /// How long a quit or an activation is given to be seen.
    static let settleTimeoutSeconds: TimeInterval = 5

    static func resolveApplicationURL(_ query: String, running: [AppDescriptor]) throws -> URL {
        if let live = running.first(where: { matches($0, query: query) }),
           let url = live.app.bundleURL {
            return url
        }
        if looksLikeBundleIdentifier(query), let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: query) {
            return url
        }
        for candidate in applicationCandidateURLs(query: query, roots: applicationRoots)
        where FileManager.default.fileExists(atPath: candidate.path) {
            return candidate
        }
        throw ProviderError.coded("app_not_found", "no application named '\(query)' under the application folders, and none running by that name")
    }

    static func launch(params: [String: JSONValue], running: [AppDescriptor]) throws -> [String: Any] {
        let query = try requiredString(params, "app")
        let url = try resolveApplicationURL(query, running: running)
        let args = (params["args"]?.string ?? "").split(whereSeparator: { $0 == " " }).map(String.init)
        let launched = try BlockingAsync.run(timeout: launchProcessTimeoutSeconds) {
            // The configuration is built inside the closure: it is not Sendable.
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = true
            configuration.arguments = args
            return try await NSWorkspace.shared.openApplication(at: url, configuration: configuration)
        }
        let pid = launched.processIdentifier
        let waitReady = try optionalInteger(params, "waitReadyMs") ?? defaultWaitReadyMs
        let deadline = Date().addingTimeInterval(TimeInterval(waitReady) / 1_000)
        var windows = WindowCapture.candidates(pid: pid).filter { $0.layer == 0 }.count
        while windows == 0, Date() < deadline {
            usleep(readyPollMs * 1_000)
            windows = WindowCapture.candidates(pid: pid).filter { $0.layer == 0 }.count
        }
        return [
            "name": launched.localizedName ?? query,
            // The bundle's own unlocalized name: the word the next verb can
            // ask by when `name` came back in the person's language.
            "bundleName": jsonNullable(bundleName(info: Bundle(url: url)?.infoDictionary)),
            "bundleId": jsonNullable(launched.bundleIdentifier),
            "pid": Int(pid),
            "path": url.path,
            "ready": windows > 0,
            "windows": windows,
        ]
    }

    static func quit(app: AppDescriptor, force: Bool) throws -> [String: Any] {
        let asked = force ? app.app.forceTerminate() : app.app.terminate()
        let deadline = Date().addingTimeInterval(settleTimeoutSeconds)
        while !app.app.isTerminated, Date() < deadline {
            usleep(readyPollMs * 1_000)
        }
        return ["name": app.name, "pid": Int(app.pid), "asked": asked, "terminated": app.app.isTerminated, "force": force]
    }

    static func activate(app: AppDescriptor) throws -> [String: Any] {
        _ = app.app.activate(options: [.activateAllWindows])
        let appElement = AXUIElementCreateApplication(app.pid)
        if let windows = copyArray(appElement, kAXWindowsAttribute as String), let first = windows.first {
            _ = AXUIElementPerformAction(first, kAXRaiseAction as CFString)
        }
        let deadline = Date().addingTimeInterval(settleTimeoutSeconds)
        while !app.app.isActive, Date() < deadline {
            usleep(readyPollMs * 1_000)
        }
        return ["name": app.name, "pid": Int(app.pid), "active": app.app.isActive]
    }

    static func open(params: [String: JSONValue], running: [AppDescriptor]) throws -> [String: Any] {
        let target: URL
        if let raw = params["url"]?.string {
            guard let url = URL(string: raw), url.scheme != nil else {
                throw ProviderError.coded("invalid_argument", "'\(raw)' is not an address with a scheme")
            }
            target = url
        } else {
            let path = ((try requiredString(params, "path")) as NSString).expandingTildeInPath
            guard FileManager.default.fileExists(atPath: path) else {
                throw ProviderError.coded("invalid_argument", "no file at '\(path)'")
            }
            target = URL(fileURLWithPath: path)
        }
        let opened: Bool
        if let with = params["with"]?.string {
            let application = try resolveApplicationURL(with, running: running)
            opened = (try? BlockingAsync.run(timeout: launchProcessTimeoutSeconds) {
                let configuration = NSWorkspace.OpenConfiguration()
                configuration.activates = true
                return try await NSWorkspace.shared.open([target], withApplicationAt: application, configuration: configuration)
            }) != nil
        } else {
            opened = NSWorkspace.shared.open(target)
        }
        guard opened else {
            throw ProviderError.coded("accessibility_error", "the system refused to open '\(target.absoluteString)'")
        }
        return ["opened": true, "target": target.absoluteString]
    }
}

private enum DesktopWindows {
    /// Every window a person could reach, front to back: on screen, at the
    /// normal layer, big enough to be a window rather than a widget.
    static let smallestWindowSide: CGFloat = 48

    /// `everyLayer`: every on-screen window at every layer and size — menus,
    /// panels, the Dock — with its layer and alpha, front to back: what covers
    /// a window for a marked desktop look.
    static func listAll(everyLayer: Bool = false) -> [[String: Any]] {
        guard let infos = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else {
            return []
        }
        let displays = DesktopScreen.displays().map(\.bounds)
        return infos.flatMap { info -> [[String: Any]] in
            guard let layer = info[kCGWindowLayer as String] as? Int, everyLayer || layer == 0,
                  let number = info[kCGWindowNumber as String] as? NSNumber,
                  let ownerPid = info[kCGWindowOwnerPID as String] as? pid_t,
                  let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDictionary),
                  everyLayer || (bounds.width >= smallestWindowSide && bounds.height >= smallestWindowSide)
            else { return [] }
            var row: [String: Any] = [
                "id": Int(number.uint32Value),
                "title": (info[kCGWindowName as String] as? String) ?? "",
                "app": ["name": (info[kCGWindowOwnerName as String] as? String) ?? "", "pid": Int(ownerPid)],
                "x": Int(bounds.minX.rounded()), "y": Int(bounds.minY.rounded()),
                "width": Int(bounds.width.rounded()), "height": Int(bounds.height.rounded()),
                // ZeroCode's own (every build, and this helper): a marked
                // desktop look never marks it, and whatever it covers is hidden.
                "own": ownerPid == getpid() || isTrustedZeroCodeApplication(ownerPid),
            ]
            var covers: [[String: Any]] = []
            if everyLayer {
                row["layer"] = layer
                row["alpha"] = (info[kCGWindowAlpha as String] as? Double) ?? 1
                row["overlay"] = DesktopOverlay.isOverlay(layer: layer, bounds: bounds, displays: displays)
                if row["overlay"] as? Bool == true {
                    let owner = AXUIElementCreateApplication(ownerPid)
                    let frames = (copyArray(owner, kAXChildrenAttribute as String) ?? []).compactMap(absoluteFrame)
                    for frame in DesktopOverlay.coveringFrames(frames, inside: bounds) {
                        var cover = row
                        cover["overlay"] = false
                        cover["x"] = frame.minX
                        cover["y"] = frame.minY
                        cover["width"] = frame.width
                        cover["height"] = frame.height
                        covers.append(cover)
                    }
                }
            }
            return covers + [row]
        }
    }

    /// The accessibility element of a window the system numbers, found through
    /// its owner's element tree — the one road that can move, size and close it.
    /// The process a window on this desktop belongs to.
    static func ownerPid(windowId: CGWindowID) -> pid_t? {
        guard let infos = CGWindowListCopyWindowInfo([.optionIncludingWindow], windowId) as? [[String: Any]] else { return nil }
        return infos.first?[kCGWindowOwnerPID as String] as? pid_t
    }

    static func element(windowId: CGWindowID) throws -> (AXUIElement, pid_t) {
        guard let ownerPid = ownerPid(windowId: windowId) else {
            throw ProviderError.coded("window_not_found", "no window \(windowId) on this desktop")
        }
        let appElement = AXUIElementCreateApplication(ownerPid)
        guard let windows = copyArray(appElement, kAXWindowsAttribute as String),
              let element = windows.first(where: { windowNumber($0) == windowId })
        else {
            throw ProviderError.coded("window_not_found", "window \(windowId) has no accessibility element (is Accessibility granted, and does the app publish its windows?)")
        }
        return (element, ownerPid)
    }

    static func act(params: [String: JSONValue]) throws -> [String: Any] {
        let raw = try requiredString(params, "action")
        guard let action = DesktopWindowAction(rawValue: raw) else {
            throw ProviderError.coded("invalid_argument", "unknown window action '\(raw)'")
        }
        let windowId = CGWindowID(try requiredInteger(params, "windowId"))
        let (element, pid) = try element(windowId: windowId)
        switch action {
        case .focus:
            NSRunningApplication(processIdentifier: pid)?.activate(options: [])
            try perform(element, kAXRaiseAction as String)
        case .move:
            var point = CGPoint(x: try requiredNumber(params, "x"), y: try requiredNumber(params, "y"))
            guard let value = AXValueCreate(.cgPoint, &point) else { throw ProviderError.coded("accessibility_error", "no point value") }
            try set(element, kAXPositionAttribute as String, value)
        case .resize:
            var size = CGSize(width: try requiredNumber(params, "width"), height: try requiredNumber(params, "height"))
            guard let value = AXValueCreate(.cgSize, &size) else { throw ProviderError.coded("accessibility_error", "no size value") }
            try set(element, kAXSizeAttribute as String, value)
        case .minimize:
            try set(element, kAXMinimizedAttribute as String, kCFBooleanTrue)
        case .zoom:
            try press(element, kAXZoomButtonAttribute as String)
        case .close:
            try press(element, kAXCloseButtonAttribute as String)
        }
        let frame = absoluteFrame(element)
        return [
            "windowId": Int(windowId),
            "action": action.rawValue,
            "frame": frame.map { ["x": $0.minX, "y": $0.minY, "width": $0.width, "height": $0.height] } ?? NSNull(),
        ]
    }

    private static func perform(_ element: AXUIElement, _ action: String) throws {
        let result = AXUIElementPerformAction(element, action as CFString)
        guard result == .success else {
            throw ProviderError.coded("accessibility_error", "AXUIElementPerformAction(\(action)) failed with \(result.rawValue)")
        }
    }

    private static func set(_ element: AXUIElement, _ attribute: String, _ value: CFTypeRef) throws {
        let result = AXUIElementSetAttributeValue(element, attribute as CFString, value)
        guard result == .success else {
            throw ProviderError.coded("accessibility_error", "setting \(attribute) failed with \(result.rawValue)")
        }
    }

    private static func press(_ element: AXUIElement, _ buttonAttribute: String) throws {
        guard let button = rawAttributeValue(element, buttonAttribute) else {
            throw ProviderError.coded("accessibility_error", "the window has no \(buttonAttribute)")
        }
        try perform(button as! AXUIElement, kAXPressAction as String)
    }
}

private enum DesktopClipboard {
    static func read() -> [String: Any] {
        let pasteboard = NSPasteboard.general
        return [
            "text": jsonNullable(pasteboard.string(forType: .string)),
            "hasImage": pasteboard.data(forType: .png) != nil || pasteboard.data(forType: .tiff) != nil,
            "types": (pasteboard.types ?? []).map(\.rawValue),
            "changeCount": pasteboard.changeCount,
        ]
    }

    static func write(_ text: String) {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }
}

// MARK: - The one hand on the operator (docs/design/computer-use-full-operator.md §1.3–1.4)

/// The helper's stop switch and action ledger. Lives outside the provider
/// lock on purpose: the hotkey and the signal must land while an action is
/// in flight, and the next posted event must see them.
enum OperatorGuardHost {
    private static let lock = NSLock()
    /// Counts by the window's table, read at launch (`guard_table()` through
    /// `open --env`); until then, or when it could not be read, nothing acts
    /// and `tableRefusal` says why — never a pace of the helper's own.
    nonisolated(unsafe) private static var ledger = OperatorLedger(budget: nil)
    nonisolated(unsafe) private static var tableRefusal: String? = "the guard table has not been read"
    nonisolated(unsafe) private static var hotkeyMonitor: Any?
    nonisolated(unsafe) private static var signalSource: DispatchSourceSignal?
    nonisolated(unsafe) private static var installedAt: Date?
    private static let stopQueue = DispatchQueue(label: "dev.zerocode.computer-use.stop", qos: .userInteractive)

    static func install() {
        installedAt = Date()
        let table = ProcessInfo.processInfo.environment[ActionBudget.windowTableEnvironmentKey] ?? ""
        let budget: ActionBudget?
        let refusal: String?
        do {
            budget = try ActionBudget(windowTable: table)
            refusal = nil
        } catch {
            budget = nil
            refusal = "\(error)"
            fputs("computer-use guard table refused: \(error)\n", stderr)
        }
        lock.lock()
        // A stop that landed before the table keeps standing.
        let standing = ledger.stoppedReason
        ledger = OperatorLedger(budget: budget)
        if let standing { ledger.stop(reason: standing) }
        tableRefusal = refusal
        lock.unlock()
        // The chord on the whole desktop. Needs the same Accessibility grant
        // the input already needs; without it the signal road still works.
        hotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown]) { event in
            let flags = event.modifierFlags
            if isStopHotkey(keyCode: event.keyCode, control: flags.contains(.control), option: flags.contains(.option), command: flags.contains(.command)) {
                stop(reason: StopReason.hotkey)
            }
        }
        // The window's road that does not wait behind an in-flight request —
        // nor behind the main thread: it has a queue of its own.
        signal(SIGUSR1, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: stopQueue)
        source.setEventHandler { stop(reason: StopReason.signal) }
        source.resume()
        signalSource = source
    }

    /// The stop, on whatever thread it came in on: the hand lets go of what
    /// it pressed first, then the count refuses, then a reflex run ends.
    static func stop(reason: String) {
        let release = OperatorHandHost.hand.stop(reason: reason)
        lock.lock()
        ledger.stop(reason: reason)
        lock.unlock()
        fputs("computer-use operator stopped: \(reason) (let go of \(release.released.count), unconfirmed \(release.unconfirmed.count))\n", stderr)
        ReflexRuntimeHost.operatorStopped(reason: reason)
    }

    static func resume(resetBudget: Bool) {
        lock.lock()
        ledger.resume(resetBudget: resetBudget)
        lock.unlock()
        OperatorHandHost.hand.resume()
    }

    /// One action's admission as the ledger answers it, for a reflex run's
    /// leaf: the run sleeps a pace's wait on the hand, where a stop ends it.
    static func admission() -> GuardAdmission {
        lock.lock()
        defer { lock.unlock() }
        return ledger.admit(now: Date().timeIntervalSince1970)
    }

    /// What the ledger would answer an action now, counting nothing: asked of a
    /// copy, so the session count, a stop and the pace stand as they are. A
    /// reflex run reads it before a spent quota comes back — a renewal is not
    /// an admission.
    static func standing() -> GuardAdmission {
        lock.lock()
        var copy = ledger
        lock.unlock()
        return copy.admit(now: Date().timeIntervalSince1970)
    }

    /// The window's session closed: nothing it asked for outlives it. The
    /// stop's own road lets go of what the hand holds and ends a reflex run,
    /// then the helper exits.
    static func sessionClosed(then exit: () -> Void) {
        stop(reason: StopReason.sessionClosed)
        exit()
    }

    /// Count one action — waiting for the pace to come round when a burst is
    /// spent — or refuse with the reason the skill recovers by. The stop is
    /// read again after every wait, so the hotkey cuts a paced action too.
    static func admit() throws {
        while true {
            lock.lock()
            let admission = ledger.admit(now: Date().timeIntervalSince1970)
            let refusal = tableRefusal
            lock.unlock()
            switch admission {
            case .admitted:
                return
            case let .wait(seconds):
                // Only a paced table waits, at most a second (`ActionBudget`);
                // an unlimited one never reaches here.
                Thread.sleep(forTimeInterval: max(0, seconds))
                continue
            case .noBudget:
                throw ProviderError.coded("provider_incompatible", "the helper has no guard table from the window (\(refusal ?? "unread")); restart the Computer Use session")
            case let .stopped(reason):
                throw ProviderError.coded("stopped", "the operator is stopped (\(reason)); `zerocode-computer resume` lifts it")
            case let .sessionBudget(limit):
                throw ProviderError.coded("budget_exceeded", "\(limit) actions this session — the operator stopped itself; `zerocode-computer resume --reset-budget` goes on")
            }
        }
    }

    /// The table the hand counts by, in the window's shape (`null` without one).
    static func renderedBudget() -> Any {
        lock.lock()
        let budget = ledger.budget
        lock.unlock()
        return budget?.rendered ?? NSNull()
    }

    static func status() -> [String: Any] {
        lock.lock()
        let snapshot = ledger
        let refusal = tableRefusal
        lock.unlock()
        let now = Date().timeIntervalSince1970
        return [
            "stopped": snapshot.isStopped,
            "reason": jsonNullable(snapshot.stoppedReason),
            "actions": snapshot.actions,
            "actionsLastMinute": snapshot.actionsInLastMinute(now: now),
            "lastActionAt": snapshot.lastActionAt.map { Int($0 * 1_000) } ?? NSNull(),
            "budget": snapshot.budget?.rendered ?? NSNull(),
            "budgetRefusal": jsonNullable(refusal),
            "hotkey": "control+option+escape",
            "hotkeyArmed": hotkeyMonitor != nil,
            // While a process holds the keyboard's secure mode, other apps'
            // monitors hear no keys (TN2150): the chord is armed but deaf.
            "hotkeyHears": hotkeyMonitor != nil && !IsSecureEventInputEnabled(),
            "secureInput": SecureInputState.now().rendered,
            "upSince": installedAt.map { Int($0.timeIntervalSince1970 * 1_000) } ?? NSNull(),
        ]
    }
}

/// The one hand as the verbs hold it (realtime v1 §5.4): a request posts only
/// while it holds the hand — never beside a reflex run — every event goes
/// through it, stamped with its tag, and a stop lets go of what it pressed.
enum OperatorHandHost {
    static let hand = OperatorHand(
        poster: CGEventHandPoster(),
        clock: HostUptimeClock(),
        sleeper: SemaphoreSleeper(),
        tag: Int64.random(in: 1...Int64.max)
    )
    private static let lock = NSLock()
    /// The hold of the request running now: requests run one at a time under
    /// the provider lock, so there is at most one.
    nonisolated(unsafe) private static var request: OperatorHand.Token?

    /// Run one acting request while it holds the hand; whatever it left held
    /// is let go of when it ends.
    static func whileHeld<T>(_ body: () throws -> T) throws -> T {
        let token: OperatorHand.Token
        do {
            token = try hand.acquire(.request)
        } catch let refusal as OperatorHand.Refusal {
            throw providerError(refusal, acquiring: true)
        }
        lock.lock()
        request = token
        lock.unlock()
        defer {
            lock.lock()
            request = nil
            lock.unlock()
            hand.relinquish(token)
        }
        return try body()
    }

    /// Post one event for the request holding the hand.
    static func post(_ event: HandEvent) throws {
        let token = try current()
        do {
            try hand.post(event, by: token)
        } catch let refusal as OperatorHand.Refusal {
            throw providerError(refusal, acquiring: false)
        }
    }

    /// Wait on the hand's clock for the request holding it: a stop ends the
    /// wait at once with the refusal.
    static func sleep(nanoseconds: UInt64) throws {
        let token = try current()
        do {
            try hand.sleep(untilNs: hand.nowNs() &+ nanoseconds, by: token)
        } catch let refusal as OperatorHand.Refusal {
            throw providerError(refusal, acquiring: false)
        }
    }

    /// Post a path's waypoints as they come due — the newest one due each
    /// time, so a hand woken late skips to where it should be.
    static func walk(_ path: [PointerWaypoint], _ make: (PointerWaypoint) -> HandEvent) throws {
        let token = try current()
        var posted = -1
        while posted + 1 < path.count {
            do {
                try hand.sleep(untilNs: path[posted + 1].dueNs, by: token)
            } catch let refusal as OperatorHand.Refusal {
                throw providerError(refusal, acquiring: false)
            }
            guard let index = PointerSchedule.due(path, after: posted, nowNs: hand.nowNs()) else { continue }
            try post(make(path[index]))
            posted = index
        }
    }

    private static func current() throws -> OperatorHand.Token {
        lock.lock()
        defer { lock.unlock() }
        guard let request else {
            throw ProviderError.coded("stopped", "no request holds the hand")
        }
        return request
    }

    /// The hand's refusal in the words the skill recovers by — before an
    /// action (`acquiring`) or in the middle of one.
    static func providerError(_ refusal: OperatorHand.Refusal, acquiring: Bool) -> ProviderError {
        switch refusal {
        case let .stopped(reason), let .revoked(reason):
            return acquiring
                ? .coded("stopped", "the operator is stopped (\(reason)); `zerocode-computer resume` lifts it")
                : .coded("stopped", "the operator was stopped mid-action (\(reason)); `zerocode-computer resume` lifts it")
        case let .busy(.reflex(run)):
            return .coded("hand_busy", "reflex run \(run) holds the hand; stop it first (reflexStop)")
        case .busy(.request):
            return .coded("hand_busy", "another request holds the hand")
        case .notHolder:
            return .coded("stopped", "this request no longer holds the hand")
        case .releaseUnconfirmed:
            return .coded("stopped", "a release the hand posted could not be confirmed; check the keys and buttons, then `zerocode-computer resume`")
        }
    }
}

/// ZeroCode's own window is never the operator's target (§1.6): a desktop
/// point that lands on it, or keys while it is frontmost, are refused unless
/// the caller said `--allow-self` (the test harness does).
enum DesktopSelf {
    /// Whose window a click at `point` lands on: the frontmost that hides
    /// what is under it — an overlay (`DesktopOverlay`) does not, and taking
    /// the Dock's for one let every click through to ZeroCode's own window.
    static func ownerPid(at point: CGPoint) -> pid_t? {
        guard let infos = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else { return nil }
        let displays = DesktopScreen.displays().map(\.bounds)
        for info in infos {
            guard let layer = info[kCGWindowLayer as String] as? Int, layer >= 0,
                  let alpha = info[kCGWindowAlpha as String] as? CGFloat, alpha > 0.01,
                  let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDictionary),
                  bounds.contains(point),
                  let ownerPid = info[kCGWindowOwnerPID as String] as? pid_t
            else { continue }
            if DesktopOverlay.isOverlay(layer: layer, bounds: bounds, displays: displays) {
                // Its background is transparent but its icons receive input.
                var hit: AXUIElement?
                if AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(), Float(point.x), Float(point.y), &hit) == .success,
                   let hit, let actual = pidAttribute(hit) {
                    return actual
                }
                continue
            }
            return ownerPid
        }
        return nil
    }

    static func refuseOwnWindow(at point: CGPoint) throws {
        try refuseOwn(ownerPid(at: point), at: point)
    }

    /// The policy itself, for whoever already knows whose window is under the
    /// point (a reflex run's press asks it too, `DesktopRunBoundary`).
    static func refuseOwn(_ pid: pid_t?, at point: CGPoint, isOwn: (pid_t) -> Bool = DesktopSelf.isOwn) throws {
        guard let pid, isOwn(pid) else { return }
        throw ProviderError.coded("app_blocked", "(\(Int(point.x)), \(Int(point.y))) lands on ZeroCode's own window; the operator does not drive the app it lives in")
    }

    /// Whether `pid` is ZeroCode itself.
    static func isOwn(_ pid: pid_t) -> Bool {
        isTrustedZeroCodeApplication(pid)
    }

    static func refuseOwnFrontmost() throws {
        guard let front = NSWorkspace.shared.frontmostApplication, isTrustedZeroCodeApplication(front.processIdentifier) else { return }
        throw ProviderError.coded("app_blocked", "ZeroCode's own window is frontmost; keys would land in it — activate the app you mean first")
    }
}

// MARK: - The last step is the person's (docs/design/computer-use-full-operator.md §1.5)

/// The words the window sent, kind by kind, or nothing when it guards nothing.
private func confirmGuardWords(_ value: JSONValue?) -> [String: [String]]? {
    guard case let .object(kinds)? = value else { return nil }
    var words: [String: [String]] = [:]
    for (kind, list) in kinds {
        guard case let .array(items) = list else { continue }
        words[kind] = items.compactMap(\.string)
    }
    return words.isEmpty ? nil : words
}

/// What a control shows a person: its title, else its description, else its
/// value, else the first static text inside it — the label a word table reads.
private func faceLabel(_ element: AXUIElement, depth: Int = 0) -> String? {
    for attribute in [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute] as [String] {
        if let text = stringAttribute(element, attribute), !text.trimmingCharacters(in: .whitespaces).isEmpty {
            return text
        }
    }
    guard depth < 2, let children = copyArray(element, kAXChildrenAttribute as String) else { return nil }
    for child in children.prefix(6) {
        if let text = faceLabel(child, depth: depth + 1) { return text }
    }
    return nil
}

/// The control under a desktop point, read through its own label or its
/// nearest labelled ancestor (a button's text is often a child static text).
/// The label of what a press at a screen point lands on — of the app that
/// will be pressed when one is named (a window a click raises first may be
/// covered now), else of whatever shows there.
private func faceLabelAtPoint(_ point: CGPoint, in pid: pid_t? = nil) -> String? {
    var hit: AXUIElement?
    let root = pid.map(AXUIElementCreateApplication) ?? AXUIElementCreateSystemWide()
    guard AXUIElementCopyElementAtPosition(root, Float(point.x), Float(point.y), &hit) == .success,
          var element = hit
    else { return nil }
    for _ in 0..<3 {
        if let label = faceLabel(element) { return label }
        guard let parent = copyElement(element, kAXParentAttribute as String) else { return nil }
        element = parent
    }
    return nil
}

extension Provider {
    /// The label a press is about to land on, by method — nil when the press
    /// has no readable target (a bare key, a point over nothing).
    func confirmTarget(method: String, params: [String: JSONValue], snapshot loadSnapshot: () throws -> Snapshot) throws -> String? {
        switch method {
        case "click", "performSecondaryAction":
            let snapshot = try loadSnapshot()
            if let index = try optionalInteger(params, "elementIndex") {
                return faceLabel(try element(snapshot, index).element)
            }
            // A click by reading is held for what it would press; one that
            // cannot choose is refused by the press itself, not asked about.
            let query = readingQuery(params)
            if method == "click", !query.isEmpty {
                guard let index = try? chosenIndex(query, in: snapshot) else { return nil }
                return faceLabel(try element(snapshot, index).element)
            }
            // A click at a window's point presses whatever is under it — read
            // it there, as a desktop click is read: a saved coordinate click
            // on "Place order" is the person's last step too.
            guard method == "click" else { return nil }
            return faceLabelAtPoint(try coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot), in: snapshot.app.pid)
        case "mouseClick":
            return faceLabelAtPoint(try desktopPoint(params, "x", "y"))
        case "mouseDrag":
            // A drag is a click on what it lets go of: a control fires on the
            // mouse-up only where the pointer is released.
            return faceLabelAtPoint(try desktopPoint(params, "toX", "toY"))
        case "key", "holdKey", "pressKey", "hotkey":
            guard let chord = params["key"]?.string, firesDefaultButton(chord) else { return nil }
            if let focused = systemWideFocusedElement() {
                if let label = faceLabel(focused) { return label }
                if let window = containingWindow(focused),
                   let button = copyElement(window, kAXDefaultButtonAttribute as String),
                   let label = faceLabel(button) {
                    return label
                }
            }
            return nil
        default:
            return nil
        }
    }
}

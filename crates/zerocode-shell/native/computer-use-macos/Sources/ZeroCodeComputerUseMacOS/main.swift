import AppKit
@preconcurrency import ApplicationServices
import Carbon.HIToolbox
import CoreGraphics
import CryptoKit
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

/// What one request has done so far, read on its own thread: requests in
/// different lanes run at once, so nothing about one of them may live in a shared
/// place.
final class RequestContext {
    /// Whether anything reached the app. A refusal after this may have landed.
    var sent = false
    /// The agent the window says this request is for: whose pointer it draws.
    var agent: String?
    /// Whether the person's setting lets this request borrow the shared desktop
    /// when the background road cannot go.
    var mayBorrowDesktop = false
    /// Set when the request borrowed the shared desktop.
    var borrowed = false
    /// Where this request last put the desktop's pointer; nil when it never moved it.
    var pointerPosted: SmoothPointerPath.Point?
    /// Whether this request goes through the input the person is using, so that
    /// what it sends first asks them first.
    var asksThePerson = false

    /// The last moment that can still say nothing was sent: any window the verb
    /// raises on its way here is up by now, and a stop that came meanwhile stands.
    func askThePersonIfFirst() throws {
        guard asksThePerson, !sent else { return }
        try OperatorHandHost.sleep(nanoseconds: 0)
        try DesktopBorrowHost.refuseIfThePersonIsBusy()
    }
    /// How long the app itself took to perform the action, in ms.
    var appActionMs: Int?

    private static let key = "dev.zerocode.computer-use.request"

    static var current: RequestContext {
        let thread = Thread.current
        if let context = thread.threadDictionary[key] as? RequestContext { return context }
        let context = RequestContext()
        thread.threadDictionary[key] = context
        return context
    }

    static func begin(_ params: [String: JSONValue]) {
        let context = RequestContext()
        context.agent = params["agent"]?.string
        context.mayBorrowDesktop = params["mayBorrowDesktop"]?.bool == true
        Thread.current.threadDictionary[key] = context
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
    let plainInput: String?
    let traits: [String]
    /// The frame cut by every clipping container above it (`MarkPin.clipRoles`),
    /// and the nearest named element above it (the row a star sits in).
    let visible: CGRect?
    let context: String?
    /// The named element above it that `context` was read from.
    let contextOwner: AXUIElement?
    /// The line the look showed for it: what the next look is compared against.
    let line: String
    /// What the walk read of it, so finding by words asks the app nothing more.
    let title: String?
    let label: String?
    let value: String?
    /// Whether it lives on a menu or the menu bar, which no window holds: nothing
    /// is on top of it, and it is pressed only by its accessibility action.
    let onSurface: Bool

    init(
        index: Int,
        element: AXUIElement,
        localFrame: CGRect?,
        actions: [String],
        signature: String,
        role: String = "",
        name: String? = nil,
        placeholder: String? = nil,
        plainInput: String? = nil,
        traits: [String] = [],
        visible: CGRect? = nil,
        context: String? = nil,
        contextOwner: AXUIElement? = nil,
        line: String = "",
        title: String? = nil,
        label: String? = nil,
        value: String? = nil,
        onSurface: Bool = false
    ) {
        self.index = index
        self.element = element
        self.localFrame = localFrame
        self.actions = actions
        self.signature = signature
        self.role = role
        self.name = name
        self.placeholder = placeholder
        self.plainInput = plainInput
        self.traits = traits
        self.visible = visible
        self.context = context
        self.contextOwner = contextOwner
        self.line = line
        self.title = title
        self.label = label
        self.value = value
        self.onSurface = onSurface
    }

    /// The words a mark's pin holds to: the name, else the placeholder.
    var words: String? { name ?? placeholder }

    /// The same element as it stands now: `actions` and `frame` read just now.
    func standing(actions: [String], frame: CGRect?) -> ElementRecord {
        let moved = frame.map { new in localFrame.map { !Actionability.framesAgree($0, new) } ?? true } ?? false
        return ElementRecord(
            index: index, element: element, localFrame: frame ?? localFrame, actions: actions, signature: signature,
            role: role, name: name, placeholder: placeholder, plainInput: plainInput, traits: traits,
            visible: moved ? frame : visible, context: context, contextOwner: contextOwner, line: line,
            title: title, label: label, value: value, onSurface: onSurface
        )
    }
}

struct Snapshot {
    let id: String
    let app: AppDescriptor
    let windowTitle: String
    let windowBounds: CGRect
    let windowId: CGWindowID
    let windowLayer: Int
    /// The identity of the document the window shows (`WindowDocument`),
    /// never its path or address.
    let document: String?
    let treeText: String
    let focusedElementId: Int?
    let screenshot: ScreenshotPayload?
    let screenshotStatus: ScreenshotStatus
    let screenshotScale: CGSize
    let screenshotEngine: String?
    let elements: [Int: ElementRecord]
    let truncated: Bool
    let maxDepthReached: Bool
    /// The elements' numbers in reading order. A number no longer says where an
    /// element stands, so nothing reads the order off the numbers.
    var order: [Int] = []
    /// The number of the element the walk started at.
    var rootIndex: Int?
    /// Why the walk stopped early, when it was not the node limit.
    var cutShort: String?

    /// What is kept between requests. Numbers outlive a look, so what is kept
    /// only has to resolve them: no picture and no text.
    func keptBetweenRequests() -> Snapshot {
        var kept = Snapshot(
            id: id,
            app: app,
            windowTitle: windowTitle,
            windowBounds: windowBounds,
            windowId: windowId,
            windowLayer: windowLayer,
            document: document,
            treeText: "",
            focusedElementId: focusedElementId,
            screenshot: nil,
            screenshotStatus: .skipped,
            screenshotScale: CGSize(width: 1, height: 1),
            screenshotEngine: nil,
            elements: elements,
            truncated: truncated,
            maxDepthReached: maxDepthReached
        )
        kept.order = order
        kept.rootIndex = rootIndex
        kept.cutShort = cutShort
        return kept
    }

    /// The kept look with what was read just now, without walking the window.
    func standing(windowBounds bounds: CGRect, focusedElementId focused: Int?, elements live: [Int: ElementRecord]) -> Snapshot {
        var fresh = Snapshot(
            id: id,
            app: app,
            windowTitle: windowTitle,
            windowBounds: bounds,
            windowId: windowId,
            windowLayer: windowLayer,
            document: document,
            treeText: "",
            focusedElementId: focused,
            screenshot: nil,
            screenshotStatus: .skipped,
            screenshotScale: screenshotScale,
            screenshotEngine: nil,
            elements: elements.merging(live) { _, new in new },
            truncated: truncated,
            maxDepthReached: maxDepthReached
        )
        fresh.order = order
        fresh.rootIndex = rootIndex
        fresh.cutShort = cutShort
        return fresh
    }
}

/// The document a window shows, said only as its identity (t-69973): the
/// first 16 hex digits of the SHA-256 of the window's `AXDocument`, else of
/// the address its first web area reads (`AXURL`). Two windows under one
/// title tell apart by it; the path or the address itself stays here, in no
/// answer, line or kept snapshot.
enum WindowDocument {
    /// The key a window object carries the identity under.
    static let key = "document"
    /// How many hex digits of the digest the identity keeps.
    static let digits = 16

    /// The identity of `document`, else of `webAreaURL`; `nil` when the
    /// window gives neither.
    static func identity(document: String?, webAreaURL: String?) -> String? {
        guard let shown = [document, webAreaURL].lazy.compactMap({ $0 }).first(where: { !$0.isEmpty }) else {
            return nil
        }
        return SHA256.hash(data: Data(shown.utf8))
            .prefix(digits / 2)
            .map { String(format: "%02x", $0) }
            .joined()
    }
}

/// The window an answer is of, by what tells it apart: its id, its title
/// and, when the window gives one, its document's identity. `find` answers
/// it as it is.
func namedWindow(_ snapshot: Snapshot) -> [String: Any] {
    var window: [String: Any] = ["id": Int(snapshot.windowId), "title": snapshot.windowTitle]
    if let document = snapshot.document {
        window[WindowDocument.key] = document
    }
    return window
}

/// The window a look is of: what names it (`namedWindow`) and where it stands.
func placedWindow(_ snapshot: Snapshot) -> [String: Any] {
    namedWindow(snapshot).merging([
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
    ]) { named, _ in named }
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
    /// The namespace and window it is of: one look per window is kept.
    let windowKey: String
}

final class Provider {
    private var snapshots: [String: Snapshot] = [:]
    private var snapshotEntries: [CachedSnapshotEntry] = []
    /// Guards everything here that lanes share. Held only to read or write the
    /// tables, never across a call into an app.
    private let cacheLock = NSLock()
    /// Each app's element numbers, and every element it last showed by number.
    private var books: [pid_t: ElementIdentityBook<AXUIElement>] = [:]
    private var tables: [pid_t: [Int: ElementRecord]] = [:]
    /// The numbers each app draws from, kept past any one of its processes.
    private var numberSources: [String: ElementNumbers] = [:]
    /// The agents each process has shown numbers to since it or this helper started.
    private var shownTo: [pid_t: Set<String>] = [:]
    /// The apps whose web accessibility was switched on this session.
    private var webAccessibilityAsked: Set<pid_t> = []
    /// When each app's web accessibility was switched on.
    private var webAccessibilityOnAt: [pid_t: Date] = [:]

    private func book(for pid: pid_t) -> ElementIdentityBook<AXUIElement> {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        if let book = books[pid] { return book }
        let owner = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier ?? "pid:\(pid)"
        let numbers = numberSources[owner] ?? ElementNumbers()
        numberSources[owner] = numbers
        let book = ElementIdentityBook<AXUIElement>(
            numbers: numbers,
            hash: { CFHash($0) },
            equal: { CFEqual($0, $1) }
        )
        books[pid] = book
        return book
    }

    private func knownRecord(pid: pid_t, index: Int) -> ElementRecord? {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        return tables[pid]?[index]
    }

    /// File what a walk showed under its numbers. `gone` left the screen.
    private func fileRecords(pid: pid_t, records: [Int: ElementRecord], gone: [Int]) {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        var table = tables[pid] ?? [:]
        for number in gone { table[number] = nil }
        table.merge(records) { _, new in new }
        tables[pid] = table
        if let agent = RequestContext.current.agent { shownTo[pid, default: []].insert(agent) }
        // A process that ended takes its elements with it.
        for known in Array(tables.keys) where known != pid && !pidIsLive(known) {
            tables[known] = nil
            books[known] = nil
            shownTo[known] = nil
            webAccessibilityAsked.remove(known)
            webAccessibilityOnAt[known] = nil
        }
    }

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
    /// The verbs that go through the pointer or the keyboard the person is using,
    /// when they run on the shared desktop rather than in one window.
    static let handMethods: Set<String> = [
        "mouseMove", "mouseClick", "mouseDrag", "mouseScroll", "key", "holdKey", "type",
        "click", "drag", "scroll", "typeText", "pressKey", "hotkey", "pasteText",
        "setValue", "performSecondaryAction",
    ]

    /// The lane a request runs in: the app it names, so one app's slow answer
    /// holds up nobody else. Whatever names no app shares one lane, as it shared
    /// one lock before.
    func lane(method: String, params: [String: JSONValue]) -> String {
        if let query = params["app"]?.string, !query.isEmpty {
            let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
            if let pid = parsePid(trimmed) { return "pid:\(pid)" }
            if let app = listApps().first(where: { matches($0, query: trimmed) }) { return "pid:\(app.pid)" }
            return "app:\(trimmed.lowercased())"
        }
        if let id = params["windowId"]?.number.flatMap(UInt32.init(exactly:)), let owner = DesktopWindows.ownerPid(windowId: id) {
            return "pid:\(owner)"
        }
        return "desktop"
    }

    func handle(method: String, params: [String: JSONValue]) throws -> Any {
        if let id = params["windowId"]?.number.flatMap(UInt32.init(exactly:)), let owner = DesktopWindows.ownerPid(windowId: id) {
            try ShareControlPanel.refuseHelper(pid: owner)
        }
        if params["background"] != nil && params["background"]?.bool == nil {
            throw ProviderError.coded("invalid_argument", "background must be a boolean")
        }
        if let refusal = BackgroundInput.validate(
            method: method, requested: params["background"]?.bool == true,
            restoresWindow: params["restoreWindow"]?.bool == true,
            hasApp: params["app"]?.string?.isEmpty == false
        ) {
            guard refusal == .requiresForeground, RequestContext.current.mayBorrowDesktop, Self.actingMethods.contains(method) else {
                throw ProviderError.coded(refusal.rawValue, "background input requires an app-scoped action; no foreground fallback was attempted")
            }
            try ShareControlPanel.refuseInput(method: method, params: params)
            return try borrowingDesktop(method: method, params: foregroundParams(params))
        }
        guard Self.actingMethods.contains(method) else { return try dispatch(method: method, params: params) }
        try ShareControlPanel.refuseInput(method: method, params: params)
        // A control that is about to be ready is waited for before any turn is
        // taken: waiting holds up nobody. It is read again once the turn is held.
        try waitUntilActionable(method: method, params: params)
        let background = params["background"]?.bool == true
        if !background, RequestContext.current.mayBorrowDesktop {
            return try borrowingDesktop(bringingUp: params["borrowApp"]?.string, method: method, params: params)
        }
        do {
            return try act(method: method, params: params, scoped: background)
        } catch let error as ProviderError where background && RequestContext.current.mayBorrowDesktop
            && !RequestContext.current.sent && BackgroundInput.foregroundMayTakeOver(method: method, refusal: error.code) {
            // The background road cannot do this one. The person's setting lets
            // the agent borrow the pointer and the keyboard, so it does, and
            // gives them back when it is done.
            return try borrowingDesktop(method: method, params: foregroundParams(params))
        }
    }

    /// One hand (realtime v1 §5.4): an acting request holds its part of it from
    /// its count to its last event — never beside a reflex run — and a stop lets
    /// go of whatever it pressed. An act on the shared desktop goes through the
    /// input the person is using, so it asks them first, whether or not it is a
    /// borrow, and hands the pointer back unless moving it was the point.
    private func act(method: String, params: [String: JSONValue], scoped: Bool) throws -> Any {
        let shared = !scoped && Self.handMethods.contains(method)
        return try OperatorHandHost.whileHeld(scoped: scoped) {
            try OperatorGuardHost.admit()
            guard shared else {
                // What repaints from here on may be this act's doing (§7.1 eye).
                ScreenEye.markAct()
                return try dispatch(method: method, params: params)
            }
            try DesktopBorrowHost.waitForThePerson()
            RequestContext.current.asksThePerson = true
            let before = frontPidNow()
            let pointer = Input.desktopCursorPosition()
            defer {
                if !RequestContext.current.sent {
                    // Refused with nothing sent: a window raised on the way leaves too.
                    giveFrontBack(before: before, raised: params["app"]?.string.flatMap { try? resolveApp($0) }?.pid)
                } else if method != "mouseMove",
                          RequestContext.current.pointerPosted.map({ $0.x != pointer.x || $0.y != pointer.y }) == true {
                    Thread.sleep(forTimeInterval: Self.borrowSettleSeconds)
                    givePointerBack(pointer)
                }
            }
            ScreenEye.markAct()
            return try dispatch(method: method, params: params)
        }
    }

    private func foregroundParams(_ params: [String: JSONValue]) -> [String: JSONValue] {
        var foreground = params
        foreground["background"] = nil
        foreground["restoreWindow"] = .bool(true)
        return foreground
    }

    /// How long what a borrowed act sent gets to arrive before the person's app is back.
    private static let borrowSettleSeconds: TimeInterval = 0.15

    /// Take the shared desktop for one act and put back what the person had: the
    /// app in front and where the pointer was. The person goes first: a borrow waits
    /// for a pause in their mouse and keys, looks again just before its first event,
    /// once any window is up, and leaves standing whatever they did meanwhile. All of
    /// it happens with the hand held, so a borrow in turn after this one reads what
    /// the person had, and raising a window or giving it back never lands in the
    /// middle of another act's keys.
    private func borrowingDesktop(bringingUp query: String? = nil, method: String, params: [String: JSONValue]) throws -> Any {
        try OperatorHandHost.whileHeld(scoped: false) {
            try OperatorGuardHost.admit()
            try DesktopBorrowHost.waitForThePerson()
            RequestContext.current.borrowed = true
            RequestContext.current.asksThePerson = true
            let before = frontPidNow()
            let pointer = Input.desktopCursorPosition()
            let raising = try query.flatMap { $0.isEmpty ? nil : try resolveApp($0) }
            let raised = raising ?? params["app"]?.string.flatMap { try? resolveApp($0) }
            defer {
                // What was sent is still on its way: what the person had comes back after it.
                if RequestContext.current.sent { Thread.sleep(forTimeInterval: Self.borrowSettleSeconds) }
                giveBack(pointer: pointer, before: before, raised: raised?.pid)
            }
            if let raising {
                recoverWindow(raising)
                // A stop that came while the window rose ends this before any press.
                try OperatorHandHost.sleep(nanoseconds: 0)
                try refuseACoveredPoint(method: method, params: params, target: raising)
            }
            ScreenEye.markAct()
            return try dispatch(method: method, params: params)
        }
    }

    private func giveBack(pointer: CGPoint, before: pid_t?, raised: pid_t?) {
        givePointerBack(pointer)
        // Only an app this borrow put in front leaves anyone thinking it is still there.
        if let raised, raised != before, RequestContext.current.sent {
            DesktopBorrowHost.gaveBack(agent: RequestContext.current.agent)
        }
        giveFrontBack(before: before, raised: raised)
    }

    private func giveFrontBack(before: pid_t?, raised: pid_t?) {
        guard DesktopBorrow.givesFrontBack(before: before, raised: raised, now: frontPidNow()),
              let before, let app = NSRunningApplication(processIdentifier: before), !app.isTerminated else { return }
        _ = app.activate(options: [])
        // A helper in the background is not always let bring an app up. `open` takes a
        // moment, and it does not go over an app the person came to in the meantime.
        if !raiseSettled(before, timeout: DesktopBorrowHost.activateTimeout),
           frontPidNow() == raised, let bundle = app.bundleIdentifier {
            openBundle(bundle)
        }
    }

    /// Put the pointer back if this request moved it, with what the person moved since
    /// its last event added; a move that carries it off every display is not kept.
    private func givePointerBack(_ pointer: CGPoint) {
        let here = Input.desktopCursorPosition()
        guard let back = DesktopBorrow.pointerAfter(
            saved: .init(x: pointer.x, y: pointer.y),
            lastPosted: RequestContext.current.pointerPosted,
            now: .init(x: here.x, y: here.y)
        ) else { return }
        let wanted = CGPoint(x: back.x, y: back.y)
        var displays: UInt32 = 0
        CGGetDisplaysWithPoint(wanted, 0, nil, &displays)
        CGWarpMouseCursorPosition(displays > 0 ? wanted : pointer)
        _ = CGAssociateMouseAndMouseCursorPosition(1)
        DesktopBorrowHost.noteOwnWarp()
    }

    /// A click read off a picture lands wherever the window stands now: a window that
    /// is still coming up is given a moment, and one that stays in front of the point
    /// would take the press, so nothing is sent.
    private func refuseACoveredPoint(method: String, params: [String: JSONValue], target: AppDescriptor) throws {
        for (xKey, yKey) in Self.pointedMethods[method] ?? [] {
            let point = try desktopPoint(params, xKey, yKey)
            let clear = DesktopBorrow.waitUntilFront(
                timeout: DesktopBorrowHost.raiseTimeout,
                interval: DesktopBorrowHost.raisePollSeconds,
                isFront: { DesktopSelf.ownerPid(at: point) == target.pid },
                now: { ProcessInfo.processInfo.systemUptime },
                sleep: { Thread.sleep(forTimeInterval: $0) }
            )
            guard clear else {
                throw ProviderError.coded("covered", "another window is over (\(Int(point.x)), \(Int(point.y))); nothing was sent. Look again before pressing")
            }
        }
    }

    private func dispatch(method: String, params unmarked: [String: JSONValue]) throws -> Any {
        var params = unmarked
        params["method"] = .string(method)
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
        try ShareControlPanel.refuseInput(method: method, params: params)
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
                keys: Data(try requiredString(params, "keys").utf8),
                eye: eye,
                display: try requiredInteger(params, "display"),
                hand: OperatorHandHost.hand,
                admit: { OperatorGuardHost.admission() },
                standing: { OperatorGuardHost.standing() },
                actingScope: { try actingScope($0) },
                benchPressAim: params["benchPressAim"]?.string
            )
        case "resume":
            let lifter = try stopLifter(params)
            try OperatorGuardHost.resume(resetBudget: params["resetBudget"]?.bool == true, by: lifter)
            return OperatorGuardHost.status()
        case "handshake":
            return providerHandshake()
        case "listApps":
            if let named = params["named"]?.string { return namedApp(named) }
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
            try DesktopBorrowHost.refuseAppLessKeys(agent: RequestContext.current.agent)
            let key = try requiredString(params, "key")
            let secureInput = try gateDesktopChord(key)
            try Input.pressKey(key, pid: 0)
            return withSecureInput(["path": "synthetic"], secureInput)
        case "holdKey":
            try DesktopBorrowHost.refuseAppLessKeys(agent: RequestContext.current.agent)
            let ms = try requiredInteger(params, "ms")
            let key = try requiredString(params, "key")
            let secureInput = try gateDesktopChord(key)
            try Input.desktopHoldKey(key, milliseconds: ms)
            return withSecureInput(["path": "synthetic", "ms": ms], secureInput)
        case "type":
            try DesktopBorrowHost.refuseAppLessKeys(agent: RequestContext.current.agent)
            let text = try requiredStringAllowingEmpty(params, "text")
            let (verification, secureInput) = try typeWatched(text, pid: 0, receiver: nil, config: keyboardGuardConfig(params))
            return withSecureInput(["path": "synthetic", "verification": verification], secureInput)
        // Apps, windows and the system (§2.2).
        case "launchApp":
            return try DesktopApps.launch(params: params, running: listApps())
        case "quitApp":
            return try DesktopApps.quit(app: try resolveApp(try requiredString(params, "app")), force: params["force"]?.bool == true)
        case "activateApp":
            let app = try resolveApp(try requiredString(params, "app"))
            let answer = try DesktopApps.activate(app: app)
            // Only an app that really came up says what is in front.
            if raiseSettled(app.pid, timeout: DesktopBorrowHost.activateTimeout) {
                DesktopBorrowHost.namedTheFront(agent: RequestContext.current.agent)
            }
            return answer
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

    /// What an action's answer says of the window afterwards.
    private enum AfterAct {
        /// Only what changed: the default for a lone act.
        static let changes = "changes"
        /// Nothing: a batch looks once, after its last step.
        static let none = "none"
    }

    /// How long a web app gets to repaint before a look says nothing changed:
    /// its accessibility tree follows its renderer, a beat behind the press.
    private static let webSettleMicros: UInt32 = 120_000

    private func lookAfterAct(params: [String: JSONValue], afterAct: String?, before: Snapshot?, action: [String: Any]) throws -> Snapshot {
        var quiet = params
        if afterAct == AfterAct.changes { quiet["noScreenshot"] = .bool(true) }
        let after = try observe(params: quiet)
        guard afterAct == AfterAct.changes, let before, Self.changes(from: before, to: after).isEmpty,
              after.elements.values.contains(where: { $0.role == "AXWebArea" }) else { return after }
        usleep(Self.webSettleMicros)
        return try observe(params: quiet)
    }

    private static func changes(from before: Snapshot, to after: Snapshot) -> ElementChanges {
        ElementChanges.between(
            previous: before.elements.mapValues(\.line),
            current: after.elements.mapValues(\.line),
            order: after.order
        )
    }

    /// An action's small answer: what it did and what it changed. The numbers in
    /// the agent's last look still hold, so no tree comes back.
    private func renderChanges(action: [String: Any], snapshot: Snapshot, changes: ElementChanges, surfaces: [[String: Any]]) -> [String: Any] {
        var metadata = action
        metadata["targetWindowId"] = Int(snapshot.windowId)
        var result: [String: Any] = [
            "action": metadata,
            "snapshot": [
                "id": snapshot.id,
                "app": ["name": snapshot.app.name, "bundleId": jsonNullable(snapshot.app.bundleId), "pid": Int(snapshot.app.pid)],
                "window": placedWindow(snapshot),
                "coordinateSpace": "window",
                "elementCount": snapshot.elements.count,
                "focusedElementId": snapshot.focusedElementId as Any,
            ],
            "changes": [
                "added": changes.added,
                "changed": changes.changed,
                "removed": changes.removed,
                "omitted": changes.omitted,
            ],
            "screenshot": NSNull(),
            "screenshotStatus": renderScreenshotStatus(.skipped, snapshot: snapshot),
        ]
        if !surfaces.isEmpty { result["surfaces"] = surfaces }
        return result
    }

    private static let actsWithNoPoint: Set<String> = ["typeText", "pasteText", "pressKey", "hotkey", "setValue", "performSecondaryAction"]

    /// The pointer the person watches gets to `point` before the act lands
    /// there, as a hand does (agent-desktop's travel). Nothing of the person's
    /// moves: it is the helper's own drawn pointer. A stop meanwhile, up to the
    /// moment the act would go, ends the act with nothing sent. Says where the
    /// pointer was drawn, nil when none could be, so a ring marks this act's
    /// point and never an earlier one.
    @discardableResult
    private func travel(to point: CGPoint, in snapshot: Snapshot) throws -> CGPoint? {
        let move = BackgroundPointerOverlay.travel(
            pid: snapshot.app.pid, windowID: snapshot.windowId, bounds: snapshot.windowBounds,
            point: point, agent: RequestContext.current.agent
        )
        let arrival = SmoothPointerPath.waitOut(
            dueNs: DispatchTime.now().uptimeNanoseconds &+ (move?.nanoseconds ?? 0),
            pollNs: Self.travelPollNanoseconds,
            nowNs: { DispatchTime.now().uptimeNanoseconds },
            isStopped: { OperatorGuardHost.isStopped() },
            sleepNs: { Thread.sleep(forTimeInterval: Double($0) / 1_000_000_000) }
        )
        guard arrival == .arrived else {
            throw ProviderError.coded("stopped", "the person stopped Computer Use while the pointer moved to the point; nothing was sent")
        }
        return move?.point
    }

    /// How often a pointer's move looks for the person's stop.
    private static let travelPollNanoseconds: UInt64 = 15_000_000

    /// Where an act with no point of its own happens, so a person can see it: the
    /// control it names, else the focus. Clicks and scrolls show their own point.
    private func showPointerForKeys(params: [String: JSONValue], look: Snapshot) {
        guard Self.actsWithNoPoint.contains(params["method"]?.string ?? "") else { return }
        let pid = look.app.pid
        let named = (try? optionalInteger(params, "elementIndex")).flatMap { $0 }.flatMap { index in
            look.elements[index] ?? knownRecord(pid: pid, index: index)
        }
        let frame = named.flatMap { absoluteFrame($0.element) } ?? appFocusedElement(pid).flatMap { absoluteFrame($0) }
        let bounds = DesktopWindows.bounds(windowId: look.windowId, ownerPid: pid) ?? look.windowBounds
        var point = CGPoint(x: bounds.midX, y: bounds.midY)
        if let frame, bounds.contains(CGPoint(x: frame.midX, y: frame.midY)) {
            point = CGPoint(x: frame.midX, y: frame.midY)
        }
        BackgroundPointerOverlay.show(pid: pid, windowID: look.windowId, bounds: bounds, point: point, agent: RequestContext.current.agent)
    }

    private func actionResult(params: [String: JSONValue], action runAction: () throws -> [String: Any]) throws -> [String: Any] {
        let foreground = params["background"]?.bool == true ? NSWorkspace.shared.frontmostApplication?.processIdentifier : nil
        // What the window showed before the act, to tell what the act changed.
        let before = try? cachedSnapshot(params: params)
        let acting = Date()
        var action = try runAction()
        if params["timing"]?.bool == true {
            action["actMs"] = Int(Date().timeIntervalSince(acting) * 1_000)
            action["appActionMs"] = RequestContext.current.appActionMs as Any
        }
        if RequestContext.current.borrowed { action["borrowedDesktop"] = true }
        if params["background"]?.bool == true, let before { showPointerForKeys(params: params, look: before) }
        func settleDelivery(effectSeen: Bool) {
            let verified = (action["verification"] as? [String: Any])?["state"] as? String == "verified"
            let delivery = ActionDelivery.finished(readBackVerified: verified, effectSeen: effectSeen)
            action["delivery"] = delivery.rawValue
            action["retry"] = delivery.retry
        }
        if params["background"]?.bool == true {
            action["background"] = [
                "requested": true,
                "foregroundUnchanged": foreground != nil && foreground == NSWorkspace.shared.frontmostApplication?.processIdentifier,
            ]
        }
        if params["deferObservation"]?.bool == true,
           params["noScreenshot"]?.bool == true,
           let window = params["windowId"]?.number,
           let windowID = boundedInteger(window, as: UInt32.self), windowID > 0 {
            action["targetWindowId"] = Int(windowID)
            settleDelivery(effectSeen: false)
            return ["action": action, "observationDeferred": true]
        }
        let afterAct = params["afterAct"]?.string
        if afterAct == AfterAct.none {
            settleDelivery(effectSeen: false)
            if let before { action["targetWindowId"] = Int(before.windowId) }
            return ["action": action, "observationDeferred": true]
        }
        do {
            let after = try lookAfterAct(params: params, afterAct: afterAct, before: before, action: action)
            let changes = before.map { Self.changes(from: $0, to: after) } ?? ElementChanges()
            let surfaces = afterAct == AfterAct.changes ? openSurfaces(after.app.pid) : []
            let effectSeen = !changes.isEmpty || !surfaces.isEmpty
            action["effect"] = before == nil ? "unknown" : (effectSeen ? "changed" : "none_seen")
            settleDelivery(effectSeen: effectSeen)
            guard afterAct == AfterAct.changes else {
                return renderActionResult(action: action, snapshot: after)
            }
            return renderChanges(action: action, snapshot: after, changes: changes, surfaces: surfaces)
        } catch let error as ProviderError where (error.code == "window_not_found" || error.code == "window_stale") && hasRequestedWindowSelector(params) {
            if action["verification"] == nil {
                action["verification"] = ["state": "unverified", "reason": "window_changed"]
            }
            if let windowID = params["windowId"]?.number {
                action["targetWindowId"] = windowID
            }
            settleDelivery(effectSeen: true)
            return [
                "action": action,
                "observationDeferred": true,
                "observation": ["state": "unavailable", "reason": error.code],
            ]
        } catch let error as ProviderError {
            // The act is done. Only the look after it failed, and an answer that
            // called the act a failure would have it done twice.
            settleDelivery(effectSeen: false)
            return [
                "action": action,
                "observationDeferred": true,
                "observation": ["state": "unavailable", "reason": error.code],
            ]
        }
    }

    /// The app's window as the request names it. `acting`: the snapshot an
    /// action is checked against, which only a window the window server lists
    /// may be (t-66753 C1); every other look may read the app's own
    /// accessibility window when the server lists none of the app's.
    private func observe(params: [String: JSONValue], acting: Bool = false) throws -> Snapshot {
        let query = try requiredString(params, "app")
        let windowId = try requestedWindowId(params)
        let windowIndex = try requestedWindowIndex(params)
        let app = try resolveApp(query)
        if params["restoreWindow"]?.bool == true {
            recoverWindow(app)
        }
        let shape = try LookShape(params)
        let snapshot = try buildSnapshot(
            app: app,
            includeScreenshot: params["noScreenshot"]?.bool != true,
            windowId: windowId,
            windowIndex: windowIndex,
            restoreWindow: params["restoreWindow"]?.bool == true,
            readsTheTree: !acting,
            shape: shape
        )
        // A look at a menu or at one region leaves the window's look standing.
        // Its elements resolve from the app's table.
        if shape.complete && shape.surface == nil {
            // Why: cached snapshots only validate element identity for follow-up
            // actions; retaining MB-scale screenshot base64 in the long-lived agent grows memory.
            rememberSnapshot(
                query: query,
                app: app,
                snapshot: snapshot.keptBetweenRequests(),
                params: params,
                windowIndex: windowIndex
            )
        }
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
        cacheLock.lock()
        defer { cacheLock.unlock() }
        let namespace = snapshotNamespace(params)
        var storedKeys: [String] = []
        let canonicalWindowKey = snapshotCanonicalWindowIdKey(cachedSnapshot.windowId)
        // Numbers outlive a look, so an older look at this window is worth
        // nothing: it only holds the memory of a whole tree.
        let windowKey = namespacedSnapshotKey(namespace, canonicalWindowKey)
        snapshotEntries.removeAll { entry in
            guard entry.windowKey == windowKey else { return false }
            for key in entry.keys where snapshots[key]?.id == entry.snapshotId {
                snapshots.removeValue(forKey: key)
            }
            return true
        }
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
            CachedSnapshotEntry(snapshotId: cachedSnapshot.id, keys: storedKeys, createdAt: Date(), windowKey: windowKey)
        )
        pruneSnapshotCacheLocked()
    }

    private func pruneSnapshotCache() {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        pruneSnapshotCacheLocked()
    }

    private func pruneSnapshotCacheLocked() {
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
        if let standing = try standingSnapshot(params: params) { return standing }
        let cached = try cachedSnapshot(params: params)
        try requireShownBeforeThisWalk(params: params, cached: cached)
        // Why: cached AX frames can be stale after a window move or resize, and
        // stale geometry can turn an intended action into a misclick.
        let snapshot = try observe(params: params.merging(["noScreenshot": .bool(true)]) { _, replacement in replacement }, acting: true)
        try validateRequestedElements(cached: cached, current: snapshot, params: params)
        return snapshot
    }

    /// The numbers a request names, whichever way it names them.
    private func requestedNumbers(_ params: [String: JSONValue]) throws -> [Int] {
        try ["elementIndex", "fromElementIndex", "toElementIndex"].compactMap { key -> Int? in
            guard params[key]?.number != nil else { return nil }
            return try optionalInteger(params, key)
        }
    }

    /// A walk numbers what it meets, so a number only proves something if it was
    /// shown before this request walked. One kept from before this helper or this
    /// process started would otherwise name whatever the walk just numbered.
    private func requireShownBeforeThisWalk(params: [String: JSONValue], cached: Snapshot?) throws {
        let requested = try requestedNumbers(params)
        guard !requested.isEmpty, try markPin(params) == nil else { return }
        // A look at a process that has ended says nothing about the one running now.
        let looked = cached.flatMap { $0.app.app.isTerminated ? nil : $0.app.pid }
        let pid = try looked ?? resolveApp(try requiredString(params, "app")).pid
        try requireShownToThisAgent(requested, pid: pid)
        for index in requested where knownRecord(pid: pid, index: index) == nil {
            throw staleElement(index)
        }
    }

    /// Another agent's look can give out a number again after a restart. The agent
    /// asking must have been shown this process's numbers itself. A request the
    /// window names no agent for is judged by the numbers alone.
    private func requireShownToThisAgent(_ requested: [Int], pid: pid_t) throws {
        guard let agent = RequestContext.current.agent, let first = requested.first else { return }
        cacheLock.lock()
        let shown = shownTo[pid]?.contains(agent) == true
        cacheLock.unlock()
        guard shown else {
            throw ProviderError.coded(
                "element_not_found",
                "element \(first) is from before this app or the helper restarted; look again and use a current number"
            )
        }
    }

    private func currentKeyboardSnapshot(params: [String: JSONValue]) throws -> Snapshot {
        // Why: AX text replacement/select-all do not post global input, so only
        // synthetic fallback paths require the target window to be focused.
        try currentSnapshot(params: params.merging(["noScreenshot": .bool(true)]) { _, replacement in replacement })
    }

    /// The window's last look brought up to now by reading only what this request
    /// touches: the window's place, the focus and the named elements. Reading the
    /// whole window for every press cost one tree walk per act, in the app being
    /// driven. Nil when only a walk can answer: no look yet, a press by words, or
    /// a focus the last look never saw.
    private func standingSnapshot(params: [String: JSONValue]) throws -> Snapshot? {
        guard readingQuery(params).isEmpty, params["restoreWindow"]?.bool != true,
              let cached = try cachedSnapshot(params: params),
              !cached.app.app.isTerminated,
              let bounds = DesktopWindows.bounds(windowId: cached.windowId, ownerPid: cached.app.pid)
        else { return nil }
        let requested = try requestedNumbers(params)
        if try markPin(params) == nil { try requireShownToThisAgent(requested, pid: cached.app.pid) }
        var live: [Int: ElementRecord] = [:]
        for index in requested {
            guard let record = cached.elements[index] ?? knownRecord(pid: cached.app.pid, index: index) else { return nil }
            live[index] = try liveRecord(record, windowBounds: bounds)
        }
        let focus = appFocusedElement(cached.app.pid)
        let focused = focus.flatMap { book(for: cached.app.pid).existingNumber(for: $0) }
        // Keys go where the focus is. A field the last look did not see needs a look.
        if focus != nil, focused == nil, Self.keyboardMethods.contains(params["method"]?.string ?? "") { return nil }
        let standing = cached.standing(windowBounds: bounds, focusedElementId: focused, elements: live)
        if try markPin(params) != nil {
            try validateRequestedElements(cached: cached, current: standing, params: params)
        }
        return standing
    }

    private static let keyboardMethods: Set<String> = ["typeText", "pressKey", "hotkey", "pasteText"]

    /// One element as it stands now, read from the element itself. The object is
    /// what the agent named: if the app removed it, this says so, and no other
    /// control can answer in its place.
    private func liveRecord(_ record: ElementRecord, windowBounds: CGRect) throws -> ElementRecord {
        var probe: CFTypeRef?
        switch AXUIElementCopyAttributeValue(record.element, kAXRoleAttribute as CFString, &probe) {
        case .success:
            break
        case .cannotComplete:
            throw ProviderError.coded("app_unresponsive", "the app is not answering; nothing was pressed")
        default:
            throw staleElement(record.index)
        }
        let reader = AXSnapshotReader()
        let role = reader.stringAttribute(record.element, kAXRoleAttribute as String) ?? "AXUnknown"
        let rawActions = reader.actions(record.element)
        let node = SnapshotRenderNode(
            role: role,
            roleDescription: reader.stringAttribute(record.element, kAXRoleDescriptionAttribute as String),
            title: reader.stringAttribute(record.element, kAXTitleAttribute as String),
            label: reader.stringAttribute(record.element, kAXDescriptionAttribute as String),
            linkText: role == "AXLink" ? reader.descendantTextSnippets(record.element, limit: 2, maxDepth: 3).first : nil,
            url: reader.stringAttribute(record.element, kAXURLAttribute as String),
            rawActions: rawActions
        )
        guard elementSignature(node) == record.signature else {
            throw ProviderError.coded("element_not_found", "element \(record.index) changed since the last look; look again")
        }
        // A list can reuse one object for another row: the row around it says so.
        if let owner = record.contextOwner, let expected = record.context {
            let ownerReader = AXSnapshotReader()
            let ownerRole = ownerReader.stringAttribute(owner, kAXRoleAttribute as String)
            guard let ownerRole else { throw staleElement(record.index) }
            let ownerNode = SnapshotRenderNode(
                role: ownerRole,
                roleDescription: ownerReader.stringAttribute(owner, kAXRoleDescriptionAttribute as String),
                title: ownerReader.stringAttribute(owner, kAXTitleAttribute as String),
                label: ownerReader.stringAttribute(owner, kAXDescriptionAttribute as String),
                value: ownerReader.valueString(owner, role: ownerRole),
                rowSummary: ownerReader.rowTextSummary(owner, role: ownerRole)
            )
            guard SnapshotRenderHeuristics.displayName(ownerNode) == expected else {
                throw ProviderError.coded("element_not_found", "element \(record.index) now sits in another row than the last look showed; look again")
            }
        }
        return record.standing(actions: rawActions, frame: reader.frame(record.element, windowBounds: windowBounds))
    }

    private func staleElement(_ index: Int) -> ProviderError {
        .coded("element_not_found", "element \(index) is gone: the app removed it since the last look; look again and use a current number")
    }

    /// Hold the press until its control can take it: it may be turning on, or
    /// still moving. Waiting holds no turn on the hand, and the control is read
    /// again once the turn is held.
    private func waitUntilActionable(method: String, params: [String: JSONValue]) throws {
        guard ["click", "performSecondaryAction", "setValue"].contains(method),
              let index = try optionalInteger(params, "elementIndex"),
              let cached = try cachedSnapshot(params: params),
              !cached.app.app.isTerminated,
              let record = cached.elements[index] ?? knownRecord(pid: cached.app.pid, index: index)
        else { return }
        if method == "click" { waitForWebPress(record, pid: cached.app.pid) }
        let waitMs = min(try optionalInteger(params, "timeoutMs") ?? Actionability.defaultWaitMs, Actionability.longestWaitMs)
        let giveUp = Date().addingTimeInterval(TimeInterval(waitMs) / 1_000)
        let lastSeen = record.localFrame.map { $0.offsetBy(dx: cached.windowBounds.minX, dy: cached.windowBounds.minY) }
        var previous = lastSeen
        while true {
            var probe: CFTypeRef?
            switch AXUIElementCopyAttributeValue(record.element, kAXRoleAttribute as CFString, &probe) {
            case .success:
                break
            case .cannotComplete:
                throw ProviderError.coded("app_unresponsive", "the app is not answering; nothing was pressed")
            default:
                throw staleElement(index)
            }
            if OperatorGuardHost.isStopped() { return }
            let frame = absoluteFrame(record.element)
            let verdict = Actionability.of(
                enabled: boolAttribute(record.element, kAXEnabledAttribute as String),
                frame: frame,
                previousFrame: record.onSurface ? nil : previous,
                needsFrame: !record.onSurface
            )
            guard case let .wait(why) = verdict else { return }
            guard Date() < giveUp else {
                throw ProviderError.coded("not_actionable", "element \(index) \(why) after waiting \(waitMs) ms; nothing was pressed")
            }
            previous = frame
            usleep(useconds_t(Actionability.pollMs * 1_000))
        }
    }

    /// A web view offers a control's press only once its tree has settled, some seconds
    /// after it was switched on. Holds up this app's lane alone.
    private func waitForWebPress(_ record: ElementRecord, pid: pid_t) {
        cacheLock.lock()
        let on = webAccessibilityOnAt[pid]
        cacheLock.unlock()
        guard let on, BackgroundInput.pressableRoles.contains(record.role) else { return }
        let giveUp = on.addingTimeInterval(WebAccessibility.settlesSeconds)
        while Date() < giveUp, !OperatorGuardHost.isStopped(), !actions(record.element).contains("AXPress") {
            usleep(UInt32(Actionability.pollMs * 1_000))
        }
    }

    private func cachedSnapshot(params: [String: JSONValue]) throws -> Snapshot? {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        return try cachedSnapshotLocked(params: params)
    }

    private func cachedSnapshotLocked(params: [String: JSONValue]) throws -> Snapshot? {
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
        let requestedIndexes = try requestedNumbers(params)
        guard !requestedIndexes.isEmpty else { return }
        for index in requestedIndexes {
            // A number names one object, so the walk just made has it or it is gone.
            guard let actual = current.elements[index] ?? knownRecord(pid: current.app.pid, index: index) else {
                throw staleElement(index)
            }
            if let expected = cached?.elements[index], expected.signature != actual.signature {
                throw ProviderError.coded("element_not_found", "element \(index) changed since the last look; look again")
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
                guard pid > 0, !ShareControlPanel.isHelperPID(pid), pidIsLive(pid) else { return nil }
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

    /// The one app a word means, by the rule every verb acts by (`resolveApp`), for
    /// the window to hold one app under one key. An app that is not running is read
    /// off its bundle as a launch would find it, so what the person ended stays
    /// ended while it is closed. Blocked apps are named too: naming is no act.
    private func namedApp(_ query: String) -> [String: Any] {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        let running = parsePid(trimmed).flatMap(appByPid)
            ?? listApps().first { matches($0, query: trimmed) }
        if let running { return ["apps": [renderListedApp(running)], "named": true] }
        guard parsePid(trimmed) == nil,
              let url = try? DesktopApps.resolveApplicationURL(trimmed, running: []),
              let bundle = Bundle(url: url),
              let bundleId = bundle.bundleIdentifier
        else { return ["apps": [] as [Any], "named": true] }
        let row: [String: Any] = [
            "name": bundleName(info: bundle.infoDictionary) ?? bundleId,
            "bundleId": bundleId,
            "pid": NSNull(),
            "isRunning": false,
            "lastUsedAt": NSNull(),
            "useCount": NSNull(),
        ]
        return ["apps": [row], "named": true]
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
                    "shareControlPanel": true,
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
                    "background": true,
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
                    "menus": true,
                    "dialogs": true,
                    "dock": false,
                    "menubar": true,
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
            try ShareControlPanel.refuseHelper(pid: pid)
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
        if let id = params["windowId"]?.number.flatMap(UInt32.init(exactly:)), let owner = DesktopWindows.ownerPid(windowId: id) {
            try ShareControlPanel.refuseHelper(pid: owner)
            if !isTrustedZeroCodeApplication(owner) { return }
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
        try ShareControlPanel.refuseHelper(pid: app.pid)
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

    /// A hung app answers one read late and then gets no more of them: the walk
    /// would otherwise spend one timeout on every attribute of every element.
    private func requireAnswering(_ appElement: AXUIElement, app: AppDescriptor) throws {
        var role: CFTypeRef?
        if AXUIElementCopyAttributeValue(appElement, kAXRoleAttribute as CFString, &role) == .cannotComplete {
            throw ProviderError.coded("app_unresponsive", "\(app.name) is not answering; nothing was read or pressed. Other apps are not held up.")
        }
    }

    /// Chromium and Electron build their web tree only when asked. Whether an app
    /// is one of them is the app's to say: it offers this attribute as settable.
    /// A native app does not, and is left alone. Read back, because a set that
    /// returned success has not always taken.
    private func prepareWebAccessibility(_ appElement: AXUIElement, app: AppDescriptor) {
        cacheLock.lock()
        let first = webAccessibilityAsked.insert(app.pid).inserted
        cacheLock.unlock()
        guard first else { return }
        if isSettable(appElement, WebAccessibility.manualAttribute) {
            _ = AXUIElementSetAttributeValue(appElement, WebAccessibility.manualAttribute as CFString, kCFBooleanTrue)
            noteWebAccessibilityOn(app.pid)
            let giveUp = Date().addingTimeInterval(WebAccessibility.readySeconds)
            while boolAttribute(appElement, WebAccessibility.manualAttribute) != true, Date() < giveUp {
                usleep(WebAccessibility.pollMicros)
            }
            return
        }
        prepareApplicationAccessibility(appElement, app: app)
        if app.isKnownBrowser { noteWebAccessibilityOn(app.pid) }
    }

    private func noteWebAccessibilityOn(_ pid: pid_t) {
        cacheLock.lock()
        webAccessibilityOnAt[pid] = Date()
        cacheLock.unlock()
    }

    /// Where a look at a surface starts, or nil when it is not open.
    private func surfaceRoot(_ surface: String, appElement: AXUIElement, window: AXUIElement) -> AXUIElement? {
        switch surface {
        case "menubar":
            return copyElement(appElement, kAXMenuBarAttribute as String)
        case "menu":
            return openMenu(appElement)
        case "sheet":
            return (copyArray(window, kAXChildrenAttribute as String) ?? [])
                .first { stringAttribute($0, kAXRoleAttribute as String) == (kAXSheetRole as String) }
        default:
            return nil
        }
    }

    /// The menu open now: one a menu bar item opened, else a context menu.
    private func openMenu(_ appElement: AXUIElement) -> AXUIElement? {
        if let bar = copyElement(appElement, kAXMenuBarAttribute as String) {
            for item in copyArray(bar, kAXChildrenAttribute as String) ?? [] where boolAttribute(item, kAXSelectedAttribute as String) == true {
                if let menu = (copyArray(item, kAXChildrenAttribute as String) ?? [])
                    .first(where: { stringAttribute($0, kAXRoleAttribute as String) == (kAXMenuRole as String) }) {
                    return menu
                }
            }
        }
        return (copyArray(appElement, kAXChildrenAttribute as String) ?? [])
            .first { stringAttribute($0, kAXRoleAttribute as String) == (kAXMenuRole as String) }
    }

    /// What is open over the window now, which a look at the window does not show.
    private func openSurfaces(_ pid: pid_t) -> [[String: Any]] {
        let appElement = AXUIElementCreateApplication(pid)
        var open: [[String: Any]] = []
        if let menu = openMenu(appElement) {
            open.append(["kind": "menu", "title": jsonNullable(stringAttribute(menu, kAXTitleAttribute as String))])
        }
        if let window = copyElement(appElement, kAXFocusedWindowAttribute as String) {
            let subrole = stringAttribute(window, kAXSubroleAttribute as String)
            if subrole == (kAXDialogSubrole as String) || subrole == (kAXSystemDialogSubrole as String) {
                open.append(["kind": "dialog", "title": jsonNullable(stringAttribute(window, kAXTitleAttribute as String))])
            }
        }
        return open
    }

    /// How much of a window a look reads.
    private struct LookShape {
        /// A menu or the menu bar instead of the window.
        var surface: String?
        /// One element's subtree instead of the whole window.
        var root: Int?
        /// How deep to read; what is cut says how much it holds.
        var maxDepth: Int?

        init(_ params: [String: JSONValue]) throws {
            surface = params["surface"]?.string
            root = try optionalInteger(params, "root")
            maxDepth = try optionalInteger(params, "maxDepth")
            if params["skeleton"]?.bool == true, maxDepth == nil { maxDepth = LookShape.skeletonDepth }
        }

        /// A skeleton reads this deep: enough to name the regions of a dense app.
        static let skeletonDepth = 3
        /// A look's whole budget. A hung or huge app answers what was read by
        /// then, marked as cut short, instead of holding the request.
        static let walkBudgetSeconds: TimeInterval = 3
        var complete: Bool { root == nil && maxDepth == nil }
    }

    private func buildSnapshot(
        app: AppDescriptor,
        includeScreenshot: Bool,
        windowId: CGWindowID?,
        windowIndex: Int?,
        restoreWindow: Bool,
        readsTheTree: Bool,
        shape: LookShape
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
        try requireAnswering(appElement, app: app)
        prepareWebAccessibility(appElement, app: app)
        let listed = WindowCapture.candidates(pid: app.pid)
        // Without Screen Recording the window server names no other app's
        // window and may list none of them as shared (t-66753 C1): a look
        // then reads the app's own accessibility windows — that process's
        // alone, and never for an action, whose checks ask the server.
        let windowCandidates = readsTheTree
            ? WindowList.forReading(listed: listed, pid: app.pid, accessibility: { accessibilityWindows(of: appElement, app: app) })
            : listed
        let throughTheTree = listed.isEmpty && !windowCandidates.isEmpty
        let focused = try focusedWindow(
            appElement: appElement,
            app: app,
            visibleWindowCount: listed.count,
            allowRecovery: restoreWindow
        )
        let focusedTitle = stringAttribute(focused, kAXTitleAttribute as String) ?? app.name
        // A window the server does not list is no picture's: none is taken.
        let canCaptureScreenshot = includeScreenshot && !throughTheTree && screenCaptureTrustedSettled()
        guard let capture = WindowCapture.resolve(
            candidates: windowCandidates,
            titleHint: focusedTitle,
            windowId: windowId,
            windowIndex: windowIndex,
            captureImage: canCaptureScreenshot
        ) else {
            throw ProviderError.coded("window_not_found", "app '\(app.name)' has no on-screen window")
        }
        ShareControlPanel.observedWindow(pid: app.pid, windowID: capture.windowId)
        guard let window = matchingWindow(appElement: appElement, capture: capture, focused: focused, explicitTarget: windowId != nil || windowIndex != nil) else {
            throw ProviderError.coded("window_not_found", "could not match accessibility window to requested window; run get-app-state again or retry without a window selector")
        }
        let title = stringAttribute(window, kAXTitleAttribute as String) ?? capture.title ?? app.name
        let book = book(for: app.pid)
        let startsAt: AXUIElement
        let rootKey: String
        if let surface = shape.surface {
            guard let root = surfaceRoot(surface, appElement: appElement, window: window) else {
                throw ProviderError.coded("surface_not_open", "\(app.name) has no open \(surface); open it, then look again")
            }
            startsAt = root
            rootKey = "surface:\(surface)"
        } else if let rootIndex = shape.root {
            guard let record = knownRecord(pid: app.pid, index: rootIndex) else {
                throw ProviderError.coded("element_not_found", "element \(rootIndex) is gone; look at the window again")
            }
            startsAt = record.element
            rootKey = "window:\(capture.windowId)"
        } else {
            startsAt = window
            rootKey = "window:\(capture.windowId)"
        }
        // A web tree switched on moments ago may not stand yet: the whole
        // window's look reads again while it shows only its frame.
        cacheLock.lock()
        let switchedOnAt = shape.surface == nil && shape.root == nil ? webAccessibilityOnAt[app.pid] : nil
        cacheLock.unlock()
        let settled = WebTreeSettling.settle(
            switchedOnAt: switchedOnAt,
            now: { Date() },
            read: { () -> TreeRenderer in
                book.beginWalk()
                let renderer = TreeRenderer(
                    windowBounds: capture.bounds,
                    focused: focusedElement(appElement: appElement),
                    compactBrowserTabs: app.isKnownBrowser,
                    book: book,
                    rootKey: rootKey,
                    onSurface: shape.surface != nil,
                    maxDepth: shape.maxDepth,
                    deadline: Date().addingTimeInterval(LookShape.walkBudgetSeconds)
                )
                renderer.render(startsAt)
                let wholeWalk = shape.complete && !renderer.truncated
                fileRecords(pid: app.pid, records: renderer.records, gone: wholeWalk ? book.endCompleteWalk(root: rootKey) : book.endCutWalk())
                return renderer
            },
            frameOnly: { self.showsOnlyItsFrame($0) },
            pause: { usleep(WebAccessibility.pollMicros) },
            stopped: { OperatorGuardHost.isStopped() }
        )
        let renderer = settled.look
        let treeLines = settled.building ? renderer.lines + ["", WebTreeSettling.buildingLine(app: app.name)] : renderer.lines
        // What tells this window from another of its title (t-69973): its
        // own document — the one read a look adds — else the page its first
        // web area showed the walk above.
        let document = WindowDocument.identity(
            document: stringAttribute(window, kAXDocumentAttribute as String),
            webAreaURL: renderer.firstWebAreaURL
        )
        let screenshot = includeScreenshot ? capture.screenshotPayload() : nil
        let screenshotStatus: ScreenshotStatus = if screenshot != nil {
            .captured
        } else if includeScreenshot && throughTheTree {
            .failed(treeOnlyReason(app: app, presence: windowCandidates.first { $0.windowId == capture.windowId && $0.bounds == capture.bounds }?.presence ?? .unknown))
        } else if includeScreenshot && !canCaptureScreenshot {
            .failed("Screen Recording permission is required for ZeroCode Computer Use; grant permission or pass --no-screenshot to inspect accessibility state only.")
        } else if includeScreenshot {
            .failed("window screenshot capture returned no image; retry with --no-screenshot if accessibility state is sufficient.")
        } else {
            .skipped
        }
        var built = Snapshot(
            id: UUID().uuidString,
            app: app,
            windowTitle: title,
            windowBounds: capture.bounds,
            windowId: capture.windowId,
            windowLayer: capture.layer,
            document: document,
            treeText: renderTreeText(app: app, title: title, bounds: capture.bounds, lines: treeLines, focused: renderer.focusedSummary),
            focusedElementId: renderer.focusedElementId,
            screenshot: screenshot,
            screenshotStatus: screenshotStatus,
            screenshotScale: screenshotScale(screenshot: screenshot, bounds: capture.bounds),
            screenshotEngine: capture.image?.engine,
            elements: renderer.records,
            truncated: renderer.truncated,
            maxDepthReached: renderer.maxDepthReached
        )
        built.order = renderer.order
        built.rootIndex = renderer.order.first
        built.cutShort = settled.building ? WebTreeSettling.buildingCode : renderer.cutShort
        return built
    }

    /// Whether a look shows nothing of the window but its frame: every element
    /// under the window is one of its frame's controls or an empty container.
    private func showsOnlyItsFrame(_ renderer: TreeRenderer) -> Bool {
        let inside = renderer.order.dropFirst()
        guard inside.count <= WebTreeSettling.mostFrameParts else { return false }
        return WebTreeSettling.isFrameOnly(inside.compactMap { renderer.records[$0] }.map { record in
            WebTreeSettling.Part(
                role: record.role,
                subrole: stringAttribute(record.element, kAXSubroleAttribute as String),
                empty: record.name == nil && (copyArray(record.element, kAXChildrenAttribute as String) ?? []).isEmpty
            )
        })
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
                "window": placedWindow(snapshot),
                "coordinateSpace": "window",
                "treeText": snapshot.treeText,
                "elementCount": snapshot.elements.count,
                "focusedElementId": snapshot.focusedElementId as Any,
                "truncation": [
                    "truncated": snapshot.truncated,
                    "maxNodes": TreeRenderer.maxNodes,
                    "maxDepth": TreeRenderer.maxDepth,
                    "maxDepthReached": snapshot.maxDepthReached,
                    "cutShort": jsonNullable(snapshot.cutShort),
                ],
            ].merging(elementFrames ? ["elements": renderElementFaces(snapshot)] : [:]) { current, _ in current },
            "screenshot": screenshot,
            "screenshotStatus": renderScreenshotStatus(snapshot.screenshotStatus, snapshot: snapshot),
        ]
    }

    /// Every indexed element with a frame of positive area, by index: the
    /// faces the window plans a marked look from (the core's `ElementFace`).
    private func renderElementFaces(_ snapshot: Snapshot) -> [[String: Any]] {
        snapshot.order.compactMap { index in
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
            if let plainInput = record.plainInput { face["plainInput"] = plainInput }
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
        if params["background"]?.bool == true {
            try admitBackgroundTarget(snapshot)
            if let requested = params["ocrPin"] {
                try validatePixelTarget(requested, snapshot: snapshot)
            }
            let query = readingQuery(params)
            let index = try optionalInteger(params, "elementIndex")
                ?? (query.isEmpty ? nil : chosenIndex(query, in: snapshot))
            let requestedPoint = try index == nil ? coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot) : nil
            let record = try index.map { try element(snapshot, $0) }
                ?? requestedPoint.flatMap { backgroundClickRecord(at: $0, in: snapshot) }
            if let record,
               let action = BackgroundInput.clickAction(
                   actions: record.actions,
                   textEntry: MarkPin.textEntryRoles.contains(record.role),
                   plainLeftClick: modifiers.isEmpty && count == 1 && button == .left
               ) {
                try requireReachable(record, in: snapshot, at: requestedPoint)
                var shown: CGPoint?
                if let point = requestedPoint ?? center(record.visible ?? record.localFrame, in: snapshot.windowBounds) {
                    shown = try travel(to: point, in: snapshot)
                }
                guard try performActionChecked(record.element, action) else {
                    throw ProviderError.coded("accessibility_error", "the background semantic action failed; no fallback was attempted")
                }
                BackgroundPointerOverlay.pressed(at: shown, agent: RequestContext.current.agent)
                return actionMetadata(path: "accessibility", actionName: action)
            }
            // Pressing a field is asking for its focus, and a web view ignores events
            // sent to a window that is not key.
            let pressesAField = record.map { modifiers.isEmpty && count == 1 && button == .left && MarkPin.textEntryRoles.contains($0.role) } ?? false
            var shownAtField: CGPoint?
            if pressesAField, let record,
               let point = requestedPoint ?? center(record.visible ?? record.localFrame, in: snapshot.windowBounds) {
                shownAtField = try travel(to: point, in: snapshot)
            }
            if pressesAField, let record,
               AXUIElementSetAttributeValue(record.element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success {
                try requireReachable(record, in: snapshot, at: requestedPoint)
                BackgroundPointerOverlay.pressed(at: shownAtField, agent: RequestContext.current.agent)
                return actionMetadata(path: "accessibility", actionName: "AXFocused")
            }
            let point: CGPoint
            if let requestedPoint {
                point = requestedPoint
            } else if let record {
                guard let center = center(record.visible ?? record.localFrame, in: snapshot.windowBounds) else {
                    throw ProviderError.coded("element_not_clickable", "the background control has no visible frame")
                }
                point = center
            } else {
                point = try coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot)
            }
            let shown = try travel(to: point, in: snapshot)
            try Input.windowClick(at: point, button: button, count: count, modifiers: modifiers, snapshot: snapshot) {
                try self.requireBackgroundWindow(snapshot, at: point)
                if let record { try self.requireOnTop(record, in: snapshot, background: true, at: point) }
            }
            BackgroundPointerOverlay.pressed(at: shown, agent: RequestContext.current.agent)
            return actionMetadata(path: "window", verification: unverifiedAction(reason: "synthetic_input"))
        }
        // Why: agents expect a click into a target app to make the next
        // keyboard action safe, even when the click uses an AX action path.
        recoverWindow(snapshot.app, windowId: snapshot.windowId, windowBounds: snapshot.windowBounds)
        if let requested = params["ocrPin"] {
            try validatePixelTarget(requested, snapshot: snapshot)
        }
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
                let context = RequestContext.current
                let sentBefore = context.sent
                try context.askThePersonIfFirst()
                context.sent = true
                if AXUIElementSetAttributeValue(record.element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success {
                    return actionMetadata(path: "accessibility", actionName: "AXFocused")
                }
                context.sent = sentBefore
                if try performActionChecked(record.element, "AXPress") {
                    return actionMetadata(path: "accessibility", actionName: "AXPress")
                }
            } else if plain,
               button.hasAccessibilityAction,
               let actionName = try performClickAction(record: record, mouseButton: button) {
                return actionMetadata(path: "accessibility", actionName: actionName)
            }
            if let point = center(record.localFrame, in: snapshot.windowBounds) {
                // A control this walk did not reach is pressed at its place only
                // if that is what a pointer would hit there.
                if snapshot.elements[elementIndex] == nil { try requireOnTop(record, in: snapshot) }
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
        let faces = snapshot.order.compactMap { index -> ElementFace? in
            guard let record = snapshot.elements[index] else { return nil }
            return ElementFace(index: index, role: record.role, label: record.title, value: record.value, description: record.label)
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

    /// The control can be pressed by its accessibility action: it is on top where
    /// it stands, or only scrolled out of its list with nothing over the list. A
    /// person would scroll to it; its own action needs no pointer, so it asks the
    /// list to bring it into view and presses it either way. A control under a
    /// dialog or a menu is refused, as it would be for a person.
    private func requireReachable(_ record: ElementRecord, in snapshot: Snapshot, at requestedPoint: CGPoint? = nil) throws {
        if record.onSurface { return }
        do {
            try requireOnTop(record, in: snapshot, background: true, at: requestedPoint)
            return
        } catch let covered as ProviderError {
            guard requestedPoint == nil, let list = scrollAreaHiding(record, in: snapshot) else { throw covered }
            if record.actions.contains("AXScrollToVisible") {
                _ = AXUIElementPerformAction(record.element, "AXScrollToVisible" as CFString)
                if let frame = reader(frameOf: record.element, windowBounds: snapshot.windowBounds),
                   (try? requireOnTop(record.standing(actions: record.actions, frame: frame), in: snapshot, background: true)) != nil {
                    return
                }
            }
            guard let listFrame = absoluteFrame(list) else { throw covered }
            var hit: AXUIElement?
            let scope = AXUIElementCreateApplication(snapshot.app.pid)
            guard AXUIElementCopyElementAtPosition(scope, Float(listFrame.midX), Float(listFrame.midY), &hit) == .success,
                  let hit, reaches(hit, list) || reaches(list, hit)
            else { throw covered }
        }
    }

    private func reader(frameOf element: AXUIElement, windowBounds: CGRect) -> CGRect? {
        AXSnapshotReader().frame(element, windowBounds: windowBounds)
    }

    /// The list that keeps `record` out of view: the nearest scroll area above it
    /// whose frame does not hold its centre. Nil when it is in view, so something
    /// else is covering it.
    private func scrollAreaHiding(_ record: ElementRecord, in snapshot: Snapshot) -> AXUIElement? {
        guard let point = center(record.localFrame, in: snapshot.windowBounds) else { return nil }
        var node = copyElement(record.element, kAXParentAttribute as String)
        for _ in 0..<MarkPin.hitDepth {
            guard let current = node else { return nil }
            if MarkPin.clipRoles.contains(stringAttribute(current, kAXRoleAttribute as String) ?? ""),
               let frame = absoluteFrame(current), !frame.contains(point) {
                return current
            }
            node = copyElement(current, kAXParentAttribute as String)
        }
        return nil
    }

    /// What is on top at a marked control's centre must be the control, inside
    /// it, or around it: a mark never presses, or points at, something a
    /// person could not hit there (a row scrolled under a header, a control
    /// under a menu or an in-page dialog).
    /// A region's own content is never over it (`MarkPin.Reach.region`): a
    /// scroll lands on whatever its list shows at its centre.
    private func requireOnTop(_ record: ElementRecord, in snapshot: Snapshot, background: Bool = false, at requestedPoint: CGPoint? = nil, reach: MarkPin.Reach = .press) throws {
        guard let point = requestedPoint ?? center(record.localFrame, in: snapshot.windowBounds) else {
            throw ProviderError.coded("element_not_clickable", "element \(record.index) has no clickable frame")
        }
        var hit: AXUIElement?
        let scope = background ? AXUIElementCreateApplication(snapshot.app.pid) : AXUIElementCreateSystemWide()
        guard AXUIElementCopyElementAtPosition(scope, Float(point.x), Float(point.y), &hit) == .success,
              let hit
        else {
            throw ProviderError.coded("element_not_found", "nothing answers at the centre of element \(record.index); look again with --marks")
        }
        if MarkPin.onTop(hit: hit, target: record.element, reach: reach, parent: { copyElement($0, kAXParentAttribute as String) }, same: { CFEqual($0, $1) }) {
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
        MarkPin.reaches(from, to, within: MarkPin.hitDepth, parent: { copyElement($0, kAXParentAttribute as String) }, same: { CFEqual($0, $1) })
    }

    private func backgroundClickRecord(at point: CGPoint, in snapshot: Snapshot) -> ElementRecord? {
        var hit: AXUIElement?
        let application = AXUIElementCreateApplication(snapshot.app.pid)
        guard AXUIElementCopyElementAtPosition(application, Float(point.x), Float(point.y), &hit) == .success else { return nil }
        var fallback: ElementRecord?
        for _ in 0..<MarkPin.hitDepth {
            guard let current = hit else { break }
            if let record = snapshot.elements.values.first(where: { CFEqual($0.element, current) }) {
                if fallback == nil { fallback = record }
                if MarkPin.textEntryRoles.contains(record.role)
                    || BackgroundInput.clickAction(actions: record.actions, textEntry: false, plainLeftClick: true) != nil {
                    return record
                }
            }
            hit = copyElement(current, kAXParentAttribute as String)
        }
        return fallback
    }

    private func performClickAction(record: ElementRecord, mouseButton: MouseButtonSelection) throws -> String? {
        if mouseButton == .right {
            return try performActionChecked(record.element, "AXShowMenu") ? "AXShowMenu" : nil
        }
        for action in ["AXPress", "AXConfirm", "AXOpen"] {
            if try performActionChecked(record.element, action) {
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
        guard try performActionChecked(record.element, action) else {
            throw ProviderError.coded("accessibility_error", "AXUIElementPerformAction(\(action)) failed")
        }
        return actionMetadata(path: "accessibility", actionName: action)
    }

    private func setValue(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentSnapshot(params: params)
        let record = try element(snapshot, try requiredInteger(params, "elementIndex"))
        if params["background"]?.bool == true {
            try admitBackgroundTarget(snapshot)
            try requireReachable(record, in: snapshot)
        }
        let expected = try requiredStringAllowingEmpty(params, "value")
        if params["plainInputOnly"]?.bool == true,
           record.plainInput == nil || record.plainInput != params["expectedPlainValue"]?.string || looksLikeSecret(record.element) {
            throw ProviderError.coded("value_not_settable", "the marked control is no longer a readable, non-secret text field")
        }
        guard isSettable(record.element, kAXValueAttribute as String) else {
            throw ProviderError.coded("value_not_settable", "element \(record.index) is not settable")
        }
        let current = rawAttributeValue(record.element, kAXValueAttribute as String)
        let coercion = AttributeValueCoercion(existingValue: current, requested: expected)
        try RequestContext.current.askThePersonIfFirst()
        RequestContext.current.sent = true
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
        if snapshot.app.bundleId == PhoneTyping.mirroringBundleId {
            return try typeOnMirroredPhone(text, snapshot: snapshot, params: params)
        }
        if params["background"]?.bool == true {
            return try backgroundTypeText(text, snapshot: snapshot, config: keyboardGuardConfig(params))
        }
        let focused = focusedRecord(snapshot)?.element
        let secureInput = try gateSecretEntry(writesText: true, pastes: false, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        if let focused, case .applied(let verification) = try TextInput.replaceSelection(focused, with: text) {
            return withSecureInput(actionMetadata(path: "accessibility", actionName: "AXReplaceSelection", verification: verification), secureInput)
        }
        try requireTargetWindowFocused(snapshot, restoreWindowRequested: params["restoreWindow"]?.bool == true)
        let (verification, typedSecureInput) = try typeWatched(text, pid: snapshot.app.pid, receiver: snapshot.app.pid, config: keyboardGuardConfig(params))
        return withSecureInput(actionMetadata(path: "synthetic", actionName: "typeText", verification: verification), typedSecureInput)
    }

    private func pressKey(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentKeyboardSnapshot(params: params)
        let key = try requiredString(params, "key")
        if snapshot.app.bundleId == PhoneTyping.mirroringBundleId {
            return try pressOnMirroredPhone(key, snapshot: snapshot, params: params)
        }
        if params["background"]?.bool == true {
            return try backgroundKey(key, snapshot: snapshot)
        }
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
        if params["background"]?.bool == true {
            return try backgroundKey(key, snapshot: snapshot)
        }
        let writes = KeyboardInputSafety.chordWrites(key)
        _ = try gateSecretEntry(writesText: writes == .character, pastes: writes == .clipboard, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        if isSelectAllHotkey(key), let focused = focusedRecord(snapshot) {
            let context = RequestContext.current
            let sentBefore = context.sent
            try context.askThePersonIfFirst()
            context.sent = true
            if TextInput.selectAll(focused.element) {
                return actionMetadata(
                    path: "accessibility",
                    actionName: "AXSelectAll",
                    verification: TextInput.selectionVerification(focused.element)
                )
            }
            context.sent = sentBefore
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
        if params["background"]?.bool == true {
            return try backgroundTypeText(text, snapshot: snapshot, config: keyboardGuardConfig(params), pasting: true)
        }
        let focused = focusedRecord(snapshot)?.element
        _ = try gateSecretEntry(writesText: true, pastes: false, focused: keyTargets(snapshot), receiver: snapshot.app.pid)
        if let focused, case .applied(let verification) = try TextInput.replaceSelection(focused, with: text) {
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
            // Name what made it secret: a field the focus is on, or another
            // app holding the keyboard's secure mode while the focus here
            // could not be read (2026-11-02: Chrome's password field was
            // reported as iPhone Mirroring's).
            let holdsIt = !focused.contains { isSecretField($0) } && state.on
                && state.holderPid != nil && state.holderPid != receiver
            // `secret_field: <app>` is what the window turns into the model's
            // sentence (`validate::secure_input_sentence`): the app named is
            // the one whose password field it is.
            let app = holdsIt ? (state.holder ?? "another app") : focusedAppName(focused.first)
            var message = "secret_field: \(app)"
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
    private func typeWatched(_ text: String, pid: pid_t, receiver: pid_t?, config: KeyboardGuardConfig?, background: Snapshot? = nil) throws -> ([String: Any], SecureInputState) {
        func readFocus() -> AXUIElement? {
            if case .some = background { return appFocusedElement(pid) }
            return systemWideFocusedElement()
        }
        var focus = readFocus()
        if let background { try requireBackgroundFocus(focus, in: background) }
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
                focus = settledFocus(after: focus, config: config, readFocus: readFocus)
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
            if let background {
                let expectedFocus = focus
                try Input.windowText(piece, snapshot: background) {
                    try self.requireBackgroundWindow(background)
                    let current = readFocus()
                    try self.requireBackgroundFocus(current, in: background)
                    guard let expectedFocus, let current, CFEqual(expectedFocus, current) else {
                        throw ProviderError.coded("focus_changed", "background text stopped because its field changed; observe before retrying")
                    }
                    _ = try self.gateSecretEntry(writesText: true, pastes: false, focused: [current], receiver: pid)
                }
            } else {
                try Input.typeText(piece, pid: pid)
            }
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
    private func settledFocus(after before: AXUIElement?, config: KeyboardGuardConfig, readFocus: () -> AXUIElement? = systemWideFocusedElement) -> AXUIElement? {
        var waited = 0
        var last: AXUIElement?
        while waited < config.settleMs {
            usleep(useconds_t(config.pollMs * 1_000))
            waited += config.pollMs
            guard let now = readFocus(), !(before.map { CFEqual($0, now) } ?? false) else {
                last = nil
                continue
            }
            if let last, CFEqual(last, now) {
                return now
            }
            last = now
        }
        return readFocus()
    }

    private func scroll(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentSnapshot(params: params)
        let direction = try scrollDirection(try requiredString(params, "direction"))
        let pages = try positiveNumber(params["pages"]?.number, defaultValue: 1, name: "pages")
        if params["background"]?.bool == true {
            return try backgroundScroll(params: params, snapshot: snapshot, direction: direction, pages: pages)
        }
        if let elementIndex = try optionalInteger(params, "elementIndex") {
            let record = try element(snapshot, elementIndex)
            let action = "AXScroll\(direction.capitalized)ByPage"
            if pages.rounded() == pages, let pageCount = boundedInteger(pages, as: Int.self),
               record.actions.contains(action) {
                try RequestContext.current.askThePersonIfFirst()
                RequestContext.current.sent = true
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

    private func backgroundUnavailable() -> ProviderError {
        .coded("requires_foreground", "this control has no supported background path; no desktop fallback was attempted")
    }

    private func admitBackgroundTarget(_ snapshot: Snapshot) throws {
        guard BackgroundInput.admitsTarget(snapshot.app.pid, foreground: NSWorkspace.shared.frontmostApplication?.processIdentifier) else {
            throw ProviderError.coded("requires_foreground", "background input pauses while the person is using the target app or the foreground app is unknown")
        }
    }

    private func requireBackgroundWindow(_ snapshot: Snapshot, at point: CGPoint? = nil) throws {
        try admitBackgroundTarget(snapshot)
        guard let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] else {
            throw ProviderError.coded("window_stale", "background window ownership is unavailable")
        }
        for row in rows where row[kCGWindowOwnerPID as String] as? Int32 == snapshot.app.pid {
            guard let bounds = row[kCGWindowBounds as String] as? [String: Any],
                  let frame = CGRect(dictionaryRepresentation: bounds as CFDictionary)
            else { throw ProviderError.coded("window_stale", "a background window has unreadable bounds") }
            if row[kCGWindowNumber as String] as? UInt32 == snapshot.windowId {
                guard frame == snapshot.windowBounds else {
                    throw ProviderError.coded("window_stale", "the background window moved or resized; observe before retrying")
                }
                return
            }
            if let point, frame.contains(point), (row[kCGWindowAlpha as String] as? Double ?? 1) > 0 {
                throw ProviderError.coded("element_not_found", "another window of the target app covers the background point")
            }
        }
        throw ProviderError.coded("window_stale", "the background target window is no longer available")
    }

    private func requireBackgroundFocus(_ focus: AXUIElement?, in snapshot: Snapshot) throws {
        guard let focus, let root = snapshot.rootIndex.flatMap({ snapshot.elements[$0]?.element }),
              copyElement(focus, kAXWindowAttribute as String).map({ CFEqual($0, root) }) == true || reaches(focus, root)
        else {
            throw ProviderError.coded("focus_changed", "the background keyboard receiver cannot be verified in the target window")
        }
    }

    private func backgroundFocusedElement(_ snapshot: Snapshot) -> AXUIElement? {
        let actual = appFocusedElement(snapshot.app.pid)
        let candidates = snapshot.elements.values.lazy.compactMap { record -> Int? in
            guard MarkPin.textEntryRoles.contains(record.role),
                  actual.map({ CFEqual($0, record.element) }) == true
                    || boolAttribute(record.element, kAXFocusedAttribute as String) == true
            else { return nil }
            return record.index
        }
        switch BackgroundInput.focus(among: candidates) {
        case let .one(index): return snapshot.elements[index]?.element
        case .ambiguous: return nil
        case .none:
            return (try? requireBackgroundFocus(actual, in: snapshot)) != nil ? actual : nil
        }
    }

    private func requireBackgroundField(_ focus: AXUIElement, in snapshot: Snapshot) throws {
        guard let record = snapshot.elements.values.first(where: { CFEqual($0.element, focus) }) else {
            throw ProviderError.coded("focus_changed", "the background field is not in the current observation")
        }
        try requireOnTop(record, in: snapshot, background: true)
    }

    private func backgroundTypeText(_ text: String, snapshot: Snapshot, config: KeyboardGuardConfig?, pasting: Bool = false) throws -> [String: Any] {
        try requireBackgroundWindow(snapshot)
        let focus = backgroundFocusedElement(snapshot)
        try requireBackgroundFocus(focus, in: snapshot)
        guard let focus, MarkPin.textEntryRoles.contains(rawStringAttribute(focus, kAXRoleAttribute as String) ?? "") else {
            throw ProviderError.coded("focus_changed", "background text requires a readable text field in its target window")
        }
        try requireBackgroundField(focus, in: snapshot)
        if pasting || isMultiLineText(focus) || !KeyboardInputSafety.movesTheFocus(text), isSettable(focus, kAXValueAttribute as String) {
            let state = try gateSecretEntry(writesText: true, pastes: false, focused: [focus], receiver: snapshot.app.pid)
            switch try TextInput.replaceSelection(focus, with: text) {
            case let .applied(verification):
                return withSecureInput(actionMetadata(path: "accessibility", actionName: "AXReplaceSelection", verification: verification), state)
            case .unavailable:
                throw ProviderError.coded("value_not_settable", "background text replacement failed; no synthetic retry was attempted")
            }
        }
        if pasting && KeyboardInputSafety.containsControlCharacters(text) {
            throw ProviderError.coded("requires_foreground", "background paste with control characters requires a field-specific value writer; no keys were sent")
        }
        try requireBackgroundKeyboard(snapshot, expected: focus)
        let (verification, state) = try typeWatched(text, pid: snapshot.app.pid, receiver: snapshot.app.pid, config: config, background: snapshot)
        return withSecureInput(actionMetadata(path: "window", actionName: "typeText", verification: verification), state)
    }

    private func requireBackgroundKeyboard(_ snapshot: Snapshot, expected: AXUIElement) throws {
        try requireBackgroundWindow(snapshot)
        let actual = appFocusedElement(snapshot.app.pid)
        try requireBackgroundFocus(actual, in: snapshot)
        guard let actual, CFEqual(actual, expected) else {
            throw ProviderError.coded("focus_changed", "the app's native keyboard receiver is not the selected background control")
        }
    }

    private func backgroundKey(_ key: String, snapshot: Snapshot) throws -> [String: Any] {
        guard !KeyboardInputSafety.usesClipboard(key) else {
            throw ProviderError.coded("requires_foreground", "background keys never use the shared clipboard; pass text directly instead")
        }
        try requireBackgroundWindow(snapshot)
        let focus = backgroundFocusedElement(snapshot)
        try requireBackgroundFocus(focus, in: snapshot)
        guard let focus else { throw backgroundUnavailable() }
        if isSelectAllHotkey(key), !looksLikeSecret(focus) {
            try requireBackgroundField(focus, in: snapshot)
            if TextInput.selectAll(focus) {
                return actionMetadata(path: "accessibility", actionName: "AXSelectAll", verification: TextInput.selectionVerification(focus))
            }
        }
        try requireBackgroundKeyboard(snapshot, expected: focus)
        let writes = KeyboardInputSafety.chordWrites(key)
        var state = SecureInputState.now()
        try Input.windowKey(key, snapshot: snapshot) {
            try self.requireBackgroundKeyboard(snapshot, expected: focus)
            state = try self.gateSecretEntry(writesText: writes == .character, pastes: false, focused: [focus], receiver: snapshot.app.pid)
        }
        return withSecureInput(actionMetadata(path: "window", actionName: "pressKey", verification: unverifiedAction(reason: "synthetic_input")), state)
    }

    private func backgroundScroll(params: [String: JSONValue], snapshot: Snapshot, direction: String, pages: Double) throws -> [String: Any] {
        try admitBackgroundTarget(snapshot)
        let action = "AXScroll\(direction.capitalized)ByPage"
        let index: Int?
        let point: CGPoint
        if let requested = try optionalInteger(params, "elementIndex") {
            index = requested
            let record = try element(snapshot, requested)
            guard let visiblePoint = center(record.visible ?? record.localFrame, in: snapshot.windowBounds) else {
                throw backgroundUnavailable()
            }
            point = visiblePoint
        } else {
            point = try coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot)
            let local = CGPoint(x: point.x - snapshot.windowBounds.minX, y: point.y - snapshot.windowBounds.minY)
            let candidates = snapshot.elements.values.compactMap { record -> BackgroundInput.Scroller? in
                guard record.actions.contains(action), let frame = record.localFrame else { return nil }
                return BackgroundInput.Scroller(index: record.index, frame: record.visible.map { frame.intersection($0) } ?? frame)
            }
            index = BackgroundInput.scrollTarget(at: local, among: candidates)
        }
        let record = try index.map { try element(snapshot, $0) }
        if let record {
            try requireOnTop(record, in: snapshot, background: true, at: point, reach: .region)
            if pages == 1 && record.actions.contains(action) {
                try travel(to: point, in: snapshot)
                guard performAction(record.element, action) else {
                    throw ProviderError.coded("accessibility_error", "the background scroll failed; no fallback was attempted")
                }
                return actionMetadata(path: "accessibility", actionName: action)
            }
        }
        let viewport = record?.visible ?? record?.localFrame ?? snapshot.windowBounds
        let extent = direction == "up" || direction == "down" ? viewport.height : viewport.width
        guard let delta = boundedInteger(max(1, (extent * pages).rounded()), as: Int32.self) else {
            throw ProviderError.coded("invalid_argument", "pages is out of range")
        }
        try requireBackgroundWindow(snapshot, at: point)
        try travel(to: point, in: snapshot)
        // The window may have moved or closed while the pointer travelled.
        try requireBackgroundWindow(snapshot, at: point)
        try Input.windowScroll(at: point, direction: direction, delta: delta, snapshot: snapshot)
        return actionMetadata(path: "window", verification: unverifiedAction(reason: "synthetic_input"))
    }

    private func drag(params: [String: JSONValue]) throws -> [String: Any] {
        let snapshot = try currentSnapshot(params: params)
        let start: CGPoint
        let end: CGPoint
        if let fromIndex = try optionalInteger(params, "fromElementIndex"),
           let toIndex = try optionalInteger(params, "toElementIndex") {
            let from = try element(snapshot, fromIndex)
            let to = try element(snapshot, toIndex)
            if snapshot.elements[fromIndex] == nil { try requireOnTop(from, in: snapshot) }
            if snapshot.elements[toIndex] == nil { try requireOnTop(to, in: snapshot) }
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
        if let record = snapshot.elements[index] { return record }
        guard let known = knownRecord(pid: snapshot.app.pid, index: index) else { throw staleElement(index) }
        // This walk did not reach it: cut short, or another window. The table's
        // frame is from an older look, so it is read from the element itself.
        return try liveRecord(known, windowBounds: snapshot.windowBounds)
    }

    private func focusedRecord(_ snapshot: Snapshot) -> ElementRecord? {
        guard let focusedElementId = snapshot.focusedElementId else {
            return nil
        }
        return snapshot.elements[focusedElementId]
    }
}

// MARK: - A mirrored phone (`PhoneTyping`)

extension Provider {
    /// Text for a mirrored iPhone goes as the keys of the phone's own keyboard
    /// to the mirroring app's process: no borrow, its window need not be in
    /// front, and the person's pointer, front app, keyboard and clipboard are
    /// not used. The keyboard's language is read by a two-key probe that is
    /// deleted again and moved with the chords that work on a mirrored phone;
    /// what the screen shows afterwards is read back, not retried.
    fileprivate func typeOnMirroredPhone(_ text: String, snapshot: Snapshot, params: [String: JSONValue]) throws -> [String: Any] {
        let runs: [PhoneRun]
        switch PhoneTyping.plan(text) {
        case let .success(planned):
            runs = planned
        case let .failure(refusal):
            throw ProviderError.coded("invalid_argument", MirroredPhoneWords.refusal(refusal))
        }
        try admitMirroredPhone(snapshot, params: params)
        var reading = params
        reading["ocr"] = .bool(true)
        reading["background"] = nil
        let read: () throws -> [String] = { try self.linesToRead(params: reading).lines.map(\.text) }
        let keys = MirroredPhoneHand(pid: snapshot.app.pid)
        let first = try read()
        // A field the phone hides is the person's to type, as on the desktop (B3).
        if PhoneTyping.showsHiddenText(first) {
            throw ProviderError.coded("secure_input", "secret_field: \(snapshot.app.name); the phone shows a hidden field (dots), nothing was typed")
        }
        var state: PhoneKeyboardState?
        var chords: [String] = []
        /// Two keys typed and deleted again, whatever happens between. The first
        /// screen stands for the first probe: nothing has been typed since it.
        var untouched: [String]? = first
        func probe() throws -> PhoneKeyboardState? {
            let shown = untouched
            untouched = nil
            let hand = PhoneTyping.Probe(
                read: read,
                type: { try keys.type($0) },
                deleteOne: { try keys.deleteOne() },
                settle: { try OperatorHandHost.sleep(nanoseconds: MirroredPhoneHand.settleNanoseconds) }
            )
            do {
                return try hand.run(shown: shown)
            } catch PhoneTyping.Probe.Failure.nothingShown {
                throw ProviderError.coded("phone_keyboard_unknown", "the two language probe keys (gk) showed nothing on the screen: a field that takes no letters, a hidden or a full one, or one too small to read. Nothing was deleted, so if it did take them they are still there, and nothing else was typed. Look at the field before typing again; a small search field takes press-key letter by letter")
            } catch let PhoneTyping.Probe.Failure.left(why) {
                throw ProviderError.coded("phone_probe_left", "the language probe was not taken back: \(why). Up to two letters (gk, 하 or ㅎㅏ) may be left in the field, and nothing else was typed. Look before typing again")
            }
        }
        for run in runs {
            if let language = run.language {
                if state == nil {
                    guard let found = try probe() else {
                        throw ProviderError.coded("phone_keyboard_unknown", "the phone's keyboard language could not be read: the screen changed but did not show the two probe keys, as in a small search field. Both were deleted and nothing else was typed. Type with press-key letter by letter and read the screen, or hand it to the person")
                    }
                    state = found
                }
                let wanted: PhoneKeyboardState = language == .korean ? .korean : .english
                for chord in PhoneTyping.route(from: state!, to: language) {
                    try keys.switchLanguage(chord)
                    chords.append(chord == .capsLock ? "capsLock" : "controlSpace")
                    state = state.flatMap { PhoneTyping.after(chord, from: $0) }
                }
                if !chords.isEmpty, state == wanted {
                    // A chord is believed only once the phone shows it: Caps Lock
                    // locks capitals instead where its language switch is off.
                    let shown = try probe()
                    if shown != wanted {
                        if chords.last == "capsLock" { try keys.switchLanguage(.capsLock) }
                        throw ProviderError.coded("phone_keyboard_unknown", "the phone did not change to \(language.rawValue) (it shows \(shown.map { "\($0)" } ?? "nothing readable")); \(chords.last == "capsLock" ? "Caps Lock was pressed again to undo it" : "nothing more was sent"). Its Caps Lock language switch may be off: hand it to the person")
                    }
                    state = shown
                }
            }
            try keys.type(run.keys)
        }
        try OperatorHandHost.sleep(nanoseconds: MirroredPhoneHand.settleNanoseconds)
        let shown = (try? read()) ?? []
        let seen = PhoneTyping.newlyShows(text, before: first, after: shown)
        var answer = actionMetadata(
            path: "process",
            actionName: "phoneKeys",
            verification: seen
                ? verifiedAction(property: "ocr", expected: text, actualPreview: text)
                : unverifiedAction(reason: "phone_text_not_read", expected: text, actualPreview: shown.last)
        )
        answer["phoneKeyboard"] = ["chords": chords, "language": state.map { "\($0)" } as Any]
        return answer
    }

    /// A key or a chord for a mirrored iPhone, to the mirroring app's process.
    fileprivate func pressOnMirroredPhone(_ key: String, snapshot: Snapshot, params: [String: JSONValue]) throws -> [String: Any] {
        guard !KeyboardInputSafety.usesClipboard(key) else {
            throw ProviderError.coded("requires_foreground", "keys for a mirrored phone never use the shared clipboard; pass text to type-text instead")
        }
        try admitMirroredPhone(snapshot, params: params)
        let stroke = try KeyMap.parse(key).stroke
        let route = HandRoute.process(snapshot.app.pid)
        let ups = stroke.ups(route: route)
        defer {
            for modifier in ups.dropFirst() { try? OperatorHandHost.post(modifier) }
        }
        for press in stroke.downs(route: route) { try OperatorHandHost.post(press) }
        try OperatorHandHost.post(ups[0])
        return actionMetadata(path: "process", actionName: "phoneKey", verification: unverifiedAction(reason: "synthetic_input"))
    }

    /// Keys go only to the mirroring app, so another app's secure mode is not
    /// theirs to read; the mirroring app's own is. In the background the person
    /// using that app comes first; a call that said --foreground has already
    /// waited for them on the desktop's road.
    private func admitMirroredPhone(_ snapshot: Snapshot, params: [String: JSONValue]) throws {
        if params["background"]?.bool == true { try admitBackgroundTarget(snapshot) }
        let secure = SecureInputState.now()
        if secure.on, secure.holderPid == snapshot.app.pid {
            throw ProviderError.coded("secure_input", "secret_field: \(snapshot.app.name); it holds the keyboard's secure mode")
        }
    }
}

/// The keys of a mirrored phone, posted to the mirroring app's process.
private struct MirroredPhoneHand {
    let pid: pid_t
    /// Between two keys, as a person's fast typing.
    static let keyGapNanoseconds: UInt64 = 20_000_000
    /// Between two deletes (see `deleteOne`).
    static let deleteGapNanoseconds: UInt64 = 120_000_000
    /// For the phone to draw what the keys typed.
    static let settleNanoseconds: UInt64 = 350_000_000
    /// After a chord, before the next key: a switch commits late (simulator: past 0.5 s for Control-Space).
    static let capsLockSettleNanoseconds: UInt64 = 700_000_000
    static let controlSpaceSettleNanoseconds: UInt64 = 900_000_000
    static let deleteCode: UInt16 = 51
    static let shiftCode: UInt16 = 56
    static let controlCode: UInt16 = 59
    static let spaceCode: UInt16 = 49

    private var route: HandRoute { .process(pid) }

    func type(_ keys: [PhoneKey]) throws {
        let shift = CGEventFlags.maskShift.rawValue
        for key in keys {
            let flags = key.shift ? shift : 0
            if key.shift {
                try OperatorHandHost.post(HandEvent(.key(code: Self.shiftCode, down: true, modifier: shift), flags: shift, route: route))
            }
            try OperatorHandHost.post(HandEvent(.key(code: key.code, down: true, modifier: 0), flags: flags, route: route))
            try OperatorHandHost.post(HandEvent(.key(code: key.code, down: false, modifier: 0), flags: flags, route: route))
            if key.shift {
                try OperatorHandHost.post(HandEvent(.key(code: Self.shiftCode, down: false, modifier: 0), route: route))
            }
            try OperatorHandHost.sleep(nanoseconds: Self.keyGapNanoseconds)
        }
    }

    /// One delete and a pause as long as a person's repeat: the same key twice at
    /// a typing gap is what could land as one. A probe's left-over `g` was seen
    /// once (2026-11-03, real phone) and its cause not told apart, so the probe
    /// reads the screen afterwards as well.
    func deleteOne() throws {
        try type([PhoneKey(code: Self.deleteCode)])
        try OperatorHandHost.sleep(nanoseconds: Self.deleteGapNanoseconds)
    }

    func switchLanguage(_ chord: PhoneSwitch) throws {
        switch chord {
        case .capsLock:
            try OperatorHandHost.post(HandEvent(.capsLock(down: true), route: route))
            try OperatorHandHost.sleep(nanoseconds: 120_000_000)
            try OperatorHandHost.post(HandEvent(.capsLock(down: false), route: route))
            try OperatorHandHost.sleep(nanoseconds: Self.capsLockSettleNanoseconds)
        case .controlSpace:
            // The Space carries no flags: the phone reads Control from flagsChanged
            // alone, and the Mac's own Control-Space never sees a chord.
            let control = CGEventFlags.maskControl.rawValue
            try OperatorHandHost.post(HandEvent(.key(code: Self.controlCode, down: true, modifier: control), flags: control, route: route))
            try OperatorHandHost.sleep(nanoseconds: 80_000_000)
            try OperatorHandHost.post(HandEvent(.key(code: Self.spaceCode, down: true, modifier: 0), route: route))
            try OperatorHandHost.sleep(nanoseconds: 40_000_000)
            try OperatorHandHost.post(HandEvent(.key(code: Self.spaceCode, down: false, modifier: 0), route: route))
            try OperatorHandHost.sleep(nanoseconds: 150_000_000)
            try OperatorHandHost.post(HandEvent(.key(code: Self.controlCode, down: false, modifier: 0), route: route))
            try OperatorHandHost.sleep(nanoseconds: Self.controlSpaceSettleNanoseconds)
        }
    }
}

enum MirroredPhoneWords {
    static func refusal(_ refusal: PhoneTyping.Refusal) -> String {
        switch refusal {
        case let .unsupported(characters):
            return "no key of the phone's keyboard writes \(characters); nothing was typed. Type the rest, or hand it to the person"
        case .lineBreak:
            return "a line break presses Return, which can send a message; nothing was typed. Type each line, and press-key Return yourself"
        case let .looseJamo(jamo):
            return "\(jamo) would join the syllable beside it on the phone; nothing was typed"
        }
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

extension DesktopWindows {
    /// Where one window stands, asked of the window server: no app is asked. Nil
    /// when the window is gone or is not the owner's.
    static func bounds(windowId: CGWindowID, ownerPid: pid_t) -> CGRect? {
        guard let rows = CGWindowListCopyWindowInfo([.optionIncludingWindow], windowId) as? [[String: Any]],
              let row = rows.first,
              row[kCGWindowOwnerPID as String] as? Int32 == ownerPid,
              let raw = row[kCGWindowBounds as String] as? [String: Any]
        else { return nil }
        return CGRect(dictionaryRepresentation: raw as CFDictionary)
    }
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

func requiredInteger(_ params: [String: JSONValue], _ key: String) throws -> Int {
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

private func prepareApplicationAccessibility(_ appElement: AXUIElement, app: AppDescriptor) {
    _ = stringAttribute(appElement, kAXRoleAttribute as String)
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

/// The app's own windows as its accessibility tree gives them (t-66753 C1),
/// for a look at an app the window server lists no window of.
/// Where each is shown is only what the app (hidden or not) and the window
/// server said of it (`AccessibilityWindow.presence`, t-68324 ③): the tree
/// lists windows of other Spaces and of a hidden app too.
private func accessibilityWindows(of appElement: AXUIElement, app: AppDescriptor) -> [AccessibilityWindow] {
    let serverView = WindowList.serverView(
        onScreen: windowServerInfos([.optionOnScreenOnly]),
        all: windowServerInfos([.optionAll]),
        pid: app.pid
    )
    return (copyArray(appElement, kAXWindowsAttribute as String) ?? []).map { window in
        var owner: pid_t = 0
        if AXUIElementGetPid(window, &owner) != .success {
            owner = 0
        }
        let number = windowNumber(window)
        return AccessibilityWindow(
            pid: owner,
            number: number,
            frame: absoluteFrame(window),
            title: stringAttribute(window, kAXTitleAttribute as String),
            minimized: boolAttribute(window, kAXMinimizedAttribute as String) == true,
            appHidden: app.app.isHidden,
            serverOnScreen: number.flatMap { serverView[$0] }
        )
    }
}

private func windowServerInfos(_ options: CGWindowListOption) -> [[String: Any]] {
    CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
}

/// Why a look read through the app's own tree has no picture (t-66753 C1):
/// said in the answer, so no one takes the tree for what the screen shows.
private func treeOnlyReason(app: AppDescriptor, presence: WindowPresence) -> String {
    let placed = " " + WindowPresence.sentence(for: presence)
    if screenCaptureTrusted() {
        return "the window server lists no window of '\(app.name)' it may picture (the app keeps its windows from capture): this answer is the window's accessibility tree alone, with no picture." + placed
    }
    return "Screen Recording is not granted to ZeroCode Computer Use, so the window server lists no window of '\(app.name)' to picture: this answer is the window's accessibility tree alone, with no picture. Grant it for a picture (zerocode-computer permissions --id screenshots)." + placed
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
    let appElement = AXUIElementCreateApplication(app.pid)
    // Already in front with a window to act in: raising it again only costs the wait.
    if windowId == nil, windowBounds == nil, !app.app.isHidden, frontPidNow() == app.pid,
       copyElement(appElement, kAXFocusedWindowAttribute as String) != nil {
        return
    }
    _ = app.app.unhide()
    _ = app.app.activate(options: [.activateAllWindows])
    if !raiseSettled(app.pid, timeout: DesktopBorrowHost.activateTimeout), let bundleId = app.bundleId {
        openBundle(bundleId)
    }
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
    if raiseSettled(app.pid, timeout: DesktopBorrowHost.raiseTimeout) {
        Thread.sleep(forTimeInterval: DesktopBorrowHost.raisedSettleSeconds)
    }
}

/// Which app takes the keys right now. The accessibility answer is live; the
/// workspace's only moves when the main run loop gets a turn.
private func frontPidNow() -> pid_t? {
    if let focused = copyElement(AXUIElementCreateSystemWide(), kAXFocusedApplicationAttribute as String),
       let pid = pidAttribute(focused) {
        return pid
    }
    return NSWorkspace.shared.frontmostApplication?.processIdentifier
}

/// Whether `pid` came to the front within `timeout`, looking as often as it is cheap to.
private func raiseSettled(_ pid: pid_t, timeout: TimeInterval) -> Bool {
    DesktopBorrow.waitUntilFront(
        timeout: timeout,
        interval: DesktopBorrowHost.raisePollSeconds,
        isFront: { frontPidNow() == pid },
        now: { ProcessInfo.processInfo.systemUptime },
        sleep: { Thread.sleep(forTimeInterval: $0) }
    )
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
    DesktopWindowPick.framesMatch(lhs, rhs)
}

private func openBundle(_ bundleId: String) {
    // With two processes of one bundle this could raise the other one, which may be
    // the person's.
    guard NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).count <= 1 else { return }
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
    (try? performActionChecked(element, action)) == true
}

/// Perform an accessibility action and say what happened. True: it ran. False:
/// the element does not offer it, so nothing happened and another road may go.
/// A timeout or a generic failure throws: the action may have landed, and a
/// second road after it could press twice.
private func performActionChecked(_ element: AXUIElement, _ action: String) throws -> Bool {
    guard actions(element).contains(where: { $0.caseInsensitiveCompare(action) == .orderedSame }) else { return false }
    let context = RequestContext.current
    let sentBefore = context.sent
    try context.askThePersonIfFirst()
    context.sent = true
    let asked = Date()
    let status = AXUIElementPerformAction(element, action as CFString)
    context.appActionMs = Int(Date().timeIntervalSince(asked) * 1_000)
    switch AXActionVerdict.of(raw: status.rawValue) {
    case .done:
        return true
    case .unsupported:
        context.sent = sentBefore
        return false
    case .stale:
        context.sent = sentBefore
        throw ProviderError.coded("element_not_found", "the element is gone: the app removed it; look again")
    case .denied:
        context.sent = sentBefore
        throw ProviderError.coded("permission_denied", "Accessibility is off for ZeroCode Computer Use")
    case .uncertain:
        throw ProviderError.coded("delivery_uncertain", "\(action) answered with an error or not at all, and may have landed: some apps do the press and still report a failure. Look before doing it again.")
    }
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
    /// The address the walk's first web area reads (`AXURL`): the page a
    /// window shows when it names no document of its own (t-69973).
    private(set) var firstWebAreaURL: String?
    private var metWebArea = false
    private let reader = AXSnapshotReader()
    private var rendered = 0
    private let book: ElementIdentityBook<AXUIElement>
    private let rootKey: String
    private let onSurface: Bool
    private let cutDepth: Int?
    private let deadline: Date
    /// The numbers in reading order.
    private(set) var order: [Int] = []
    /// Why the walk stopped early, when it was not the node limit.
    private(set) var cutShort: String?
    static let maxNodes = 1200
    static let maxDepth = 64

    init(
        windowBounds: CGRect,
        focused: AXUIElement?,
        compactBrowserTabs: Bool,
        book: ElementIdentityBook<AXUIElement>,
        rootKey: String,
        onSurface: Bool,
        maxDepth: Int?,
        deadline: Date
    ) {
        self.windowBounds = windowBounds
        self.focused = focused
        self.compactBrowserTabs = compactBrowserTabs
        self.book = book
        self.rootKey = rootKey
        self.onSurface = onSurface
        self.cutDepth = maxDepth
        self.deadline = deadline
    }

    func render(
        _ element: AXUIElement,
        depth: Int = 0,
        ancestors: [AXUIElement] = [],
        clip: CGRect? = nil,
        context: String? = nil,
        contextOwner: AXUIElement? = nil
    ) {
        guard rendered < Self.maxNodes else {
            truncated = true
            return
        }
        if reader.unanswered {
            truncated = true
            cutShort = "app_unresponsive"
            return
        }
        if Date() >= deadline {
            truncated = true
            cutShort = "time_budget"
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
        // The first web area the walk meets (a depth of 0 is the area itself)
        // is the page shown; its address came with the node's other reads.
        if webAreaDepth == 0, !metWebArea {
            metWebArea = true
            firstWebAreaURL = url
        }
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
                render(child, depth: depth, ancestors: ancestors + [element], clip: clip, context: context, contextOwner: contextOwner)
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
        let childContextOwner = name == nil ? contextOwner : element

        let index = book.number(for: element, root: rootKey)
        rendered += 1
        order.append(index)
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
            plainInput: reader.plainInput(element, role: role),
            traits: traits,
            visible: visible,
            context: context,
            contextOwner: contextOwner,
            line: line,
            title: title,
            label: label,
            value: value,
            onSurface: onSurface
        )
        if let focused, CFEqual(focused, element) {
            focusedElementId = index
            focusedSummary = line
        }
        if summary != nil || SnapshotRenderHeuristics.shouldSuppressChildren(node) {
            return
        }
        if let cutDepth, depth >= cutDepth, !children.isEmpty {
            lines[lines.count - 1] += " [\(children.count) inside; --root \(index) reads them]"
            return
        }
        if compactBrowserTabs, let tabStripCompaction = tabStripCompaction(parent: node, children: children) {
            for (childIndex, child) in children.enumerated() where tabStripCompaction.retainedIndexes.contains(childIndex) {
                render(child, depth: depth + 1, ancestors: ancestors + [element], clip: childClip, context: childContext, contextOwner: childContextOwner)
            }
            lines.append(
                String(repeating: "\t", count: depth + 1) +
                    "... \(tabStripCompaction.omittedCount) inactive browser tabs omitted"
            )
            return
        }
        let childLineStart = lines.count
        for child in children {
            render(child, depth: depth + 1, ancestors: ancestors + [element], clip: childClip, context: childContext, contextOwner: childContextOwner)
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
                order.removeAll { $0 == recordIndex }
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
    /// Whether a read timed out. One timeout is enough to stop asking: every
    /// further read of a hung app would wait out its own timeout.
    private(set) var unanswered = false

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

    func plainInput(_ element: AXUIElement, role: String) -> String? {
        guard ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"].contains(role),
              boolAttribute(element, kAXEnabledAttribute as String) == true,
              !isSecureTextElement(element, role: role),
              isSettable(element, kAXValueAttribute as String)
        else { return nil }
        var subrole = copyAttribute(element, kAXSubroleAttribute as String)
        let status = subrole == nil
            ? AXUIElementCopyAttributeValue(element, kAXSubroleAttribute as CFString, &subrole)
            : AXError.success
        guard status == .success || status == .attributeUnsupported || status == .noValue,
              status != .success || subrole is String,
              (subrole as? String) != kAXSecureTextFieldSubrole as String
        else { return nil }
        guard let value = copyAttribute(element, kAXValueAttribute as String) as? String,
              value.utf8.prefix(SnapshotRenderHeuristics.maxPlainInputBytes + 1).count <= SnapshotRenderHeuristics.maxPlainInputBytes else { return nil }
        return value
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
        let status = AXUIElementCopyMultipleAttributeValues(
            cache.element,
            attributes as CFArray,
            [],
            &answered
        )
        if status == .cannotComplete { unanswered = true }
        guard status == .success,
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
        guard !ShareControlPanel.isHelperPID(pid) else { return [] }
        return WindowList.candidates(infos: ShareControlPanel.agentWindows(infos), pid: pid)
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
    static let windowClickHoldNanoseconds: UInt64 = 20_000_000

    static func windowClick(
        at point: CGPoint, button: MouseButtonSelection, count: Int, modifiers: [KeyModifier],
        snapshot: Snapshot, validate: () throws -> Void
    ) throws {
        guard snapshot.windowBounds.contains(point) else {
            throw ProviderError.coded("invalid_argument", "the background point is outside its target window")
        }
        let route = HandRoute.window(pid: snapshot.app.pid, id: snapshot.windowId)
        let local = CGPoint(x: point.x - snapshot.windowBounds.minX, y: point.y - snapshot.windowBounds.minY)
        let flags = modifiers.reduce(into: CGEventFlags()) { $0.insert($1.flag) }.rawValue
        for press in 1...count {
            try validate()
            try OperatorHandHost.post(HandEvent(.buttonDown(button, clickState: Int64(press)), x: local.x, y: local.y, flags: flags, route: route))
            try OperatorHandHost.sleep(nanoseconds: windowClickHoldNanoseconds)
            try OperatorHandHost.post(HandEvent(.buttonUp(button, clickState: Int64(press)), x: local.x, y: local.y, flags: flags, route: route))
        }
    }

    static func windowText(_ text: String, snapshot: Snapshot, validate: () throws -> Void) throws {
        let route = HandRoute.window(pid: snapshot.app.pid, id: snapshot.windowId)
        for chunk in WindowTextChunks(text) {
            try validate()
            try OperatorHandHost.post(HandEvent(.unicode(chunk, down: true), route: route))
            try OperatorHandHost.post(HandEvent(.unicode(chunk, down: false), route: route))
        }
    }

    static func windowScroll(at point: CGPoint, direction: String, delta: Int32, snapshot: Snapshot) throws {
        guard snapshot.windowBounds.contains(point) else {
            throw ProviderError.coded("invalid_argument", "the background point is outside its target window")
        }
        let wheel1: Int32 = direction == "up" ? delta : direction == "down" ? -delta : 0
        let wheel2: Int32 = direction == "left" ? delta : direction == "right" ? -delta : 0
        try OperatorHandHost.post(HandEvent(
            .scroll(wheel1: wheel1, wheel2: wheel2),
            x: point.x - snapshot.windowBounds.minX, y: point.y - snapshot.windowBounds.minY,
            route: .window(pid: snapshot.app.pid, id: snapshot.windowId)
        ))
    }

    static func windowKey(_ key: String, snapshot: Snapshot, validate: () throws -> Void) throws {
        let route = HandRoute.window(pid: snapshot.app.pid, id: snapshot.windowId)
        let stroke = try KeyMap.parse(key).stroke
        let releases = stroke.ups(route: route)
        defer { for release in releases { try? OperatorHandHost.post(release) } }
        for press in stroke.downs(route: route) {
            try validate()
            try OperatorHandHost.post(press)
        }
        for release in releases { try OperatorHandHost.post(release) }
    }

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

    /// Where an app's keys go. On a borrowed desktop they go to that app's own process:
    /// what the person had comes back moments later, and a key still on its way through
    /// the shared queue would land in whatever is in front by then.
    private static func keyRoute(pid: pid_t) -> HandRoute {
        RequestContext.current.borrowed && pid > 0 ? .process(pid) : .desktop
    }

    static func typeText(_ text: String, pid: pid_t) throws {
        let route = keyRoute(pid: pid)
        for stroke in TextStrokes.strokes(text) {
            // A stop lands between two characters; one inside a press lets
            // go of it at once.
            try OperatorHandHost.post(HandEvent(.unicode(stroke, down: true), route: route))
            try OperatorHandHost.post(HandEvent(.unicode(stroke, down: false), route: route))
        }
    }

    /// A chord on the desktop (`KeyChordStroke`, the sequence a reflex key
    /// posts to its app's process): the key lets go under its modifiers, and
    /// each modifier that went down lets go whatever stopped the chord — the
    /// hand posts a release only of what the hold still holds.
    static func pressKey(_ key: String, pid: pid_t) throws {
        let stroke = try KeyMap.parse(key).stroke
        let route = keyRoute(pid: pid)
        let ups = stroke.ups(route: route)
        defer {
            for modifier in ups.dropFirst() { try? OperatorHandHost.post(modifier) }
        }
        for press in stroke.downs(route: route) { try OperatorHandHost.post(press) }
        try OperatorHandHost.post(ups[0])
    }

    /// How long a posted ⌘V is given to be consumed before the previous
    /// clipboard is put back. The key event is asynchronous: restoring in the
    /// same breath handed the target the OLD clipboard (2026-09-10, a terminal
    /// pane pasted a stale URL in place of the text).
    static let pasteSettleSeconds: TimeInterval = 0.25

    static func pasteText(_ text: String, pid: pid_t) throws {
        // Whatever watches the clipboard sees the write, so it comes after asking.
        try RequestContext.current.askThePersonIfFirst()
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
    /// Throws when the field did not answer and the text may have landed: typing
    /// it again down another road could enter it twice.
    static func replaceSelection(_ element: AXUIElement, with text: String) throws -> ReplaceSelection {
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
        let context = RequestContext.current
        let sentBefore = context.sent
        try context.askThePersonIfFirst()
        context.sent = true
        let status = AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, next as CFString)
        let setSucceeded = status == .success
        if !setSucceeded {
            // A field that answers with what it held took nothing. Any other answer,
            // or none, may mean the text is in.
            let holds = rawStringAttribute(element, kAXValueAttribute as String)
            let verdict = AXActionVerdict.of(raw: status.rawValue)
            guard verdict != .uncertain || holds == current else {
                throw ProviderError.coded("delivery_uncertain", "the field did not answer in time, and the text may have landed; read it before typing again")
            }
            context.sent = sentBefore
            return .unavailable
        }
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

struct KeyModifier {
    let keyCode: CGKeyCode
    let flag: CGEventFlags
}

struct ParsedKey {
    let keyCode: CGKeyCode
    let modifiers: [KeyModifier]

    /// The chord as the hand posts it.
    var stroke: KeyChordStroke {
        KeyChordStroke(key: keyCode, modifiers: modifiers.map { KeyChordStroke.Modifier(code: $0.keyCode, flag: $0.flag.rawValue) })
    }
}

/// The helper's own key table as a reflex run reads it (`ReflexKeyboard`,
/// t-10384): the names, codes and flags the key verbs post from.
struct KeyMapKeyboard: ReflexKeyboard {
    func chord(key: String, modifiers: [String]) -> KeyChordStroke? {
        try? KeyMap.parse((modifiers + [key]).joined(separator: "+")).stroke
    }

    func flags(_ modifiers: [String]) -> UInt64? {
        guard let held = try? KeyMap.parseModifiers(modifiers.joined(separator: "+")) else { return nil }
        return held.reduce(UInt64(0)) { $0 | $1.flag.rawValue }
    }
}

enum KeyMap {
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
        case "alt", "option", "opt":
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
        "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100,
        "f9": 101, "f10": 109, "f11": 103, "f12": 111,
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
    private let lanes = LaneLocks()
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
        var reader = BoundedLineReader()
        while let data = try? reader.next(read: { buffer in
            var count: Int
            repeat { count = read(fd, buffer.baseAddress, buffer.count) } while count < 0 && errno == EINTR
            if count < 0 { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            return count
        }) {
            let keepReading = autoreleasepool { () -> Bool in
                guard let request = try? decoder.decode(Request.self, from: data)
                else {
                    return true
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
                        return false
                    }
                    sessionLock.lock()
                    let registration = sessionOwnership.registerConnection(
                        connectionID,
                        authenticated: true
                    )
                    sessionLock.unlock()
                    guard registration != .rejected else {
                        monitor.cancel()
                        return false
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
                    lanes: lanes,
                    request: request,
                    expectedToken: token,
                    authorizedPeer: authorizedPeer
                )
                writeJSON(response, to: fd)
                return true
            }
            if !keepReading { return }
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
    // One slow call must not hold a lane for the system's six seconds. Set on the
    // system-wide element, this is the helper's default for every element.
    _ = AXUIElementSetMessagingTimeout(AXUIElementCreateSystemWide(), AccessibilityTimeouts.messagingSeconds)
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
    lanes: LaneLocks,
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
    let params = request.params ?? [:]
    RequestContext.begin(params)
    do {
        if let answer = try unlockedRoad(request.method, params: params) {
            return ["id": request.id, "ok": true, "result": answer]
        }
    } catch let error as ProviderError {
        return failureAnswer(id: request.id, code: error.code, message: error.message)
    } catch {
        return failureAnswer(id: request.id, code: "invalid_argument", message: String(describing: error))
    }

    // The name a sequence holds an app by: however it was spelled, the lane it runs in.
    if request.method == "appKey" {
        return ["id": request.id, "ok": true, "result": ["key": provider.lane(method: request.method, params: params)]]
    }
    // One lane per app: a slow or hung app holds up only the requests naming it.
    let lane = lanes.lock(for: provider.lane(method: request.method, params: params))
    do {
        lane.lock()
        defer { lane.unlock() }
        let result = try provider.handle(method: request.method, params: params)
        return ["id": request.id, "ok": true, "result": result]
    } catch let error as ProviderError {
        return failureAnswer(id: request.id, code: error.code, message: error.message)
    } catch {
        return failureAnswer(id: request.id, code: "accessibility_error", message: String(describing: error))
    }
}

/// A refusal, saying whether anything reached the app and so whether it may be
/// repeated.
private func failureAnswer(id: Int, code: String, message: String) -> [String: Any] {
    let delivery = ActionDelivery.failed(anythingSent: RequestContext.current.sent)
    return ["id": id, "ok": false, "error": [
        "code": code, "message": message, "delivery": delivery.rawValue, "retry": delivery.retry,
    ]]
}

/// The requests answered outside the provider lock, or nil for every other.
/// A reflex run's road names the run it means (`run`), and the host compares
/// and answers it under its own lock (`ReflexRuntimeHost`).
func unlockedRoad(_ method: String, params: [String: JSONValue]) throws -> [String: Any]? {
    switch method {
    case "sharePanel":
        try ShareControlPanel.update(params)
        return ShareControlPanel.status()
    case "stop":
        OperatorGuardHost.stop(reason: StopReason.request)
        return OperatorGuardHost.status()
    case "status":
        return OperatorGuardHost.status()
    case "stopWatch":
        let waitMs = try requiredInteger(params, "ms")
        let after = try optionalInteger(params, "after")
        guard waitMs > 0, (after ?? 0) >= 0 else {
            throw ProviderError.coded("invalid_argument", "stopWatch waits ms above 0, after a stopRevision from 0")
        }
        return OperatorGuardHost.watch(after: after.map { UInt64($0) }, waitMs: waitMs, afterShare: try optionalInteger(params, "afterShare"))
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

/// Who a `resume` names as lifting the stop (t-66753 A): none is an agent;
/// a word the helper does not know, or no word at all, is refused.
private func stopLifter(_ params: [String: JSONValue]) throws -> StopLifter {
    switch params[StopLifter.key] {
    case nil, .null?:
        return .agent
    case let .string(word)?:
        if let lifter = StopLifter(rawValue: word) { return lifter }
    default:
        break
    }
    throw ProviderError.coded("invalid_argument", "\(StopLifter.key) is \(StopLifter.person.rawValue) or \(StopLifter.agent.rawValue)")
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
func desktopPoint(_ params: [String: JSONValue], _ xKey: String, _ yKey: String) throws -> CGPoint {
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
        // A region at point resolution is already small: no budget ladder.
        if params["fullRes"]?.bool == true || params["pointScale"] != nil {
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

    /// `pointScale` as sent: absent is nil, a number is it, anything else is
    /// refused rather than read as absent.
    private static func pointScaleArgument(_ params: [String: JSONValue]) throws -> Double? {
        guard let value = params["pointScale"] else { return nil }
        guard let number = value.number else {
            throw ProviderError.coded("invalid_argument", "pointScale must be a number above 0 and at most \(Int(DesktopPointCapture.maxPointScale))")
        }
        return number
    }

    /// What the screen shows in `rect` (global points), drawn at the pixel
    /// size asked — the region alone, never the whole display.
    private static func captureRegion(_ rect: CGRect, pixelWidth: Int, pixelHeight: Int) -> CGImage? {
        guard let shown = CGWindowListCreateImage(rect, [.optionOnScreenOnly], kCGNullWindowID, [.nominalResolution]) else { return nil }
        if shown.width == pixelWidth, shown.height == pixelHeight { return shown }
        guard let context = CGContext(data: nil, width: pixelWidth, height: pixelHeight, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
            return nil
        }
        context.interpolationQuality = .medium
        context.draw(shown, in: CGRect(x: 0, y: 0, width: pixelWidth, height: pixelHeight))
        return context.makeImage()
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
        } else {
            chosen = all[desktopDisplayIndex(showing: region, among: all.map(\.bounds))]
        }
        // Only a request that names pointScale is planned; the rest keeps the
        // whole-display capture below, cropped, as it was (t-10424).
        let pointScale: Double?
        switch DesktopPointCapture.pointScale(try pointScaleArgument(params)) {
        case let .success(value): pointScale = value
        case let .failure(error): throw ProviderError.coded("invalid_argument", error.message)
        }
        if let pointScale {
            guard case let .region(sourceRect, pixelWidth, pixelHeight)? = DesktopPointCapture.plan(
                region: region, displayBounds: chosen.bounds, pointScale: pointScale)
            else {
                throw ProviderError.coded("invalid_argument", "region does not touch display \(chosen.id)")
            }
            guard let picture = captureRegion(sourceRect, pixelWidth: pixelWidth, pixelHeight: pixelHeight) else {
                throw ProviderError.coded("accessibility_error", "capturing display \(chosen.id) failed")
            }
            return Frame(
                image: picture,
                origin: sourceRect.origin,
                pointsWidth: sourceRect.width,
                pointsHeight: sourceRect.height,
                display: chosen,
                displayIndex: all.firstIndex { $0.id == chosen.id } ?? 0
            )
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
        if let captured = params["capturedFrame"] {
            guard case let .object(frame) = captured,
                  case let .object(screenshot)? = frame["screenshot"],
                  case let .object(window)? = frame["window"],
                  let encoded = screenshot["data"]?.string,
                  let bytes = Data(base64Encoded: encoded),
                  let source = CGImageSourceCreateWithData(bytes as CFData, nil),
                  let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
            else { throw ProviderError.coded("screenshot_failed", "the captured OCR frame could not be decoded") }
            let bounds = try pixelRect(window)
            return (image, bounds.origin, bounds.width, bounds.height, ["capturedFrame": true, "pixelTextPins": true])
        }
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

    private func pixelRect(_ fields: [String: JSONValue]) throws -> CGRect {
        let numbers = ["x", "y", "width", "height"].compactMap { fields[$0]?.number }
        guard numbers.count == 4, numbers.allSatisfy({ $0.isFinite }),
              numbers[2] > 0, numbers[3] > 0
        else { throw ProviderError.coded("invalid_argument", "an OCR target needs a finite positive rectangle") }
        return CGRect(x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3])
    }

    private func validatePixelTarget(_ requested: JSONValue, snapshot: Snapshot) throws {
        guard case let .object(fields) = requested,
              case let .object(frame)? = fields["frame"],
              case let .object(window)? = fields["window"],
              let text = fields["text"]?.string,
              let tolerance = fields["tolerance"]?.number,
              let confidence = fields["minimumConfidence"]?.number
        else { throw ProviderError.coded("invalid_argument", "the OCR target pin is incomplete") }
        let pin = PixelTextPin(text: text, frame: try pixelRect(frame), window: try pixelRect(window), tolerance: tolerance, minimumConfidence: confidence)
        guard pin.windowStands(snapshot.windowBounds),
              screenCaptureTrusted(),
              let captured = WindowCapture.captureImage(windowId: snapshot.windowId, bounds: snapshot.windowBounds)
        else { throw ProviderError.coded("window_stale", "the OCR target's window cannot be verified; look again") }
        let lines = try recognizeText(in: captured.image, origin: snapshot.windowBounds.origin, pointsWidth: snapshot.windowBounds.width, pointsHeight: snapshot.windowBounds.height)
        guard pin.holds(lines: lines, window: snapshot.windowBounds) else {
            throw ProviderError.coded("element_not_found", "the OCR target's words or position changed; look again")
        }
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
        for index in snapshot.order {
            guard let record = snapshot.elements[index] else { continue }
            guard query.matches(role: record.role, label: record.title, value: record.value, description: record.label) else { continue }
            seen += 1
            guard matches.count < Self.findMatchLimit else { continue }
            let frame = absoluteFrame(record.element)
            matches.append([
                "index": index,
                "role": jsonNullable(record.role),
                "label": jsonNullable(record.title),
                "value": jsonNullable(record.value),
                "description": jsonNullable(record.label),
                "actions": record.actions,
                "frame": frame.map { ["x": $0.minX, "y": $0.minY, "width": $0.width, "height": $0.height, "centerX": $0.midX, "centerY": $0.midY] } ?? NSNull(),
            ])
        }
        return [
            "source": "accessibility",
            "coordinateSpace": "screen",
            "snapshotId": snapshot.id,
            "app": snapshot.app.name,
            "window": namedWindow(snapshot),
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
        let stroke = try KeyMap.parse(key).stroke
        for press in stroke.downs(route: .desktop) { try OperatorHandHost.post(press) }
        try OperatorHandHost.sleep(nanoseconds: UInt64(max(0, milliseconds)) * 1_000_000)
        for release in stroke.ups(route: .desktop) { try OperatorHandHost.post(release) }
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
    private let windowPoster = LockedWindowEventPoster()

    func post(_ event: HandEvent, tag: Int64) throws {
        if case .window = event.route {
            try windowPoster.post(event, tag: tag)
            return
        }
        let made = try make(event)
        made.setIntegerValueField(.eventSourceUserData, value: tag)
        switch event.route {
        case .desktop:
            made.post(tap: .cghidEventTap)
        case let .process(pid):
            made.postToPid(pid)
        case .window:
            throw ProviderError.coded("background_input_unavailable", "window-scoped input requires its private event source")
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
        case let .unicode(text, down):
            guard let made = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else {
                throw ProviderError.coded("accessibility_error", "failed to create keyboard event")
            }
            let units = Array(text.utf16)
            made.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
            return made
        case let .capsLock(down):
            // The shape a Caps Lock press takes on its way into an app: a
            // system-defined event, field 83 = 0xD3, field 149 = 1 down, 0 up.
            guard let made = CGEvent(source: source),
                  let kind = CGEventType(rawValue: 14),
                  let subtype = CGEventField(rawValue: 83),
                  let state = CGEventField(rawValue: 149) else {
                throw ProviderError.coded("accessibility_error", "failed to create the Caps Lock event")
            }
            made.type = kind
            made.setIntegerValueField(subtype, value: 0xD3)
            made.setIntegerValueField(state, value: down ? 1 : 0)
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
        let comesUpBehind = params["background"]?.bool == true
        let front = comesUpBehind ? NSWorkspace.shared.frontmostApplication : nil
        let launchedAt = ProcessInfo.processInfo.systemUptime
        let launched = try BlockingAsync.run(timeout: launchProcessTimeoutSeconds) {
            // The configuration is built inside the closure: it is not Sendable.
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = !comesUpBehind
            configuration.arguments = args
            return try await NSWorkspace.shared.openApplication(at: url, configuration: configuration)
        }
        let pid = launched.processIdentifier
        keepInFront(front, over: pid, launchedAt: launchedAt)
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

    /// How long an app is watched: as long as a launch may take to come up at
    /// all, since an app that connects first (iPhone Mirroring) comes forward when
    /// it has, not at its first window. How late it comes was not measured.
    private static let comesForwardLateSeconds: TimeInterval = launchProcessTimeoutSeconds
    /// Told to come up behind, an app may still bring itself to the front. What the
    /// person had goes back in front, unless they moved on since (`FrontWatch`).
    /// Beside the launch, which answers without waiting for it.
    /// `launchedAt` is the system uptime the launch began at: input since then
    /// is the person's say over the front.
    private static func keepInFront(_ front: NSRunningApplication?, over pid: pid_t, launchedAt: TimeInterval) {
        guard let front, front.processIdentifier != pid else { return }
        DispatchQueue.global(qos: .utility).async {
            var watching = FrontWatch(original: front.processIdentifier, launched: pid)
            _ = watchFront(&watching, of: front, launchedAt: launchedAt, for: comesForwardLateSeconds)
        }
    }

    /// `kCGAnyInputEventType`: the age of the newest input of any type.
    private static let anyInput = CGEventType(rawValue: ~0)!

    private static func watchFront(_ watching: inout FrontWatch, of front: NSRunningApplication, launchedAt: TimeInterval, for seconds: TimeInterval) -> Bool {
        let giveUp = Date().addingTimeInterval(seconds)
        return watching.run(
            until: { Date() >= giveUp },
            front: { NSWorkspace.shared.frontmostApplication?.processIdentifier },
            // `isTerminated` changes as the main run loop runs, which this helper's
            // main thread may not be doing: the process itself is asked as well.
            originalAlive: {
                let pid = front.processIdentifier
                return !front.isTerminated && (kill(pid, 0) == 0 || errno == EPERM)
            },
            // The HID system's table counts what came through the input devices.
            // The helper's process input (`HandRoute.process`, CGEventHandPoster)
            // and window input (WindowEventEncoding, a private source) go by
            // `postToPid` into one process's queue and never pass the HID system,
            // so its ages do not count them. Its desktop input is posted at the
            // HID tap and may count; DesktopBorrowHost stamps each such post and
            // `personActed` takes a matching age for it. Read from the code, not
            // measured on a person's machine.
            personActed: {
                FrontWatch.personActed(
                    sinceLaunch: ProcessInfo.processInfo.systemUptime - launchedAt,
                    reading: DesktopBorrowHost.reading(),
                    sinceAnyInput: CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: anyInput)
                )
            },
            giveBack: {
                _ = front.activate(options: [])
                if !front.isActive, let bundle = front.bundleIdentifier { openBundle(bundle) }
            },
            pause: { usleep(readyPollMs * 1_000) }
        )
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
        let comesUpBehind = params["background"]?.bool == true
        if let with = params["with"]?.string {
            let application = try resolveApplicationURL(with, running: running)
            let front = comesUpBehind ? NSWorkspace.shared.frontmostApplication : nil
            let launchedAt = ProcessInfo.processInfo.systemUptime
            let wasRunning = running.contains { $0.app.bundleURL == application }
            let came = try? BlockingAsync.run(timeout: launchProcessTimeoutSeconds) {
                let configuration = NSWorkspace.OpenConfiguration()
                configuration.activates = !comesUpBehind
                return try await NSWorkspace.shared.open([target], withApplicationAt: application, configuration: configuration)
            }
            opened = came != nil
            if let came, !wasRunning { keepInFront(front, over: came.processIdentifier, launchedAt: launchedAt) }
        } else if comesUpBehind {
            opened = (try? BlockingAsync.run(timeout: launchProcessTimeoutSeconds) {
                let configuration = NSWorkspace.OpenConfiguration()
                configuration.activates = false
                return try await NSWorkspace.shared.open(target, configuration: configuration)
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
        return ShareControlPanel.agentWindows(infos).flatMap { info -> [[String: Any]] in
            guard let layer = info[kCGWindowLayer as String] as? Int, everyLayer || layer == 0,
                  let number = info[kCGWindowNumber as String] as? NSNumber,
                  let ownerPid = info[kCGWindowOwnerPID as String] as? pid_t,
                  !ShareControlPanel.isHelperPID(ownerPid),
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
                // The pointer's own picture, told once for every reader (t-12979).
                row["pointer"] = DesktopPointerPicture.isPointerPicture(
                    layer: layer, ownerName: (info[kCGWindowOwnerName as String] as? String) ?? "", bounds: bounds)
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
        // Many Cocoa windows publish no AXWindowNumber: the window the list
        // numbers is then the one numberless window at its frame, if only one
        // stands there (t-15085); two are refused as a missing one is.
        let listed = (CGWindowListCopyWindowInfo([.optionIncludingWindow], windowId) as? [[String: Any]])?.first
        let targetBounds = (listed?[kCGWindowBounds as String] as? NSDictionary).flatMap { CGRect(dictionaryRepresentation: $0) }
        guard let windows = copyArray(appElement, kAXWindowsAttribute as String),
              case let .found(at) = DesktopWindowPick.pick(
                  target: windowId,
                  targetBounds: targetBounds,
                  windows: windows.map { (number: windowNumber($0), frame: absoluteFrame($0)) })
        else {
            throw ProviderError.coded("window_not_found", "window \(windowId) has no accessibility element (is Accessibility granted, and does the app publish its windows?)")
        }
        return (windows[at], ownerPid)
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
    /// Guards the ledger, and wakes the window's waiting request when the
    /// stop moved (t-66753 B).
    private static let lock = NSCondition()
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

    /// Change the ledger under its lock; a stop that moved wakes the window's
    /// waiting request (t-66753 B).
    private static func changing<T>(_ change: (inout OperatorLedger) -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        let before = ledger.stopRevision
        let result = change(&ledger)
        if ledger.stopRevision != before { lock.broadcast() }
        return result
    }

    /// The stop, on whatever thread it came in on: the hand lets go of what
    /// it pressed first, then the count refuses, then a reflex run ends. The
    /// person's chord keeps its reason through a later stop that is not
    /// theirs (t-66753 A): the stop is theirs, as the reason a run ends for.
    static func stop(reason: String) {
        let release = OperatorHandHost.hand.stop(reason: reason)
        let standing = changing { ledger in
            ledger.stop(reason: reason)
            return ledger.stoppedReason ?? reason
        }
        BackgroundPointerOverlay.hide()
        fputs("computer-use operator stopped: \(reason), standing \(standing) (let go of \(release.released.count), unconfirmed \(release.unconfirmed.count))\n", stderr)
        ReflexRuntimeHost.operatorStopped(reason: standing)
    }

    /// Lift the stop for `lifter` (t-66753 A): the person's stop for the
    /// person — the window's band — alone; anyone else's resume of it is
    /// refused and lifts nothing. So is a release the hand could not confirm
    /// (t-68324 ②): the hand answers for it, and an agent's resume over it
    /// lifts neither that nor the stop standing beside it. The hand is asked
    /// inside the ledger's turn, so a refusal leaves both as they were.
    static func resume(resetBudget: Bool, by lifter: StopLifter, hand: OperatorHand = OperatorHandHost.hand) throws {
        enum Outcome { case lifted, personsStop(String), personsRelease }
        let outcome: Outcome = changing { ledger in
            guard ledger.liftable(by: lifter) else { return .personsStop(ledger.stoppedReason ?? "") }
            do {
                try hand.resume(by: lifter)
            } catch {
                return .personsRelease
            }
            ledger.resume(resetBudget: resetBudget)
            return .lifted
        }
        switch outcome {
        case .lifted:
            return
        case let .personsStop(reason):
            throw ProviderError.coded("stopped", "the operator is stopped (\(reason)); the person stopped it — only they lift it, from the window's band")
        case .personsRelease:
            throw ProviderError.coded("stopped", OperatorHandHost.releaseUnconfirmedWords)
        }
    }

    /// The window's waiting request (t-66753 B): answered once the stop is
    /// no longer the one the window last heard (`after`, a `stopRevision`),
    /// else after `waitMs` with the stop as it stands — at once without
    /// `after`. It waits on its connection's own thread, outside the provider
    /// lock: no request waits behind it, and it waits behind none.
    static func shareChanged() {
        lock.lock()
        lock.broadcast()
        lock.unlock()
    }

    static func watch(after: UInt64?, waitMs: Int, afterShare: Int? = nil) -> [String: Any] {
        let deadline = Date(timeIntervalSinceNow: Double(waitMs) / 1_000)
        lock.lock()
        while let after, ledger.stopRevision == after,
              afterShare == nil || ShareControlPanel.status()["event"] as? Int == afterShare,
              lock.wait(until: deadline) {}
        lock.unlock()
        return status()
    }

    /// One action's admission as the ledger answers it, for a reflex run's
    /// leaf: the run sleeps a pace's wait on the hand, where a stop ends it.
    static func admission() -> GuardAdmission {
        changing { $0.admit(now: Date().timeIntervalSince1970) }
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
            let (admission, refusal) = changing { ($0.admit(now: Date().timeIntervalSince1970), tableRefusal) }
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

    static func isStopped() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return ledger.isStopped
    }

    static func status() -> [String: Any] {
        lock.lock()
        let snapshot = ledger
        let refusal = tableRefusal
        lock.unlock()
        let now = Date().timeIntervalSince1970
        return [
            "stopped": snapshot.isStopped,
            "backgroundPointer": BackgroundPointerOverlay.status(),
            "shareControl": ShareControlPanel.status(),
            "reason": jsonNullable(snapshot.stoppedReason),
            // What the window's waiting request asks after (t-66753 B).
            "stopRevision": Int(clamping: snapshot.stopRevision),
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
    /// The hold of each request running now, by the thread it runs on: requests
    /// in different lanes act at once.
    nonisolated(unsafe) private static var requests: [ObjectIdentifier: OperatorHand.Token] = [:]
    /// How long a shared-desktop request waits its turn for the pointer. Requests
    /// take turns on it instead of being refused; a reflex run is still refused,
    /// because it holds the desk until its deadline.
    private static let turnWaitSeconds: TimeInterval = 10
    private static let turnPollMicros: UInt32 = 2_000

    /// Run one acting request while it holds its part of the hand; whatever it
    /// left held is let go of when it ends.
    static func whileHeld<T>(scoped: Bool = false, _ body: () throws -> T) throws -> T {
        let token = try take(scoped ? .scoped : .request)
        let thread = ObjectIdentifier(Thread.current)
        lock.lock()
        requests[thread] = token
        lock.unlock()
        defer {
            lock.lock()
            requests[thread] = nil
            lock.unlock()
            hand.relinquish(token)
        }
        return try body()
    }

    private static func take(_ owner: OperatorHand.Owner) throws -> OperatorHand.Token {
        let giveUp = Date().addingTimeInterval(turnWaitSeconds)
        while true {
            do {
                return try hand.acquire(owner)
            } catch OperatorHand.Refusal.busy(.request) where Date() < giveUp {
                usleep(turnPollMicros)
            } catch let refusal as OperatorHand.Refusal {
                throw providerError(refusal, acquiring: true)
            }
        }
    }

    static func revokeForShare() {
        lock.lock()
        let held = Array(requests.values)
        lock.unlock()
        for token in held { hand.revoke(token, reason: "person_took_control") }
    }

    /// Post one event for the request holding the hand.
    static func post(_ event: HandEvent) throws {
        let token = try current()
        let context = RequestContext.current
        // A release sends nothing that was not sent: the hand drops one of nothing
        // held, and it goes whatever stopped the press, so it asks no one.
        if event.letsGo == nil {
            try context.askThePersonIfFirst()
            context.sent = true
        }
        do {
            try hand.post(event, by: token)
        } catch let refusal as OperatorHand.Refusal {
            throw providerError(refusal, acquiring: false)
        }
        DesktopBorrowHost.noteOwn(event)
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
        guard let request = requests[ObjectIdentifier(Thread.current)] else {
            throw ProviderError.coded("stopped", "no request holds the hand")
        }
        return request
    }

    /// What a release the platform refused says (t-68324 ②): the person
    /// checks the keys and buttons and lifts it — an agent's `resume` does not.
    static let releaseUnconfirmedWords = "a release the hand posted could not be confirmed; the person checks the keys and buttons — only they lift it, from the window's band"

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
        case .busy(.request), .busy(.scoped):
            return .coded("hand_busy", "another request holds the hand")
        case .notHolder:
            return .coded("stopped", "this request no longer holds the hand")
        case .releaseUnconfirmed:
            return .coded("stopped", releaseUnconfirmedWords)
        }
    }
}

/// The helper's half of `DesktopBorrow`: what the system says about the input
/// devices, and the record of what this helper posted through them.
enum DesktopBorrowHost {
    /// How long `activate` gets before `open -b`, which starts a process, is tried.
    static let activateTimeout: TimeInterval = 0.12
    static let raiseTimeout: TimeInterval = 0.4
    static let raisePollSeconds: TimeInterval = 0.015
    /// A window just raised has yet to draw itself in front.
    static let raisedSettleSeconds: TimeInterval = 0.03

    private static let lock = NSLock()
    nonisolated(unsafe) private static var ownKeyAt: TimeInterval?
    nonisolated(unsafe) private static var ownPointerAt: TimeInterval?
    nonisolated(unsafe) private static var ownWarpAt: TimeInterval?
    /// When a borrow last gave the front back, agent by agent: another agent's
    /// keys are not the ones that mistake what is in front.
    nonisolated(unsafe) private static var gaveBackAt: [String: TimeInterval] = [:]

    private static let pointerEvents: [CGEventType] = [
        .mouseMoved, .scrollWheel,
        .leftMouseDown, .leftMouseUp, .leftMouseDragged,
        .rightMouseDown, .rightMouseUp, .rightMouseDragged,
        .otherMouseDown, .otherMouseUp, .otherMouseDragged,
    ]
    private static let buttons: [CGMouseButton] = [.left, .right, .center]
    private static let chordFlags: CGEventFlags = [.maskShift, .maskControl, .maskAlternate, .maskCommand]

    private static func now() -> TimeInterval { ProcessInfo.processInfo.systemUptime }

    /// Record an event this helper just posted through the input devices: whether
    /// the system's table counts it is the table's business, and `personAge` takes
    /// either answer. Background input goes through neither, and says nothing.
    static func noteOwn(_ event: HandEvent) {
        if case .window = event.route { return }
        let at = now()
        lock.lock()
        if event.points {
            ownPointerAt = at
        } else if pressesAKey(event.kind) {
            ownKeyAt = at
        }
        lock.unlock()
        if case .desktop = event.route, placesThePointer(event.kind) {
            RequestContext.current.pointerPosted = SmoothPointerPath.Point(x: event.x, y: event.y)
        }
    }

    /// A warp is ours too, should the system's table count it.
    static func noteOwnWarp() {
        let at = now()
        lock.lock()
        ownWarpAt = at
        lock.unlock()
    }

    private static func placesThePointer(_ kind: HandEvent.Kind) -> Bool {
        switch kind {
        case .pointerMove, .buttonDown, .buttonUp, .buttonDrag: return true
        case .scroll, .key, .unicode, .capsLock: return false
        }
    }

    /// What the table's key ages count: a press, and a modifier either way.
    private static func pressesAKey(_ kind: HandEvent.Kind) -> Bool {
        switch kind {
        case let .key(_, down, modifier): return down || modifier != 0
        case let .unicode(_, down): return down
        case let .capsLock(down): return down
        default: return false
        }
    }

    static func reading() -> DesktopBorrow.Reading {
        lock.lock()
        let ownKey = ownKeyAt
        let ownPointer = ownPointerAt
        let ownWarp = ownWarpAt
        lock.unlock()
        let at = now()
        func since(_ types: [CGEventType]) -> Double {
            types.map { CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: $0) }.min() ?? .infinity
        }
        return DesktopBorrow.Reading(
            sinceKey: since([.keyDown, .flagsChanged]),
            sincePointer: since(pointerEvents),
            buttonHeld: buttons.contains { CGEventSource.buttonState(.hidSystemState, button: $0) },
            modifierHeld: !CGEventSource.flagsState(.hidSystemState).intersection(chordFlags).isEmpty,
            sinceModifier: since([.flagsChanged]),
            sinceOwnKey: ownKey.map { at - $0 },
            sinceOwnPointer: ownPointer.map { at - $0 },
            sinceOwnWarp: ownWarp.map { at - $0 }
        )
    }

    /// With the turn held, so a stop or a reflex run answers before the person does,
    /// and on the hand's own clock, so a stop ends the wait at once. Other agents'
    /// turns on the shared desktop would be waiting for the same person.
    static func waitForThePerson() throws {
        try refuse(DesktopBorrow.waitForQuiet(
            read: reading, now: now, giveUpAt: now() + DesktopBorrow.longestWaitSeconds,
            sleep: { try OperatorHandHost.sleep(nanoseconds: UInt64($0 * 1_000_000_000)) }
        ))
    }

    static func refuseIfThePersonIsBusy() throws {
        try refuse(DesktopBorrow.busy(reading()))
    }

    private static func refuse(_ busy: DesktopBorrow.Busy?) throws {
        switch busy {
        case nil:
            return
        case .typing:
            throw ProviderError.coded(DesktopBorrow.Busy.typing.rawValue, "the person is typing, and this act needs the keyboard or the pointer; nothing was done. Try again shortly")
        case .pointing:
            throw ProviderError.coded(DesktopBorrow.Busy.pointing.rawValue, "the person is using the mouse, and this act needs the keyboard or the pointer; nothing was done. Try again shortly")
        }
    }

    static func gaveBack(agent: String?) {
        lock.lock()
        let at = now()
        gaveBackAt = gaveBackAt.filter { at - $0.value < DesktopBorrow.appLessKeysRefusedSeconds }
        gaveBackAt[agent ?? ""] = at
        lock.unlock()
    }

    /// An agent that brought an app up itself knows what is in front.
    static func namedTheFront(agent: String?) {
        lock.lock()
        gaveBackAt[agent ?? ""] = nil
        lock.unlock()
    }

    static func refuseAppLessKeys(agent: String?) throws {
        lock.lock()
        let since = gaveBackAt[agent ?? ""].map { now() - $0 }
        lock.unlock()
        guard DesktopBorrow.refusesAppLessKeys(sinceGiveBack: since) else { return }
        throw ProviderError.coded("invalid_argument", "a borrow just put the person's app back in front and these keys name no app, so they would land in it; nothing was sent. Name the app (type-text --app, press-key --app) or activate it first")
    }
}

/// ZeroCode's own window is never the operator's target (§1.6): a desktop
/// point that lands on it, or keys while it is frontmost, are refused unless
/// the caller said `--allow-self` (the test harness does).
enum DesktopSelf {
    /// Whose window a click at `point` lands on: the frontmost that hides
    /// what is under it — an overlay (`DesktopOverlay`) does not, and taking
    /// the Dock's for one let every click through to ZeroCode's own window;
    /// nor does the pointer's own picture (`DesktopPointerPicture`), and
    /// taking it for one let a click where the pointer rests through to
    /// ZeroCode's own window (t-12979). The walk is `DesktopFrontOwner`'s.
    static func ownerPid(at point: CGPoint) -> pid_t? {
        guard let infos = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] else { return nil }
        return DesktopFrontOwner.pid(at: point, infos: infos.filter { !BackgroundPointerOverlay.owns($0) }, displays: DesktopScreen.displays().map(\.bounds)) { point in
            var hit: AXUIElement?
            guard AXUIElementCopyElementAtPosition(AXUIElementCreateSystemWide(), Float(point.x), Float(point.y), &hit) == .success,
                  let hit else { return nil }
            return pidAttribute(hit)
        }
    }

    static func refuseOwnWindow(at point: CGPoint) throws {
        try ShareControlPanel.refusePoint(point)
        try refuseOwn(ownerPid(at: point), at: point)
    }

    /// The policy itself, for whoever already knows whose window is under the
    /// point (a reflex run's press asks it too, `DesktopRunBoundary`).
    static func refuseOwn(_ pid: pid_t?, at point: CGPoint, isOwn: (pid_t) -> Bool = DesktopSelf.isOwn) throws {
        if let pid { try ShareControlPanel.refuseHelper(pid: pid) }
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
        func label(_ record: ElementRecord?) throws -> String? {
            if params["background"]?.bool == true {
                guard let record,
                      let text = ConfirmationTarget.backgroundLabel(role: record.role, actions: record.actions,
                                                                    label: faceLabel(record.element)) else {
                    throw ProviderError.coded("element_not_found", "the background confirmation target is not a readable control; observe again before retrying; no input was sent")
                }
                return text
            }
            return record.flatMap { faceLabel($0.element) }
        }
        switch method {
        case "click", "performSecondaryAction":
            let snapshot = try loadSnapshot()
            if let index = try optionalInteger(params, "elementIndex") {
                return try label(element(snapshot, index))
            }
            // A click by reading is held for what it would press; one that
            // cannot choose is refused by the press itself, not asked about.
            let query = readingQuery(params)
            if method == "click", !query.isEmpty {
                guard let index = try? chosenIndex(query, in: snapshot) else { return nil }
                return try label(element(snapshot, index))
            }
            // A click at a window's point presses whatever is under it — read
            // it there, as a desktop click is read: a saved coordinate click
            // on "Place order" is the person's last step too.
            guard method == "click" else { return nil }
            let point = try coordinatePoint(params: params, xKey: "x", yKey: "y", snapshot: snapshot)
            if params["background"]?.bool == true {
                return try label(backgroundClickRecord(at: point, in: snapshot))
            }
            return faceLabelAtPoint(point, in: snapshot.app.pid)
        case "mouseClick":
            return faceLabelAtPoint(try desktopPoint(params, "x", "y"))
        case "mouseDrag":
            // A drag is a click on what it lets go of: a control fires on the
            // mouse-up only where the pointer is released.
            return faceLabelAtPoint(try desktopPoint(params, "toX", "toY"))
        case "key", "holdKey", "pressKey", "hotkey":
            guard let chord = params["key"]?.string, firesDefaultButton(chord) else { return nil }
            let focused: AXUIElement?
            let window: AXUIElement?
            if method == "pressKey" || method == "hotkey" {
                let snapshot = try loadSnapshot()
                focused = params["background"]?.bool == true ? backgroundFocusedElement(snapshot) : focusedRecord(snapshot)?.element
                window = snapshot.rootIndex.flatMap { snapshot.elements[$0]?.element }
            } else {
                focused = systemWideFocusedElement()
                window = focused.flatMap(containingWindow)
            }
            let button = window.flatMap { copyElement($0, kAXDefaultButtonAttribute as String) }
            return ConfirmationTarget.labels(focused: focused.flatMap { faceLabel($0) }, defaultButton: button.flatMap { faceLabel($0) })
        default:
            return nil
        }
    }
}

import CoreGraphics
import Foundation

/// What a press's refusal says when the look it chose from did not show the
/// control: an element that shows the words only in its text, whether that
/// look read the whole window, and the app's other windows on screen, where
/// the control may stand instead. On 11-20 a click
/// by label read one Chrome window's dashboard while the button may
/// have stood in another Chrome window at the same place; the refusal named
/// neither, and the model went on to pixels and the person's pointer
/// (t-121372). The refusal itself stays: nothing is pressed, and nothing is
/// chosen for the caller.
public enum ReadingRefusal {
    /// One of the app's windows other than the one the look read.
    public struct OtherWindow: Equatable, Sendable {
        public let windowId: CGWindowID
        public let title: String?

        public init(windowId: CGWindowID, title: String?) {
            self.windowId = windowId
            self.title = title
        }

        /// `--window-id 7 «Inbox»`, or the number alone when it shows no title.
        var said: String {
            let shown = ReadingRefusal.clipped(title ?? "")
            return shown.isEmpty ? "--window-id \(windowId)" : "--window-id \(windowId) «\(shown)»"
        }
    }

    /// How many other windows one refusal names.
    public static let mostOtherWindows = 3

    /// How many characters one item of a refusal — a window's title, an
    /// element's text — keeps, so that a long title cannot make it long.
    public static let mostItemCharacters = 80

    /// An item as a refusal names it: trimmed, and cut with "…" past
    /// `mostItemCharacters`.
    static func clipped(_ text: String) -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count > mostItemCharacters else { return trimmed }
        return String(trimmed.prefix(mostItemCharacters - 1)) + "…"
    }

    /// Why a look did not read the whole window, as a refusal says it; nil
    /// when it read all of it.
    public static func cutWords(truncated: Bool, cutShort: String?, maxDepthReached: Bool, read: Int, maxNodes: Int) -> String? {
        if cutShort == WebTreeSettling.buildingCode {
            return "this look caught the app still building its web tree"
        }
        guard truncated else { return nil }
        switch cutShort {
        case "time_budget":
            return "this look stopped at its time budget after \(read) elements, before the end of the window"
        case "app_unresponsive":
            return "the app stopped answering after \(read) elements, before the end of the window"
        default:
            return maxDepthReached
                ? "this look read no deeper than its depth limit, so deeper elements were not read"
                : "this look stopped at its \(maxNodes)-element limit, before the end of the window"
        }
    }

    /// The app's other windows, as a refusal names them; nil when it has none.
    public static func otherWindowsWords(app: String, others: [OtherWindow]) -> String? {
        guard !others.isEmpty else { return nil }
        let named = others.prefix(mostOtherWindows).map(\.said).joined(separator: ", ")
        let more = others.count > mostOtherWindows ? " and \(others.count - mostOtherWindows) more" : ""
        return "\(app) has other windows on screen: \(named)\(more); the control may be in one of them — look at it by its --window-id"
    }

    /// How many elements one refusal names as showing the words.
    public static let mostShown = 3

    /// What a person reads of an element by the name a look gave it: a link's
    /// `[text](address)` (`SnapshotRenderHeuristics.displayName`) is its text,
    /// unescaped, and never its address; any other name is itself.
    public static func shownText(role: String?, name: String) -> String {
        guard role == "AXLink", name.hasPrefix("[") else { return name }
        var text = ""
        var rest = name.dropFirst()
        while let glyph = rest.first {
            rest = rest.dropFirst()
            switch glyph {
            case "\\":
                guard let escaped = rest.first else { return name }
                text.append(escaped)
                rest = rest.dropFirst()
            case "]":
                return rest.first == "(" ? text : name
            default:
                text.append(glyph)
            }
        }
        return name
    }

    /// One element as the check below weighs it: what a click by reading
    /// matches of it, and the name a look shows it by.
    public struct Shown: Equatable {
        public let face: ElementFace
        public let name: String

        public init(face: ElementFace, name: String) {
            self.face = face
            self.name = name
        }
    }

    /// The elements whose shown text holds every asked word while nothing a
    /// click by reading matches of them — label, value, description — holds
    /// any: a link named by its contents, a row by its cells. A link's address
    /// is not its text. An asked role narrows them as it narrows the click.
    /// Each comes back with its shown text as its label.
    ///
    /// None whose shown text holds one of the person's confirmation words
    /// (`guardWords`, the table `confirmKind` reads) is offered: the check that
    /// holds a payment, transfer or delete press reads a control's first name,
    /// not its shown text, so a link named «Go» that shows «Delete project»
    /// could be pressed unasked (t-121539 I-4, an older gap). A refusal never
    /// sends a model to one (t-121849).
    public static func shownOnly(_ query: ElementQuery, among elements: [Shown], guardWords: [String: [String]]? = nil) -> [ElementFace] {
        let words = [query.label, query.text].compactMap { $0 }
        guard !words.isEmpty else { return [] }
        return elements.compactMap { element in
            let face = element.face
            if let role = query.role, query.normalizedRole(role) != query.normalizedRole(face.role ?? "") { return nil }
            let matched = [face.label, face.value, face.description].compactMap { $0 }
            if words.contains(where: { word in matched.contains { $0.localizedCaseInsensitiveContains(word) } }) { return nil }
            let text = shownText(role: face.role, name: element.name)
            guard words.allSatisfy({ text.localizedCaseInsensitiveContains($0) }) else { return nil }
            if let guardWords, confirmKind(of: text, words: guardWords) != nil { return nil }
            return ElementFace(index: face.index, role: face.role, label: text, value: nil, description: nil)
        }
    }

    /// The elements `shownOnly` found, as a refusal names them; nil when none
    /// does. The click still chooses nothing: a press names one by number.
    public static func shownWords(_ faces: [ElementFace]) -> String? {
        guard !faces.isEmpty else { return nil }
        let named = faces.prefix(mostShown)
            .map { ElementFace(index: $0.index, role: $0.role, label: $0.label.map(clipped), value: nil, description: nil).said }
            .joined(separator: "; ")
        let more = faces.count > mostShown ? " and \(faces.count - mostShown) more" : ""
        return "these words show only in the text of \(named)\(more), which a click by words does not read: press one by its --element-index"
    }

    /// A click by reading that no control of the look answers: the words it
    /// always said, then why the look may have missed the control.
    public static func noControl(app: String, windowTitle: String, words: String, shown: String?, cut: String?, others: String?) -> String {
        ["no control in \(app)'s window «\(windowTitle)» reads \(words); look with --marks, or find", shown, cut, others]
            .compactMap { $0 }
            .joined(separator: "; ")
    }

    /// The number a refusal names one of the app's windows by: the one the
    /// window publishes (`AXWindowNumber`), else the window-server number the
    /// accessibility library links it to. Chrome's windows and the twin
    /// fixture publish none, so a refusal that read the published number
    /// alone never named their windows (t-122948). The link is asked only
    /// when nothing is published, and only for the refusal's words.
    public static func windowID(published: CGWindowID?, linked: () -> CGWindowID?) -> CGWindowID? {
        published ?? linked()
    }

    /// A press whose control is under something at its centre (`requireOnTop`).
    /// Under another window of the same app — the twin windows of one app the
    /// person's scene had — the refusal says which window, and that moving it
    /// aside clears the control: a window move takes no pointer, key or front
    /// app (`ComputerMethod::takes_the_desk`), and the helper's move only sets
    /// the window's position. Bringing a window to the front is not offered
    /// first. Anything else keeps the words it always had (t-122111 Q2).
    public static func covered(index: Int, by covering: String, otherWindow: OtherWindow?) -> String {
        let shown = "element \(index) is covered at its centre\(covering.isEmpty ? "" : " by \(covering)")"
        guard let otherWindow else { return "\(shown); look again with --marks" }
        return "\(shown), which stands in another window of this app (\(otherWindow.said)): move that window aside in the background — a window move of its --window-id takes no pointer and raises nothing — then look again and press; if it will not move, ask the person"
    }

    /// Why a background press's target could not be read for its confirmation.
    public enum Unreadable: Equatable, Sendable {
        /// The point stands in another of the app's windows than the look read.
        case anotherWindow(OtherWindow)
        /// The look stopped before it reached what stands there.
        case notReached(String)
        /// What stands there is no control with a label and a press of its own.
        case notAControl
    }

    /// The background confirmation's refusal. A container, an unlabelled
    /// control or one that offers no press is refused in the words it always was.
    public static func backgroundTarget(_ why: Unreadable) -> String {
        let refused = "the background confirmation target is not a readable control"
        switch why {
        case let .anotherWindow(window):
            return "\(refused): the point is in another of the app's windows (\(window.said)) than the one this look read; look at that window by its --window-id and press there; no input was sent"
        case let .notReached(cut):
            return "\(refused): \(cut), so what stands at the point was not read; observe again before retrying; no input was sent"
        case .notAControl:
            return "\(refused); observe again before retrying; no input was sent"
        }
    }
}

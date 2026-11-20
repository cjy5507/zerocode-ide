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
            let shown = title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return shown.isEmpty ? "--window-id \(windowId)" : "--window-id \(windowId) «\(shown)»"
        }
    }

    /// How many other windows one refusal names.
    public static let mostOtherWindows = 3

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
                ? "this look stopped at its depth limit, so deeper elements were not read"
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

    /// The elements a look shows with the asked words in their shown name — a
    /// link named by its text, a row by its cells — while their label and
    /// description, which a click by reading matches, lack them; nil when
    /// none does. The click still chooses nothing: a press names one by number.
    public static func shownWords(_ faces: [ElementFace]) -> String? {
        guard !faces.isEmpty else { return nil }
        let named = faces.prefix(mostShown).map(\.said).joined(separator: "; ")
        let more = faces.count > mostShown ? " and \(faces.count - mostShown) more" : ""
        return "these words show in the text of \(named)\(more), not in a label or description: press one by its --element-index"
    }

    /// A click by reading that no control of the look answers: the words it
    /// always said, then why the look may have missed the control.
    public static func noControl(app: String, windowTitle: String, words: String, shown: String?, cut: String?, others: String?) -> String {
        ["no control in \(app)'s window «\(windowTitle)» reads \(words); look with --marks, or find", shown, cut, others]
            .compactMap { $0 }
            .joined(separator: "; ")
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

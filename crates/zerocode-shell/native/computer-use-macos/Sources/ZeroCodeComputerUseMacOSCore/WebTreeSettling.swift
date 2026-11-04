import Foundation

/// A Chromium or Electron app builds its web tree a while after it was asked to
/// (`WebAccessibility`): the attribute reads back as set at once, and until the
/// tree stands the window shows only its frame — its close, minimise and zoom
/// buttons (t-93286). A look in that time reads the window again, briefly,
/// instead of answering the frame as the app's whole content; one that still
/// sees only the frame says so, so the agent looks again rather than turning to
/// the screen's pixels or the person's pointer.
public enum WebTreeSettling {
    /// How long one look reads the window again for its tree to stand.
    public static let waitSeconds: TimeInterval = 2
    /// What `truncation.cutShort` says when the tree was still building.
    public static let buildingCode = "web_tree_building"
    /// The controls a window's own frame holds.
    public static let frameSubroles: Set<String> = ["AXCloseButton", "AXMinimizeButton", "AXZoomButton", "AXFullScreenButton"]
    /// The containers a tree not built yet shows empty.
    public static let emptyContentRoles: Set<String> = ["AXWebArea", "AXScrollArea", "AXGroup", "AXSplitGroup", "AXUnknown"]
    /// A window whose look holds more parts than this shows content: none of
    /// them is read again to tell.
    public static let mostFrameParts = 8

    /// One element a look showed inside the window, as far as telling the
    /// frame from content needs.
    public struct Part: Equatable, Sendable {
        public let role: String
        public let subrole: String?
        /// No name and nothing inside it.
        public let empty: Bool

        public init(role: String, subrole: String?, empty: Bool) {
            self.role = role
            self.subrole = subrole
            self.empty = empty
        }
    }

    /// Whether every part is the window's frame or an empty container: the
    /// app's content is not in the tree yet.
    public static func isFrameOnly(_ parts: [Part]) -> Bool {
        guard parts.count <= mostFrameParts else { return false }
        return parts.allSatisfy { part in
            part.subrole.map(frameSubroles.contains) == true || (part.empty && emptyContentRoles.contains(part.role))
        }
    }

    /// A look, and whether the tree was still building when it gave up.
    public struct Settled<Look> {
        public let look: Look
        public let building: Bool
        /// How many times the window was read.
        public let reads: Int
        /// How long the look waited for the tree, in seconds.
        public let waitedSeconds: TimeInterval
    }

    /// Read the window; while an app whose web tree was switched on at
    /// `switchedOnAt` — within `WebAccessibility.settlesSeconds` — shows only
    /// its frame, pause and read again, for `waitSeconds` at most. An app never
    /// switched on (a native app) is read once.
    public static func settle<Look>(
        switchedOnAt: Date?,
        now: () -> Date,
        read: () -> Look,
        frameOnly: (Look) -> Bool,
        pause: () -> Void,
        stopped: () -> Bool
    ) -> Settled<Look> {
        let started = now()
        var look = read()
        var reads = 1
        guard let switchedOnAt,
              started < switchedOnAt.addingTimeInterval(WebAccessibility.settlesSeconds),
              frameOnly(look)
        else {
            return Settled(look: look, building: false, reads: reads, waitedSeconds: 0)
        }
        let giveUp = started.addingTimeInterval(waitSeconds)
        while !stopped(), now() < giveUp {
            pause()
            look = read()
            reads += 1
            if !frameOnly(look) {
                return Settled(look: look, building: false, reads: reads, waitedSeconds: now().timeIntervalSince(started))
            }
        }
        return Settled(look: look, building: true, reads: reads, waitedSeconds: now().timeIntervalSince(started))
    }

    /// The line a look that still sees only the frame ends its tree with.
    public static func buildingLine(app: String) -> String {
        "\(app) is still building its web tree: this look shows only the window's frame. Look again in a moment before reading the screen's pixels or taking the pointer."
    }
}

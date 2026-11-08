import Foundation

/// Which lane a request waits in: one per app, `desktop` for what acts through
/// the shared pointer and keys with no app of its own, or none at all for the
/// reads that touch no app and no input.
///
/// A press the window borrows the desktop for is aimed at one app's window
/// (`borrowApp`: a press by the words a mirrored phone shows, 2026-11-09). It
/// waits in that app's lane, behind the keys going into the same app — not in
/// the desktop lane, where it held up every app-less look of every other
/// agent for the time it took: the person's wait, the press and the pointer
/// put back.
public enum RequestLane {
    /// The name of the lane every app-less act shares.
    public static let desktop = "desktop"

    /// The reads that touch no app's tree and send no input: a picture of the
    /// screen, the displays, where the pointer is, the windows on screen. They
    /// never wait behind an act.
    public static let unlanedReads: Set<String> = [
        "screenshotDesktop", "displays", "cursorPosition", "listAllWindows",
    ]

    /// The lane for a request, or `nil` for one that waits in none. `resolve`
    /// turns an app as it was written into the lane an app runs in (`pid:N`),
    /// or `nil` when no running app answers to it.
    public static func name(
        method: String,
        app: String?,
        windowOwner: Int32?,
        borrowApp: String?,
        resolve: (String) -> String?
    ) -> String? {
        if let app = written(app) {
            return resolve(app) ?? "app:\(app.lowercased())"
        }
        if let owner = windowOwner {
            return "pid:\(owner)"
        }
        if let borrowed = written(borrowApp) {
            return resolve(borrowed) ?? "app:\(borrowed.lowercased())"
        }
        if unlanedReads.contains(method) {
            return nil
        }
        return desktop
    }

    private static func written(_ value: String?) -> String? {
        guard let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty else {
            return nil
        }
        return trimmed
    }
}

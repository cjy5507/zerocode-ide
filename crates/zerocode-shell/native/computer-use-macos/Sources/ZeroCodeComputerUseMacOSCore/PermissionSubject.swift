import Foundation

/// Which process a System Settings permission list judges. Accessibility
/// looks at the caller — this helper. Screen Recording looks at the
/// responsible process — the app that opened the helper — so its row wears
/// the app's name and the tile a person drags in must be the app bundle.
public enum PermissionSubject: Equatable {
    case helper
    case app
}

public enum PermissionSubjectResolver {
    /// The bundle to drag into the list for `subject`: the nearest `.app`
    /// above the helper bundle, or the helper itself when it stands alone
    /// (a dev build is its own responsible process).
    public static func judgedBundleURL(subject: PermissionSubject, helperURL: URL) -> URL {
        guard subject == .app else { return helperURL }
        var candidate = helperURL.deletingLastPathComponent()
        while candidate.path != "/" && !candidate.path.isEmpty {
            if candidate.pathExtension == "app" { return candidate }
            let parent = candidate.deletingLastPathComponent()
            if parent.path == candidate.path { break }
            candidate = parent
        }
        return helperURL
    }
}

import Foundation

/// Where an application named by a person may live, in the order a launch
/// looks: a path is itself; anything else is tried as `<name>.app` under each
/// root, then as the bare name. Nothing here touches the disk — the caller
/// keeps the first candidate that exists.
public func applicationCandidateURLs(query: String, roots: [URL]) -> [URL] {
    let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return [] }
    if trimmed.hasPrefix("/") || trimmed.hasPrefix("~") {
        return [URL(fileURLWithPath: (trimmed as NSString).expandingTildeInPath)]
    }
    let leaf = trimmed.hasSuffix(".app") ? String(trimmed.dropLast(4)) : trimmed
    return roots.flatMap { root in
        [root.appendingPathComponent("\(leaf).app"), root.appendingPathComponent(leaf)]
    }
}

/// Whether a query names an application by its bundle identifier rather than
/// its name: reverse-DNS words, no spaces, no slashes.
public func looksLikeBundleIdentifier(_ query: String) -> Bool {
    // Empty parts count: "a..b" is not a bundle identifier.
    let parts = query.split(separator: ".", omittingEmptySubsequences: false)
    return parts.count >= 2
        && !query.contains(" ")
        && !query.contains("/")
        && parts.allSatisfy { !$0.isEmpty }
}

/// The window actions a person has on any window, by the name the verb sends.
public enum DesktopWindowAction: String, CaseIterable {
    case focus
    case move
    case resize
    case minimize
    case zoom
    case close

    public var movesTheWindow: Bool { self == .move || self == .resize }
}

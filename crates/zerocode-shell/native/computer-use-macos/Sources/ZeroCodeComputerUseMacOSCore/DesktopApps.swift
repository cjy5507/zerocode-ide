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

/// The other names an application answers to, read off the app itself and
/// never guessed — the core's `identity::matches` table
/// (`MACOS_BUNDLE_NAME_KEYS`; a source contract in the shell holds the
/// spelling): its bundle's own unlocalized `CFBundleDisplayName` and
/// `CFBundleName` — the `Calculator` a person types while the running app
/// calls itself `계산기` — and its executable's file stem. In that order,
/// without repeats.
public let bundleNameKeys = ["CFBundleDisplayName", "CFBundleName"]

/// The bundle's own name — the first of `bundleNameKeys` it states — which
/// `launch` answers as `bundleName` so the next verb can ask by that word.
public func bundleName(info: [String: Any]?) -> String? {
    for key in bundleNameKeys {
        if let name = info?[key] as? String, !name.isEmpty { return name }
    }
    return nil
}

public func applicationOtherNames(info: [String: Any]?, executableURL: URL?) -> [String] {
    var names: [String] = []
    for key in bundleNameKeys {
        if let name = info?[key] as? String, !name.isEmpty, !names.contains(name) {
            names.append(name)
        }
    }
    if let stem = executableURL?.deletingPathExtension().lastPathComponent, !stem.isEmpty, !names.contains(stem) {
        names.append(stem)
    }
    return names
}

/// `identity::matches`: the name or the identifier, case-insensitively, or
/// one of the other names — never a fragment. The other names are read only
/// when the first two miss: they cost a bundle's `Info.plist`.
public func applicationAnswers(
    to query: String,
    name: String,
    bundleId: String?,
    otherNames: @autoclosure () -> [String]
) -> Bool {
    let same = { (candidate: String) in candidate.caseInsensitiveCompare(query) == .orderedSame }
    if same(name) || bundleId.map(same) == true { return true }
    return otherNames().contains(where: same)
}

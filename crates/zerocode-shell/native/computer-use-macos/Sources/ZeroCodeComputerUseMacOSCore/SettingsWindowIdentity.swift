import Foundation

/// Whether a window on screen belongs to System Settings.
///
/// Asked of the window's OWNER and answered from its bundle identifier —
/// never from the name the window list carries. `kCGWindowOwnerName` is the
/// **localized** display name: on a Korean machine System Settings owns its
/// windows as "시스템 설정", so a comparison against "System Settings" says no
/// on every machine that is not in English.
///
/// That answer is load-bearing twice over in the permission assistant, which
/// is why one wrong word took the whole surface down (live report 2026-08-25,
/// "열기를 누르면 설정창이 열리는게 아니고 [Dock 아이콘]이 열림"): the
/// assistant only shows itself once it has FOUND the settings window to sit
/// beside, and it only terminates once it has SEEN that window and then lost
/// it. Never recognising it means a helper that shows nothing and never
/// exits — one stray process in the Dock per press.
///
/// The English names stay as a fallback for the one case the identifier
/// cannot answer: a window whose owning process has already gone by the time
/// it is asked about has no `NSRunningApplication` left to ask. An identifier
/// that IS known and does not match settles it — a window is not System
/// Settings because something else was named like it.
public enum SettingsWindowIdentity {
    /// Both identifiers the settings app has shipped under.
    public static let bundleIdentifiers: Set<String> = [
        "com.apple.systempreferences",
        "com.apple.SystemSettings",
    ]

    /// The English display names, for a window whose owner is already gone.
    public static let fallbackOwnerNames: Set<String> = [
        "System Settings",
        "System Preferences",
    ]

    public static func isSettings(bundleIdentifier: String?, ownerName: String?) -> Bool {
        if let bundleIdentifier {
            return bundleIdentifiers.contains(bundleIdentifier)
        }
        guard let ownerName else { return false }
        return fallbackOwnerNames.contains(ownerName)
    }
}

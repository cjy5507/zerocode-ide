import Foundation

/// The build embeds the same permission table that the Rust owner reads.
public struct PermissionSettingsTarget: Decodable, Sendable {
    public let id: String
    public let settings_url: String

    public static func settingsURL(id: String, data: Data) -> String? {
        let targets = try? JSONDecoder().decode([Self].self, from: data)
        return targets?.first(where: { $0.id == id })?.settings_url
    }
}

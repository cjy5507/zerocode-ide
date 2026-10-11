import Foundation

public enum ConfirmationTarget {
    public static func backgroundLabel(role: String, actions: [String], label: String?) -> String? {
        guard MarkPin.textEntryRoles.contains(role)
                || BackgroundInput.clickAction(actions: actions, textEntry: false, plainLeftClick: true) != nil,
              let label, !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return label
    }

    public static func labels(focused: String?, defaultButton: String?) -> String? {
        let labels = [focused, defaultButton].compactMap { $0 }.filter { !$0.isEmpty }
        return labels.isEmpty ? nil : labels.joined(separator: "\n")
    }
}

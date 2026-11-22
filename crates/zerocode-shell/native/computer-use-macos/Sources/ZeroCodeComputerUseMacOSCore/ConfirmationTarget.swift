import Foundation

public enum ConfirmationTarget {
    /// What a background press reads before it goes: a field's or a control's
    /// own label, or a row's that a click chooses (`selectable`) — never the
    /// label of a window or a page around what was hit.
    public static func backgroundLabel(role: String, actions: [String], label: String?, selectable: @autoclosure () -> Bool = false) -> String? {
        guard MarkPin.textEntryRoles.contains(role)
                || BackgroundInput.clickAction(actions: actions, textEntry: false, plainLeftClick: true) != nil
                || selectable(),
              let label, !label.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return label
    }

    public static func labels(focused: String?, defaultButton: String?) -> String? {
        let labels = [focused, defaultButton].compactMap { $0 }.filter { !$0.isEmpty }
        return labels.isEmpty ? nil : labels.joined(separator: "\n")
    }

    /// What an app's own action on a control lands on, for the person's last
    /// step: the control's label, the name of an action the app named itself
    /// (a row's "Delete"), and — for a confirm, which fires what a Return
    /// fires — the default button of the dialog around it.
    public static func secondaryActionLabels(action: String, label: String?, defaultButton: String?) -> String? {
        let named = action.hasPrefix("AX") ? nil : customActionName(action)
        let fired = action == "AXConfirm" ? defaultButton : nil
        let labels = [label, named, fired].compactMap { $0 }
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        return labels.isEmpty ? nil : labels.joined(separator: "\n")
    }

    /// The name an app gave its own action: macOS spells one as
    /// `Name:<name>` then its target and selector, a line each.
    public static func customActionName(_ raw: String) -> String {
        let first = raw.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? raw
        return first.hasPrefix("Name:") ? String(first.dropFirst("Name:".count)) : first
    }
}

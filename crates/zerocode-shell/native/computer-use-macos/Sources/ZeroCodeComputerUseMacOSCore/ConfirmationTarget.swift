public enum ConfirmationTarget {
    public static func labels(focused: String?, defaultButton: String?) -> String? {
        let labels = [focused, defaultButton].compactMap { $0 }.filter { !$0.isEmpty }
        return labels.isEmpty ? nil : labels.joined(separator: "\n")
    }
}

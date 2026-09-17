import Foundation

/// The kinds a person confirms, in the order the window's table names them
/// (docs/design/computer-use-full-operator.md §1.5). The words come with
/// every guarded request from the window — the table lives there, once.
public let confirmKindOrder = ["payment", "transfer", "delete"]

/// Which kind a control's label answers to under the given words, if any:
/// a case-insensitive fragment, the first kind in the order that matches.
public func confirmKind(of label: String?, words: [String: [String]]) -> String? {
    guard let label, !label.isEmpty else { return nil }
    let shown = label.lowercased()
    return confirmKindOrder.first { kind in
        (words[kind] ?? []).contains { !$0.isEmpty && shown.contains($0.lowercased()) }
    }
}

/// Whether a key chord ends on the key that fires a window's default button.
public func firesDefaultButton(_ chord: String) -> Bool {
    let last = chord.split(separator: "+").last.map { $0.trimmingCharacters(in: .whitespaces).lowercased() } ?? ""
    return last == "return" || last == "enter"
}

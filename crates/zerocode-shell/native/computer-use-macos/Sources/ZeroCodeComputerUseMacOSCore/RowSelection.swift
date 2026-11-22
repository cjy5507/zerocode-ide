import Foundation

/// What a person's click on a row of a list chooses, behind them (t-125705):
/// the row alone, the row added or taken away (cmd), or the run from the one
/// chosen row to it (shift). Rows are told by their place in the list; the
/// helper writes the result through the list's own selection and reads it
/// back, so a row that offers no press (a Finder row) is chosen all the same.
public enum RowSelection {
    public enum Pick: Equatable, Sendable {
        case only
        case toggle
        case run
    }

    /// The roles a click inside walks up to: a table's or outline's row, a
    /// cell that is chosen by itself, an icon view's item.
    public static let itemRoles: Set<String> = ["AXRow", "AXCell", "AXImage"]

    /// How far up from the control clicked the chosen item may stand.
    public static let itemDepth = 8

    /// A list longer than this is not read row by row for one click.
    public static let maxRows = 5_000

    /// The pick a click's modifiers make, or nil for one a person's click
    /// does not choose by (option, control, or cmd and shift together).
    public static func pick(command: Bool, shift: Bool, others: Bool) -> Pick? {
        switch (command, shift, others) {
        case (false, false, false): return .only
        case (true, false, false): return .toggle
        case (false, true, false): return .run
        default: return nil
        }
    }

    /// Whether the click may still go on by its window road after the list's
    /// selection was written: only when the app refused the write outright,
    /// so nothing was chosen. An error or no answer may have chosen the rows
    /// all the same — the rule a press's answer follows (`AXActionVerdict`) —
    /// and a click after it would land a second time (t-125705 R6).
    public static func clickGoesOn(after written: AXActionVerdict) -> Bool {
        written == .unsupported
    }

    /// Whether the rows read back after the write are the rows wanted: nil
    /// when they could not be read, which proves nothing — an unread list is
    /// not an empty one (t-125705 R6).
    public static func chosenAsWanted<Row>(_ wanted: [Row], read: [Row]?, same: (Row, Row) -> Bool) -> Bool? {
        guard let read else { return nil }
        return read.count == wanted.count && wanted.allSatisfy { row in read.contains { same($0, row) } }
    }

    /// The places chosen after the pick, in list order. A run starts at the
    /// one row chosen before it; with several chosen, where a person's run
    /// would start is not known here, and nil says so.
    public static func after(_ pick: Pick, target: Int, chosen: [Int]) -> [Int]? {
        let before = Array(Set(chosen)).sorted()
        switch pick {
        case .only:
            return [target]
        case .toggle:
            return before.contains(target) ? before.filter { $0 != target } : (before + [target]).sorted()
        case .run:
            guard before.count <= 1 else { return nil }
            guard let anchor = before.first else { return [target] }
            return Array(min(anchor, target)...max(anchor, target))
        }
    }
}

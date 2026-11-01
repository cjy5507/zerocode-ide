import Foundation

/// What an act changed in a window, told in lines instead of the whole tree.
///
/// An action's answer used to carry the window's whole tree. On a 1,016-element
/// window that was 23.8 KB, about six thousand tokens the model read after every
/// press. Numbers now outlive a look, so an answer only has to say what is new.
public struct ElementChanges: Equatable, Sendable {
    public var added: [String]
    public var changed: [String]
    public var removed: [Int]
    /// How many lines the limits left out. Above zero, a full look shows them.
    public var omitted: Int

    public init(added: [String] = [], changed: [String] = [], removed: [Int] = [], omitted: Int = 0) {
        self.added = added
        self.changed = changed
        self.removed = removed
        self.omitted = omitted
    }

    public var isEmpty: Bool { added.isEmpty && changed.isEmpty && removed.isEmpty && omitted == 0 }

    /// How many lines, and how many bytes of them, one answer carries.
    public static var lineLimit: Int { 60 }
    public static var byteLimit: Int { 6_000 }

    /// The lines of `current` that differ from `previous`. Both map an element's
    /// number to its line; `order` is `current`'s reading order, so what is new
    /// reads top to bottom. The element's own number comes first on every line.
    public static func between(
        previous: [Int: String],
        current: [Int: String],
        order: [Int],
        lineLimit: Int = ElementChanges.lineLimit,
        byteLimit: Int = ElementChanges.byteLimit
    ) -> ElementChanges {
        var changes = ElementChanges()
        var bytes = 0
        var seen = Set<Int>()
        for number in order where seen.insert(number).inserted {
            guard let line = current[number] else { continue }
            let old = previous[number]
            guard old != line else { continue }
            let room = changes.added.count + changes.changed.count < lineLimit && bytes + line.utf8.count <= byteLimit
            guard room else {
                changes.omitted += 1
                continue
            }
            bytes += line.utf8.count
            if old == nil {
                changes.added.append(line)
            } else {
                changes.changed.append(line)
            }
        }
        for number in previous.keys.sorted() where current[number] == nil {
            if changes.removed.count < lineLimit {
                changes.removed.append(number)
            } else {
                changes.omitted += 1
            }
        }
        return changes
    }
}

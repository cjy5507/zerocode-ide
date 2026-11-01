import Foundation

/// Hands out numbers. Every process of one app draws from one, so a number kept
/// from before a relaunch can never name a control of the new process.
public final class ElementNumbers: @unchecked Sendable {
    private let lock = NSLock()
    private var next = 0

    public init() {}

    public func take() -> Int {
        lock.lock()
        defer { lock.unlock() }
        let number = next
        next += 1
        return number
    }
}

/// The number an element keeps for as long as the app keeps the element.
///
/// A number used to be the element's place in the walk, so a row arriving above
/// moved every number below it and a removed row handed its number to the row
/// after it: pressing a number could press a different control, and the answer
/// said it worked. A number here names one native object. It is never reused, a
/// removed element's number simply stops resolving, and every look and every
/// agent read the same number for the same control.
///
/// `Element` is generic so the rule runs in a test with no accessibility API;
/// the helper hands in `CFHash` and `CFEqual`, which ask the app nothing.
public final class ElementIdentityBook<Element> {
    private struct Entry {
        let element: Element
        let number: Int
        var root: String
        var lastSeenWalk: UInt64
    }

    /// Past this many remembered elements the oldest are forgotten, so one app
    /// that makes a new object for every repaint cannot grow the book for ever.
    public static var maxEntries: Int { 5_000 }

    private let hash: (Element) -> UInt
    private let equal: (Element, Element) -> Bool
    private var buckets: [UInt: [Entry]] = [:]
    private var count = 0
    private let numbers: ElementNumbers
    private var walk: UInt64 = 0

    public init(
        numbers: ElementNumbers = ElementNumbers(),
        hash: @escaping (Element) -> UInt,
        equal: @escaping (Element, Element) -> Bool
    ) {
        self.numbers = numbers
        self.hash = hash
        self.equal = equal
    }

    /// How many elements the book remembers now.
    public var remembered: Int { count }

    /// Begin one walk: the elements it does not see can be forgotten at its end.
    public func beginWalk() {
        walk &+= 1
    }

    /// The element's number, given a new one when the book has not met it.
    /// `root` names the window or surface being walked.
    public func number(for element: Element, root: String) -> Int {
        let key = hash(element)
        if let at = buckets[key]?.firstIndex(where: { equal($0.element, element) }) {
            buckets[key]?[at].root = root
            buckets[key]?[at].lastSeenWalk = walk
            return buckets[key]?[at].number ?? 0
        }
        let number = numbers.take()
        buckets[key, default: []].append(Entry(element: element, number: number, root: root, lastSeenWalk: walk))
        count += 1
        return number
    }

    /// The number the book already gave `element`, without giving a new one.
    public func existingNumber(for element: Element) -> Int? {
        buckets[hash(element)]?.first(where: { equal($0.element, element) })?.number
    }

    /// End a complete walk of `root`: its elements this walk did not see are
    /// gone. Answers their numbers. A walk cut short proves nothing about what
    /// it did not reach, and its caller does not call this.
    @discardableResult
    public func endCompleteWalk(root: String) -> [Int] {
        var gone: [Int] = []
        for key in Array(buckets.keys) {
            guard let bucket = buckets[key] else { continue }
            let kept = bucket.filter { entry in
                guard entry.root == root, entry.lastSeenWalk != walk else { return true }
                gone.append(entry.number)
                return false
            }
            buckets[key] = kept.isEmpty ? nil : kept
        }
        count -= gone.count
        return gone + trimToLimit()
    }

    /// End a walk that was cut short. It proves nothing gone, but an app that is
    /// always cut short must not grow the book for ever. Answers the numbers forgotten.
    @discardableResult
    public func endCutWalk() -> [Int] {
        trimToLimit()
    }

    /// Forget what was seen longest ago, so the window in use keeps its numbers.
    private func trimToLimit() -> [Int] {
        guard count > Self.maxEntries else { return [] }
        let entries = buckets.values.flatMap { $0 }.sorted {
            ($0.lastSeenWalk, $0.number) < ($1.lastSeenWalk, $1.number)
        }
        let excess = entries.prefix(count - Self.maxEntries)
        let forgotten = Set(excess.map(\.number))
        for key in Array(buckets.keys) {
            let kept = buckets[key]?.filter { !forgotten.contains($0.number) } ?? []
            buckets[key] = kept.isEmpty ? nil : kept
        }
        count -= forgotten.count
        return Array(forgotten)
    }
}

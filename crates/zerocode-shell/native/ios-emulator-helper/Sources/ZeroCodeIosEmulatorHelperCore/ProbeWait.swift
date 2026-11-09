/// What the field showed while the two probe keys were waited on.
enum ProbeSighting<State> {
    /// The keys' signature showed: the keyboard's state. Only now may the keys be deleted.
    case recognized(State)
    /// Nothing on the screen changed before the wait ran out: a field that takes no letters
    /// (digits, a secure or a full one), or the keys never arrived.
    case unchanged
    /// The screen changed but never into the keys' signature: another value moved — a page, a
    /// spinner — or the field wrote something no signature reads. Deleting two characters here
    /// could take the person's.
    case unrecognized
}

extension ProbeSighting: Equatable where State: Equatable {}

/// The wait for the probe keys to show (the simulator helper's `probeLanguage`).
///
/// A native field shows the keys at the first read; a web page's field only once the page has
/// told the accessibility tree, later than 150 ms in Safari (Phone Harness P2, 2026-11-09: a shop's
/// search box read as unchanged every time, the keys stayed in it, and the errand took 45 model
/// round trips). So the screen is read until `recognize` answers or the wall clock passes `within`
/// — a clock, not a count: one read of the tree can take longer than the gap between reads.
enum ProbeWait {
    static let within: Double = 1.5
    static let gap: Double = 0.15

    static func wait<State>(
        before: [String],
        within: Double = ProbeWait.within,
        gap: Double = ProbeWait.gap,
        now: () -> Double,
        sleep: (Double) -> Void,
        read: () -> [String]?,
        recognize: ([String], [String]) -> State?
    ) -> ProbeSighting<State> {
        let deadline = now() + within
        var changed = false
        repeat {
            sleep(gap)
            guard let after = read() else { continue }
            if let state = recognize(before, after) { return .recognized(state) }
            if after != before { changed = true }
        } while now() < deadline
        return changed ? .unrecognized : .unchanged
    }
}

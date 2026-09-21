import Foundation

/// What the walk did with one element it was handed.
///
/// An iOS accessibility tree is a graph, not a tree: a reused table cell or a
/// container two parents share is referenced twice. The walk seats such an
/// element once, under the first parent that reached it, so that no control
/// can collect two marks. Its other parent then shows one fewer child than it
/// declared while nothing at all is missing from the export — the difference
/// these cases exist to keep.
enum AccessibilityWalkStep {
    /// The element is in the tree, under this parent.
    case seated([String: Any])
    /// The same element is already in the tree, under an earlier parent.
    case revisited
    /// No dictionary at all: a depth cut or a spent element budget. That is
    /// an observation the export does not have.
    case lost
}

/// One parent's declared children, counted by what became of each, so a
/// subtree is called truncated only for observations the export is missing.
///
/// Every skip used to count as truncation, and the reader refuses a truncated
/// tree outright — so one revisited subview made a screen that was exported
/// whole unmarkable (t-5445).
struct AccessibilityChildTally {
    /// How many children the element said it had.
    private let declared: Int
    /// How many of them were already seated elsewhere in the tree.
    private var revisited = 0
    /// The children kept here, in the order the walk reached them.
    private(set) var seats: [[String: Any]] = []

    init(declared: Int) {
        self.declared = declared
    }

    mutating func record(_ step: AccessibilityWalkStep) {
        switch step {
        case .seated(let child):
            seats.append(child)
        case .revisited:
            revisited += 1
        case .lost:
            break
        }
    }

    /// Whether a child this parent declared is neither seated here nor
    /// already seated elsewhere — including the ones a loop that stopped on
    /// a spent budget never handed over at all.
    var truncated: Bool {
        seats.count + revisited < declared
    }
}

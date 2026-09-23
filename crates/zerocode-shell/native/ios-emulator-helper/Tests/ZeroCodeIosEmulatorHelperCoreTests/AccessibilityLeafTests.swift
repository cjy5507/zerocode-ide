import CoreGraphics
import XCTest

@testable import ZeroCodeIosEmulatorHelperCore

/// Whether a walked element stops the grid decides how many points a look
/// asks about — and whether anything a hit-test alone can find is missed.
final class AccessibilityLeafTests: XCTestCase {
    private let row = CGRect(x: 16, y: 300, width: 370, height: 52)
    private let button = CGRect(x: 335, y: 811, width: 17, height: 22)
    private let screen = CGRect(x: 0, y: 0, width: 402, height: 874)

    /// The Settings row (t-6350): wider than any single control used to be
    /// allowed, and on top at its own centre.
    func testAWideRowThatAnswersItsOwnCentreBlocksTheGrid() {
        XCTAssertTrue(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: row, centre: .itself, screen: screen))
    }

    /// Maps (t-6385): the map view fills the screen, has no walked children
    /// and answers its own centre — and the place card, its search field and
    /// six more controls float over the rest of it, found only by the grid.
    /// Over half the screen is a canvas or a page (the core's
    /// `MARK_MAX_WINDOW_SHARE`): its centre being its own proves nothing
    /// about the rest of it.
    func testACanvasThatAnswersItsOwnCentreStillLeavesItsPointsToTheGrid() {
        XCTAssertFalse(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: screen, centre: .itself, screen: screen))
        let half = CGRect(x: 0, y: 0, width: screen.width, height: screen.height / 2 + 1)
        XCTAssertFalse(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: half, centre: .itself, screen: screen))
        // A screen nobody measured proves no share: nothing wide blocks.
        XCTAssertFalse(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: row, centre: .itself, screen: .zero))
    }

    /// Something inside or around it answered: the row may be a group over
    /// content only a hit-test can find, so its points are still asked.
    func testAWideElementThatDidNotAnswerItselfStaysProbeable() {
        for centre in [AccessibilityCentreHit.near, .elsewhere, nil] {
            XCTAssertFalse(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: row, centre: centre, screen: screen))
            XCTAssertFalse(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: screen, centre: centre, screen: screen))
        }
    }

    /// Whatever its centre said, an element whose children were walked is
    /// the kind that can hide more of them.
    func testAnElementWithWalkedChildrenNeverBlocks() {
        for centre in [AccessibilityCentreHit.itself, .near, .elsewhere, nil] {
            XCTAssertFalse(AccessibilityLeaf.blocksGrid(walkedChildren: 3, frame: button, centre: centre, screen: screen))
        }
    }

    /// A small leaf blocks as it always did, answered or not.
    func testASmallChildlessElementBlocksAsBefore() {
        for centre in [AccessibilityCentreHit.itself, .near, .elsewhere, nil] {
            XCTAssertTrue(AccessibilityLeaf.blocksGrid(walkedChildren: 0, frame: button, centre: centre, screen: screen))
        }
    }

    /// The tree's `hit_at_centre` is true for the element and for what is
    /// inside or around it, false only for something else.
    func testTheTreesCentreAnswerIsTrueUnlessSomethingElseIsOnTop() {
        XCTAssertTrue(AccessibilityCentreHit.itself.isItsOwn)
        XCTAssertTrue(AccessibilityCentreHit.near.isItsOwn)
        XCTAssertFalse(AccessibilityCentreHit.elsewhere.isItsOwn)
    }
}

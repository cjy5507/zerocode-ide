import Foundation
import XCTest
import ZeroCodeComputerUseMacOSCore

/// A fake accessibility tree by parent links: what the hit test meets at a
/// scroll area's centre, against what a scroll and a press accept there.
final class RegionReachTests: XCTestCase {
    /// window 0 ← scroll area 1 ← a web view's content, `deep` levels down;
    /// a sibling panel 2 and a dialog 3 over it, both children of the window.
    private func tree(deep: Int) -> (parent: [Int: Int], leaf: Int) {
        var parent: [Int: Int] = [1: 0, 2: 0, 3: 0]
        var node = 1
        for level in 0..<deep {
            let child = 100 + level
            parent[child] = node
            node = child
        }
        return (parent, node)
    }

    private func onTop(_ hit: Int, _ target: Int, _ reach: MarkPin.Reach, _ parent: [Int: Int]) -> Bool {
        MarkPin.onTop(hit: hit, target: target, reach: reach, parent: { parent[$0] }, same: ==)
    }

    func testAScrollAreaIsOnTopWhenItsCentreHitsItsOwnContentAtAnyDepth() {
        let deeper = MarkPin.hitDepth + 40
        let (parent, leaf) = tree(deep: deeper)
        XCTAssertTrue(onTop(leaf, 1, .region, parent), "its own text deep inside covers nothing")
        XCTAssertTrue(onTop(1, 1, .region, parent))
        XCTAssertTrue(onTop(0, 1, .region, parent), "what holds it is around it")
    }

    func testAScrollAreaUnderSomethingElseIsStillRefused() {
        let (parent, _) = tree(deep: 3)
        XCTAssertFalse(onTop(2, 1, .region, parent), "a sibling over it covers it")
        XCTAssertFalse(onTop(3, 1, .region, parent), "a dialog over it covers it")
    }

    func testAPressKeepsItsDepth() {
        let (parent, leaf) = tree(deep: MarkPin.hitDepth + 40)
        XCTAssertFalse(onTop(leaf, 1, .press, parent), "a press is judged as before: no deeper than hitDepth")
        let (near, nearLeaf) = tree(deep: 3)
        XCTAssertTrue(onTop(nearLeaf, 1, .press, near))
        XCTAssertFalse(onTop(2, 1, .press, near))
    }

    func testAParentLoopEndsWithinTheRegionDepth() {
        // 10 → 11 → 10 …: never reaches 1.
        let looping: [Int: Int] = [10: 11, 11: 10]
        var asked = 0
        let reached = MarkPin.onTop(hit: 10, target: 1, reach: .region, parent: { asked += 1; return looping[$0] }, same: ==)
        XCTAssertFalse(reached)
        XCTAssertLessThanOrEqual(asked, MarkPin.regionDepth + MarkPin.hitDepth)
    }

    func testOnlyTheScrollAsksForTheRegionRule() throws {
        let source = try String(
            contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
                .appendingPathComponent("../../Sources/ZeroCodeComputerUseMacOS/main.swift").standardized,
            encoding: .utf8
        )
        let regionCalls = source.components(separatedBy: "reach: .region)").count - 1
        XCTAssertEqual(regionCalls, 1, "one caller judges a region")
        let start = try XCTUnwrap(source.range(of: "private func backgroundScroll("))
        let end = try XCTUnwrap(source.range(of: "private func drag(", range: start.upperBound..<source.endIndex))
        XCTAssertTrue(source[start.lowerBound..<end.lowerBound].contains("reach: .region)"), "the scroll is that caller")
        XCTAssertTrue(source.contains("reach: MarkPin.Reach = .press"), "every other caller keeps the press rule")
    }
}

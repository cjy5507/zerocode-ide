import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOS

/// Agents working at the same time never wear the same colour while there is a
/// colour to spare: each takes the next palette colour in the order the helper
/// first saw it, and keeps it. Colours only: nothing is shown.
final class AgentColorBookTests: XCTestCase {
    private static let six = ["claude", "codex", "zo", "gemini", "kimi", "w-93661"]
    private static let colours = 6

    private static func shown(_ names: [String], in book: inout AgentColorBook) {
        for name in names { book.pointerShown(name) }
    }

    func testTwoNamesWhoseHashesMetWearTwoColours() {
        // The person's case: by the old name hash both were purple.
        XCTAssertEqual(AgentColor.hash("codex") % Self.colours, AgentColor.hash("zo") % Self.colours)
        var book = AgentColorBook(colours: Self.colours)
        book.pointerShown("codex")
        book.pointerShown("zo")
        XCTAssertNotEqual(book.index(for: "codex"), book.index(for: "zo"))
    }

    func testTheHelpersColoursGoToTheTwoNamesTooAndTheirMarksDiffer() {
        MainActor.assumeIsolated {
            let saved = AgentColor.book
            AgentColor.book = AgentColorBook(colours: AgentColor.palette.count)
            AgentColor.pointerShown("codex")
            AgentColor.pointerShown("zo")
            XCTAssertTrue(AgentColor.color(for: "codex") === AgentColor.palette[0])
            XCTAssertTrue(AgentColor.color(for: "zo") === AgentColor.palette[1])
            XCTAssertTrue(PointerView.color(for: "zo") === AgentColor.color(for: "zo"), "the pointer asks the same function")
            let a = ShareTakeLook.ground(agent: "codex", over: false, reduceTransparency: false)
            let b = ShareTakeLook.ground(agent: "zo", over: false, reduceTransparency: false)
            XCTAssertFalse(a.isEqual(b), "their marks differ too")
            AgentColor.book = saved
        }
    }

    func testColoursGoInTheOrderTheAgentsFirstCame() {
        var book = AgentColorBook(colours: Self.colours)
        for (order, name) in Self.six.enumerated() {
            book.pointerShown(name)
            XCTAssertEqual(book.index(for: name), order, name)
        }
    }

    func testTheSameNameKeepsItsColour() {
        var book = AgentColorBook(colours: Self.colours)
        Self.shown(["codex", "zo", "claude"], in: &book)
        book.pointerGone("zo")
        book.pointerShown("codex")
        book.pointerShown("zo")
        book.pointerGone("codex")
        book.pointerGone("claude")
        book.pointerShown("claude")
        XCTAssertEqual(book.index(for: "codex"), 0)
        XCTAssertEqual(book.index(for: "zo"), 1)
        XCTAssertEqual(book.index(for: "claude"), 2)
    }

    func testSixAtOnceAllDiffer() {
        var book = AgentColorBook(colours: Self.colours)
        Self.shown(Self.six, in: &book)
        XCTAssertEqual(Set(Self.six.compactMap { book.index(for: $0) }).count, 6)
    }

    func testASeventhTakesTheColourOfOneThatWent() {
        var book = AgentColorBook(colours: Self.colours)
        Self.shown(Self.six, in: &book)
        book.pointerGone("zo")
        book.pointerShown("seventh")
        XCTAssertEqual(book.index(for: "seventh"), 2, "zo's colour, the one nobody in view wears")
        let inView = Self.six.filter { $0 != "zo" } + ["seventh"]
        XCTAssertEqual(Set(inView.compactMap { book.index(for: $0) }).count, 6)
    }

    func testAColourStillOnAnAppsMarkIsNotHandedOn() {
        var book = AgentColorBook(colours: Self.colours)
        Self.shown(Self.six, in: &book)
        book.pointerGone("zo")
        book.pointerGone("kimi")
        // zo's pointer went, but the mark over the app it acted on still wears its colour.
        book.marking(["zo"])
        book.pointerShown("seventh")
        XCTAssertEqual(book.index(for: "seventh"), 4, "kimi's colour, not zo's")
        book.marking([])
    }

    func testOneThatComesBackWhileItsColourIsWornTakesAColourToSpare() {
        var book = AgentColorBook(colours: Self.colours)
        Self.shown(Self.six, in: &book)
        book.pointerGone("zo")
        book.pointerShown("seventh")
        book.pointerGone("kimi")
        let before = book.generation
        book.pointerShown("zo")
        XCTAssertNotEqual(book.index(for: "zo"), book.index(for: "seventh"), "two in view, two colours")
        XCTAssertEqual(book.index(for: "zo"), 4, "the colour nobody in view wears")
        XCTAssertGreaterThan(book.generation, before, "the marks are told to draw again")
    }

    func testSevenAtOnceShareAColourOnlyThen() {
        var book = AgentColorBook(colours: Self.colours)
        Self.shown(Self.six + ["seventh"], in: &book)
        XCTAssertEqual(Set(Self.six.compactMap { book.index(for: $0) }).count, 6)
        XCTAssertTrue((0..<Self.colours).contains(book.index(for: "seventh") ?? -1), "one of the six, worn twice")
    }

    func testNoNameWearsNoPaletteColour() {
        var book = AgentColorBook(colours: Self.colours)
        book.pointerShown("")
        XCTAssertNil(book.index(for: ""))
        book.pointerShown("codex")
        XCTAssertEqual(book.index(for: "codex"), 0, "the unnamed pointer took no colour")
        MainActor.assumeIsolated {
            XCTAssertFalse(AgentColor.palette.contains { $0 === AgentColor.color(for: "") }, "a neutral colour")
            XCTAssertTrue(AgentColor.color(for: "") === AgentColor.neutral)
        }
    }

    func testTheBookForgetsTheLongestGoneNamePastItsLimit() {
        var book = AgentColorBook(colours: Self.colours)
        for n in 0...AgentColorBook.limit {
            book.pointerShown("agent-\(n)")
            book.pointerGone("agent-\(n)")
        }
        XCTAssertEqual(book.remembered, AgentColorBook.limit)
        XCTAssertNil(book.index(for: "agent-0"), "the longest gone is forgotten")
        XCTAssertNotNil(book.index(for: "agent-\(AgentColorBook.limit)"))
    }
}

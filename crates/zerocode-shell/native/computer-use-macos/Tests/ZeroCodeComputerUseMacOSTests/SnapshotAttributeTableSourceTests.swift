import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The table is only worth its comment while it stays level with the walk.
///
/// An attribute the walk reads but the table forgets still answers — through
/// the reader's single-read fallback — so nothing breaks; it quietly costs a
/// cross-process round trip per node again, which is the whole thing this
/// change bought. That is the kind of regression no snapshot can show, so the
/// source says it instead: every attribute the tree walk names is in the
/// table, and the table names nothing the walk never asks for.
final class SnapshotAttributeTableSourceTests: XCTestCase {
    /// `kAXRoleAttribute as String` and the like, resolved to what they spell.
    private static let constants: [String: String] = [
        "kAXRoleAttribute": "AXRole",
        "kAXSubroleAttribute": "AXSubrole",
        "kAXRoleDescriptionAttribute": "AXRoleDescription",
        "kAXTitleAttribute": "AXTitle",
        "kAXDescriptionAttribute": "AXDescription",
        "kAXValueAttribute": "AXValue",
        "kAXURLAttribute": "AXURL",
        "kAXPlaceholderValueAttribute": "AXPlaceholderValue",
        "kAXPositionAttribute": "AXPosition",
        "kAXSizeAttribute": "AXSize",
        "kAXSelectedAttribute": "AXSelected",
        "kAXExpandedAttribute": "AXExpanded",
        "kAXEnabledAttribute": "AXEnabled",
        "kAXChildrenAttribute": "AXChildren",
        "kAXRowsAttribute": "AXRows",
    ]

    func testTheTableCoversEveryAttributeTheWalkReads() throws {
        let asked = try attributesTheWalkReads()
        XCTAssertFalse(asked.isEmpty, "the scan found no attribute reads — has the walk moved?")
        let table = Set(SnapshotNodeAttributes.all)
        XCTAssertEqual(
            asked.subtracting(table).sorted(),
            [],
            "the walk reads these one at a time because the batch does not ask for them"
        )
        XCTAssertEqual(
            table.subtracting(asked).sorted(),
            [],
            "the batch asks the observed app for these and nothing reads them"
        )
    }

    /// Every attribute asked for inside `AXSnapshotReader`, plus every one
    /// the tree renderer hands it. Free functions of the same name outside
    /// the reader belong to window discovery, which walks no tree, and
    /// `isSettable` is a different call that no batch can carry.
    private func attributesTheWalkReads() throws -> Set<String> {
        let source = try helperSource()
        let readerBody = try XCTUnwrap(
            body(of: "private final class AXSnapshotReader {", in: source),
            "AXSnapshotReader has been renamed"
        )
        return try attributeNames(in: readerBody, receiver: "")
            .union(attributeNames(in: source, receiver: "reader\\."))
    }

    private func attributeNames(in source: String, receiver: String) throws -> Set<String> {
        let accessors = "stringAttribute|boolAttribute|numberAttribute|copyArray|copyAttribute"
        let call = try NSRegularExpression(
            pattern: "\(receiver)(?:\(accessors))\\(\\s*[A-Za-z0-9_.]+\\s*,\\s*(kAX[A-Za-z]+Attribute|\"AX[A-Za-z]+\")"
        )
        var found: Set<String> = []
        let whole = NSRange(source.startIndex..<source.endIndex, in: source)
        for match in call.matches(in: source, range: whole) {
            guard let range = Range(match.range(at: 1), in: source) else { continue }
            let argument = String(source[range])
            if argument.hasPrefix("\"") {
                found.insert(String(argument.dropFirst().dropLast()))
            } else {
                found.insert(try XCTUnwrap(
                    Self.constants[argument],
                    "\(argument) is new here — say what it spells"
                ))
            }
        }
        return found
    }

    /// The text between a declaration's braces, counted rather than guessed.
    private func body(of declaration: String, in source: String) -> String? {
        guard let start = source.range(of: declaration) else { return nil }
        var depth = 0
        var index = source.index(before: start.upperBound)
        while index < source.endIndex {
            let character = source[index]
            if character == "{" {
                depth += 1
            } else if character == "}" {
                depth -= 1
                if depth == 0 {
                    return String(source[start.upperBound..<index])
                }
            }
            index = source.index(after: index)
        }
        return nil
    }

    private func helperSource() throws -> String {
        let packageRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return try String(
            contentsOf: packageRoot
                .appendingPathComponent("Sources")
                .appendingPathComponent("ZeroCodeComputerUseMacOS")
                .appendingPathComponent("main.swift"),
            encoding: .utf8
        )
    }
}

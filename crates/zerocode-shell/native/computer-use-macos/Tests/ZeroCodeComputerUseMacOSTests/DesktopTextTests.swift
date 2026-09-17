import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class DesktopTextTests: XCTestCase {
    func testAVisionBoxLandsOnTheScreenFromTheTopLeft() {
        // The top-left quarter of the image in Vision terms is x 0..0.5, y 0.5..1.
        let rect = recognizedTextRect(box: CGRect(x: 0, y: 0.5, width: 0.5, height: 0.5), origin: CGPoint(x: 100, y: 200), pointsWidth: 800, pointsHeight: 600)
        XCTAssertEqual(rect, CGRect(x: 100, y: 200, width: 400, height: 300))
        let whole = recognizedTextRect(box: CGRect(x: 0, y: 0, width: 1, height: 1), origin: .zero, pointsWidth: 10, pointsHeight: 20)
        XCTAssertEqual(whole, CGRect(x: 0, y: 0, width: 10, height: 20))
    }

    func testTextRenderedIntoAnImageIsReadBackWithItsPlace() throws {
        let width = 640, height = 160
        let rep = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
        ))
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor.white.setFill()
        NSRect(x: 0, y: 0, width: width, height: height).fill()
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 64, weight: .bold), .foregroundColor: NSColor.black]
        ("HELLO 123" as NSString).draw(at: NSPoint(x: 32, y: 40), withAttributes: attributes)
        NSGraphicsContext.restoreGraphicsState()
        let image = try XCTUnwrap(rep.cgImage)

        let lines = try recognizeText(in: image, origin: CGPoint(x: 1000, y: 500), pointsWidth: 640, pointsHeight: 160)
        let hello = try XCTUnwrap(lines.first { $0.text.uppercased().contains("HELLO") }, "lines: \(lines)")
        XCTAssertTrue(hello.text.contains("123"), hello.text)
        XCTAssertGreaterThan(hello.confidence, 0.3)
        XCTAssertTrue(CGRect(x: 1000, y: 500, width: 640, height: 160).contains(hello.frame), "\(hello.frame)")
        XCTAssertGreaterThan(hello.frame.width, 200)
    }

    func testAQueryMatchesTheWayAPersonReads() {
        let button = ElementQuery(text: nil, role: "button", label: "buy now")
        XCTAssertTrue(button.matches(role: "AXButton", label: "Buy Now", value: nil, description: nil))
        XCTAssertTrue(button.matches(role: "AXButton", label: nil, value: nil, description: "Buy now, pay later"))
        XCTAssertFalse(button.matches(role: "AXLink", label: "Buy Now", value: nil, description: nil))
        XCTAssertFalse(button.matches(role: "AXButton", label: "Cancel", value: nil, description: nil))
        let total = ElementQuery(text: "total", role: nil, label: nil)
        XCTAssertTrue(total.matches(role: "AXStaticText", label: nil, value: "Total: $12", description: nil))
        XCTAssertTrue(total.matches(role: "AXStaticText", label: "Subtotal", value: nil, description: nil), "a person searching 'total' finds 'Subtotal'")
        XCTAssertFalse(total.matches(role: "AXStaticText", label: "Tax", value: "$1", description: nil))
        XCTAssertTrue(ElementQuery(text: " ", role: nil, label: "").isEmpty)
    }

    func testAClickByReadingChoosesOneControlPrefersTheExactReadingAndNamesTheRest() {
        let faces = [
            ElementFace(index: 3, role: "AXButton", label: "Save", value: nil, description: nil),
            ElementFace(index: 5, role: "AXButton", label: "Save As…", value: nil, description: nil),
            ElementFace(index: 8, role: "AXStaticText", label: nil, value: "Saved 3 notes", description: nil),
            ElementFace(index: 9, role: "AXButton", label: nil, value: nil, description: "Cancel"),
        ]
        // "save" reads three faces; the button that reads exactly "Save" wins.
        XCTAssertEqual(ElementQuery(text: "save", role: nil, label: nil).choose(among: faces), .one(3))
        // A role narrows before the exact reading is asked.
        XCTAssertEqual(ElementQuery(text: "save", role: "button", label: nil).choose(among: faces), .one(3))
        XCTAssertEqual(ElementQuery(text: "saved", role: nil, label: nil).choose(among: faces), .one(8))
        XCTAssertEqual(ElementQuery(text: nil, role: nil, label: "cancel").choose(among: faces), .one(9))
        XCTAssertEqual(ElementQuery(text: "print", role: nil, label: nil).choose(among: faces), .none)
        // Two buttons, neither reading the words whole: the press never guesses.
        XCTAssertEqual(ElementQuery(text: nil, role: "button", label: "sav").choose(among: faces), .many([faces[0], faces[1]]))
        XCTAssertEqual(ElementQuery(text: nil, role: "button", label: nil).choose(among: faces).self,
                       .many([faces[0], faces[1], faces[3]]))
        XCTAssertEqual(faces[1].said, "5 AXButton «Save As…»")
        XCTAssertEqual(ElementQuery(text: "Save", role: "button", label: nil).words, "--text «Save» --role button")
    }
}

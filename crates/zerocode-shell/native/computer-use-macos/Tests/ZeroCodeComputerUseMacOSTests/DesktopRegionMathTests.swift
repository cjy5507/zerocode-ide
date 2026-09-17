import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The desktop screenshot's region (docs/design/computer-use-full-operator.md
/// §2.1): points on the desktop become pixels on the display that shows them.
final class DesktopRegionMathTests: XCTestCase {
    func testARegionOnARetinaDisplayBecomesItsPixels() {
        let display = CGRect(x: 0, y: 0, width: 1440, height: 900)
        let pixels = desktopRegionInPixels(region: CGRect(x: 10, y: 20, width: 300, height: 200), displayBounds: display, pixelWidth: 2880)
        XCTAssertEqual(pixels, CGRect(x: 20, y: 40, width: 600, height: 400))
    }

    func testARegionOnASecondDisplayIsLocalToThatDisplay() {
        let second = CGRect(x: 1440, y: -200, width: 1920, height: 1080)
        let pixels = desktopRegionInPixels(region: CGRect(x: 1500, y: 0, width: 100, height: 50), displayBounds: second, pixelWidth: 1920)
        XCTAssertEqual(pixels, CGRect(x: 60, y: 200, width: 100, height: 50))
    }

    func testARegionOffTheDisplayIsNothingAndAnOverhangIsClipped() {
        let display = CGRect(x: 0, y: 0, width: 1000, height: 500)
        XCTAssertNil(desktopRegionInPixels(region: CGRect(x: 2000, y: 0, width: 10, height: 10), displayBounds: display, pixelWidth: 1000))
        XCTAssertEqual(
            desktopRegionInPixels(region: CGRect(x: 950, y: 480, width: 100, height: 100), displayBounds: display, pixelWidth: 1000),
            CGRect(x: 950, y: 480, width: 50, height: 20)
        )
        XCTAssertEqual(desktopScreenshotScale(pixelWidth: 2880, pointsWidth: 1440), 2)
    }
}

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

    /// A screen left of the main one and one above it have negative points
    /// (t-66753 C2): a region there is captured on that screen, its pixels are
    /// counted from that screen's own corner, and the picture's corner is the
    /// region's — a pixel read off it names the same point on the desktop.
    func test_a_region_on_a_screen_left_of_or_above_the_main_one_names_the_same_points() {
        let main = CGRect(x: 0, y: 0, width: 1512, height: 982)
        let left = CGRect(x: -2560, y: -38, width: 2560, height: 1440)
        let above = CGRect(x: 0, y: -1080, width: 1920, height: 1080)
        let displays = [main, left, above]
        let region = CGRect(x: -2560, y: -38, width: 560, height: 1340)
        let at = desktopDisplayIndex(showing: region, among: displays)
        XCTAssertEqual(at, 1, "the screen left of the main one")
        XCTAssertEqual(
            desktopRegionInPixels(region: region, displayBounds: displays[at], pixelWidth: 5120),
            CGRect(x: 0, y: 0, width: 1120, height: 2680)
        )
        let scale = desktopScreenshotScale(pixelWidth: 1120, pointsWidth: region.width)
        XCTAssertEqual(scale, 2)
        XCTAssertEqual(
            CGPoint(x: region.minX + 1120 / scale, y: region.minY + 2680 / scale),
            CGPoint(x: region.maxX, y: region.maxY),
            "the picture's far corner is the region's"
        )
        let high = CGRect(x: 100, y: -1000, width: 300, height: 200)
        XCTAssertEqual(desktopDisplayIndex(showing: high, among: displays), 2, "the screen above the main one")
        XCTAssertEqual(
            desktopRegionInPixels(region: high, displayBounds: above, pixelWidth: 1920),
            CGRect(x: 100, y: 80, width: 300, height: 200)
        )
        XCTAssertEqual(
            DesktopPointCapture.plan(region: high, displayBounds: above, pointScale: 2),
            .region(sourceRect: high, pixelWidth: 600, pixelHeight: 400)
        )
        XCTAssertEqual(desktopDisplayIndex(showing: nil, among: displays), 0, "no region is the main screen")
        XCTAssertEqual(
            desktopDisplayIndex(showing: CGRect(x: -5000, y: -5000, width: 10, height: 10), among: displays),
            0,
            "a region no screen shows falls to the main one"
        )
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

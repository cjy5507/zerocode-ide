import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The probe's region capture at point resolution (t-10424). Pure planning:
/// no display is read.
final class PointCaptureTests: XCTestCase {
    private let retina = CGRect(x: 0, y: 0, width: 1512, height: 982)

    func testWithoutPointScaleTheWholeDisplayIsCapturedAndCroppedAsBefore() {
        XCTAssertEqual(
            DesktopPointCapture.plan(region: CGRect(x: 100, y: 100, width: 720, height: 440), displayBounds: retina, pointScale: nil),
            .wholeDisplayThenCrop)
        XCTAssertEqual(DesktopPointCapture.plan(region: nil, displayBounds: retina, pointScale: nil), .wholeDisplayThenCrop)
    }

    func testTheRegionsPixelSizeIsTheRegionTimesThePointScale() {
        let region = CGRect(x: 100, y: 100, width: 720, height: 440)
        for (scale, width, height) in [(1.0, 720, 440), (2.0, 1440, 880), (3.0, 2160, 1320), (0.5, 360, 220)] {
            XCTAssertEqual(
                DesktopPointCapture.plan(region: region, displayBounds: retina, pointScale: scale),
                .region(sourceRect: region, pixelWidth: width, pixelHeight: height), "scale \(scale)")
        }
    }

    func testARegionOverTheEdgeKeepsTheVisibleShareInGlobalPointsOnANegativeOriginDisplay() {
        let left = CGRect(x: -2560, y: -982, width: 2560, height: 1440)
        XCTAssertEqual(
            DesktopPointCapture.plan(region: CGRect(x: -100, y: 0, width: 300, height: 200), displayBounds: left, pointScale: 1),
            .region(sourceRect: CGRect(x: -100, y: 0, width: 100, height: 200), pixelWidth: 100, pixelHeight: 200))
    }

    func testARegionThatDoesNotTouchTheDisplayHasNoPlan() {
        XCTAssertNil(DesktopPointCapture.plan(region: CGRect(x: 5000, y: 0, width: 10, height: 10), displayBounds: retina, pointScale: 1))
    }

    func testNoRegionWithPointScaleIsTheWholeDisplayInPoints() {
        XCTAssertEqual(
            DesktopPointCapture.plan(region: nil, displayBounds: retina, pointScale: 1),
            .region(sourceRect: retina, pixelWidth: 1512, pixelHeight: 982))
    }

    func testPointScaleIsAPositiveNumberUpToTheFinestBackingScale() {
        XCTAssertEqual(try DesktopPointCapture.pointScale(nil).get(), nil)
        XCTAssertEqual(try DesktopPointCapture.pointScale(1).get(), 1)
        XCTAssertEqual(try DesktopPointCapture.pointScale(DesktopPointCapture.maxPointScale).get(), 4)
        for bad in [0, -1, 4.5, Double.nan, Double.infinity] {
            XCTAssertThrowsError(try DesktopPointCapture.pointScale(bad).get(), "\(bad)")
        }
    }
}

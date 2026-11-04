import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOS

/// The take-control mark wears the colour of the agent that last acted on its
/// app, the colour that agent's pointer wears. Colours only: nothing is shown.
final class ShareTakeColorTests: XCTestCase {
    /// Two names the palette tells apart, as two agents at once would be.
    private static let one = "claude"
    private static let other = "codex"
    private static let appearances: [NSAppearance.Name] = [
        .aqua, .darkAqua, .accessibilityHighContrastAqua, .accessibilityHighContrastDarkAqua,
    ]

    private static func each(_ body: (NSAppearance.Name) -> Void) {
        for name in appearances {
            guard let appearance = NSAppearance(named: name) else { return XCTFail("no \(name.rawValue)") }
            appearance.performAsCurrentDrawingAppearance { body(name) }
        }
    }

    private static func same(_ a: NSColor, _ b: NSColor, _ message: String) {
        guard let a = a.usingColorSpace(.sRGB), let b = b.usingColorSpace(.sRGB) else { return XCTFail(message) }
        XCTAssertEqual(a.redComponent, b.redComponent, accuracy: 0.001, message)
        XCTAssertEqual(a.greenComponent, b.greenComponent, accuracy: 0.001, message)
        XCTAssertEqual(a.blueComponent, b.blueComponent, accuracy: 0.001, message)
        XCTAssertEqual(a.alphaComponent, b.alphaComponent, accuracy: 0.001, message)
    }

    func testOneAgentsPointerAndItsAppsMarkWearOneColourFromOneFunction() {
        MainActor.assumeIsolated {
            Self.each { appearance in
                for agent in [Self.one, Self.other, "w-93661", String(repeating: "a long agent name ", count: 3)] {
                    XCTAssertTrue(PointerView.color(for: agent) === AgentColor.color(for: agent), "\(agent) \(appearance.rawValue)")
                    Self.same(ShareTakeLook.ground(agent: agent, over: false, reduceTransparency: false),
                         PointerView.nameGround(for: agent), "\(agent) \(appearance.rawValue): the mark is its name's ground")
                    // Reduce Transparency changes nothing on an agent's ground: it is opaque already.
                    Self.same(ShareTakeLook.ground(agent: agent, over: false, reduceTransparency: true),
                         PointerView.nameGround(for: agent), "\(agent) \(appearance.rawValue) reduce transparency")
                }
            }
        }
    }

    func testTwoAgentsTakeTwoDifferentColoursOfThePalette() {
        MainActor.assumeIsolated {
            let saved = AgentColor.book
            defer { AgentColor.book = saved }
            AgentColor.book = AgentColorBook(colours: AgentColor.palette.count)
            AgentColor.pointerShown(Self.one)
            AgentColor.pointerShown(Self.other)
            let first = AgentColor.color(for: Self.one)
            let second = AgentColor.color(for: Self.other)
            XCTAssertTrue(AgentColor.palette.contains { $0 === first })
            XCTAssertTrue(AgentColor.palette.contains { $0 === second })
            XCTAssertFalse(first === second, "\(Self.one) and \(Self.other) are told apart")
            Self.each { appearance in
                let a = AgentColor.ground(for: Self.one), b = AgentColor.ground(for: Self.other)
                let apart = abs(a.redComponent - b.redComponent) + abs(a.greenComponent - b.greenComponent)
                    + abs(a.blueComponent - b.blueComponent)
                XCTAssertGreaterThan(apart, 0.2, "\(appearance.rawValue): their marks differ too")
            }
        }
    }

    func testAnAppNoNamedAgentActedOnKeepsTheNeutralGround() {
        MainActor.assumeIsolated {
            for agent in [nil, ""] as [String?] {
                Self.same(ShareTakeLook.ground(agent: agent, over: false, reduceTransparency: false),
                     NSColor.black.withAlphaComponent(0.72), "at rest")
                Self.same(ShareTakeLook.ground(agent: agent, over: true, reduceTransparency: false),
                     NSColor.black.withAlphaComponent(0.86), "under the pointer")
                Self.same(ShareTakeLook.ground(agent: agent, over: false, reduceTransparency: true),
                     NSColor.black, "Reduce Transparency: no app shows through")
            }
            XCTAssertNil(AgentActingLog().agent(pid: 42))
        }
    }

    func testTwoAppsWithTwoAgentsShowTwoMarksEachInItsAgentsColour() {
        MainActor.assumeIsolated {
            let saved = AgentColor.book
            defer { AgentColor.book = saved }
            AgentColor.book = AgentColorBook(colours: AgentColor.palette.count)
            var log = AgentActingLog()
            log.record(pid: 101, agent: Self.one)
            log.record(pid: 202, agent: Self.other)
            let first = ShareTakeButton(title: "Take control", target: nil, action: nil)
            let second = ShareTakeButton(title: "Take control", target: nil, action: nil)
            first.agent = log.agent(pid: 101)
            second.agent = log.agent(pid: 202)
            XCTAssertEqual(first.agent, Self.one)
            XCTAssertEqual(second.agent, Self.other)
            let a = ShareTakeLook.ground(agent: first.agent, over: false, reduceTransparency: false)
            let b = ShareTakeLook.ground(agent: second.agent, over: false, reduceTransparency: false)
            Self.same(a, AgentColor.ground(for: Self.one), "first app")
            Self.same(b, AgentColor.ground(for: Self.other), "second app")
            XCTAssertFalse(a.isEqual(b))
        }
    }

    func testTheMarkTakesTheColourOfTheAgentThatActedLastOnItsApp() {
        var log = AgentActingLog()
        log.record(pid: 101, agent: Self.one)
        XCTAssertEqual(log.agent(pid: 101), Self.one)
        log.record(pid: 101, agent: Self.other)
        XCTAssertEqual(log.agent(pid: 101), Self.other, "the newest act wins")
        log.record(pid: 202, agent: Self.one)
        XCTAssertEqual(log.agent(pid: 101), Self.other, "an act on another app leaves this one")
        log.record(pid: 101, agent: "")
        XCTAssertNil(log.agent(pid: 101), "an act with no name: neutral again")
    }

    func testTheLogForgetsTheOldestAppPastItsLimit() {
        var log = AgentActingLog()
        for pid in 1...Int32(AgentActingLog.limit + 1) { log.record(pid: pid, agent: Self.one) }
        XCTAssertNil(log.agent(pid: 1), "the oldest app is forgotten")
        XCTAssertEqual(log.agent(pid: 2), Self.one)
        XCTAssertEqual(log.agent(pid: Int32(AgentActingLog.limit + 1)), Self.one)
    }

    /// White on every agent's ground reads at 4.5:1 or better, in light, dark
    /// and Increase Contrast, at rest and under the pointer: a colour too light
    /// for that is darkened, one dark enough is left as it is.
    func testWhiteReadsOnEveryPaletteColourBecauseLightOnesAreDarkened() {
        MainActor.assumeIsolated {
            var darkened = 0
            var lines: [String] = []
            let darkApp = NSColor(srgbRed: 0x1E / 255, green: 0x1E / 255, blue: 0x1E / 255, alpha: 1)
            Self.each { appearance in
                for (index, color) in AgentColor.palette.enumerated() {
                    let raw = AgentColor.contrast(color, .white)
                    let ground = AgentColor.readable(color)
                    let readable = AgentColor.contrast(ground, .white)
                    let over = AgentColor.contrast(ground.blended(withFraction: ShareTakeLook.overDarkening, of: .black)!, .white)
                    XCTAssertGreaterThanOrEqual(readable, AgentColor.minimumContrast, "\(appearance.rawValue) #\(index)")
                    XCTAssertGreaterThanOrEqual(over, AgentColor.minimumContrast, "\(appearance.rawValue) #\(index) under the pointer")
                    if raw < AgentColor.minimumContrast {
                        darkened += 1
                        XCTAssertGreaterThan(readable, raw, "\(appearance.rawValue) #\(index) is darkened")
                        // Darkened no further than it takes: the agent's hue still shows.
                        XCTAssertLessThan(readable, AgentColor.minimumContrast + 1.5, "\(appearance.rawValue) #\(index)")
                    } else {
                        Self.same(ground, color, "\(appearance.rawValue) #\(index) dark enough already")
                    }
                    lines.append(String(format: "contrast %@ #%d raw %.2f ground %.2f over %.2f | mark vs white app %.2f, vs dark app %.2f, rim vs dark app %.2f",
                                        appearance.rawValue, index, raw, readable, over,
                                        readable, AgentColor.contrast(ground, darkApp), AgentColor.contrast(.white, darkApp)))
                }
            }
            XCTAssertGreaterThan(darkened, 0, "the palette's light colours take a dark variant")
            print(lines.joined(separator: "\n"))
        }
    }

    func testTheRimWidensAndGainsADarkLineUnderIncreaseContrast() {
        XCTAssertEqual(ShareTakeLook.rim(increaseContrast: false).white, 1.5)
        XCTAssertEqual(ShareTakeLook.rim(increaseContrast: false).dark, 0)
        XCTAssertEqual(ShareTakeLook.rim(increaseContrast: true).white, 2)
        XCTAssertEqual(ShareTakeLook.rim(increaseContrast: true).dark, 1)
    }

    func testTheColouredMarkStillTakesOnlyAPhysicalPress() {
        MainActor.assumeIsolated {
            let button = ShareTakeButton(title: "Take control", target: nil, action: nil)
            button.agent = Self.one
            XCTAssertFalse(button.accessibilityPerformPress())
            XCTAssertFalse(button.acceptsFirstResponder)
            XCTAssertEqual(button.title, "Take control", "no agent name is written on it")
        }
    }
}

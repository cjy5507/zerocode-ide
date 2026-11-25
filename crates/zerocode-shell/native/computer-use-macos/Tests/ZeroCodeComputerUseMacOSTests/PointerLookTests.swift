import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOS
@testable import ZeroCodeComputerUseMacOSCore

/// What an agent's pointer looks like, read off the picture the view draws:
/// its rim ends on the point its window is put by, its shadow is whole, and its
/// name stands beside the dart. Drawn into a bitmap only: nothing is shown.
final class PointerLookTests: XCTestCase {
    /// Pixels to a point, so a quarter of a point is one pixel.
    private static let scale = 4
    private static let names = ["term-41", "", String(repeating: "a long agent name ", count: 3)]

    /// The view's picture as it would be drawn, premultiplied RGBA, the top row first.
    @MainActor
    private static func picture(agent: String, increaseContrast: Bool, appearance: NSAppearance.Name = .aqua)
        -> (size: CGSize, alpha: (CGPoint) -> Double, isWhite: (CGPoint) -> Bool, edgeAlpha: Double)
    {
        let size = PointerView.size(agent: agent)
        let view = PointerView(frame: CGRect(origin: .zero, size: size), agent: agent)
        view.increaseContrast = increaseContrast
        let (wide, high) = (Int(size.width) * scale, Int(size.height) * scale)
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: wide, pixelsHigh: high, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                   bytesPerRow: 0, bitsPerPixel: 0)!
        rep.size = size
        let look = NSAppearance(named: appearance)!
        view.appearance = look
        look.performAsCurrentDrawingAppearance { view.cacheDisplay(in: view.bounds, to: rep) }
        // `bitmapData` points into the bitmap, and nothing holds the bitmap once
        // this returns: what is read afterwards is a copy of its bytes, taken
        // while it is held.
        let row = rep.bytesPerRow
        let data = withExtendedLifetime(rep) { Array(UnsafeBufferPointer(start: rep.bitmapData!, count: row * high)) }
        func pixel(_ point: CGPoint) -> (r: Int, g: Int, b: Int, a: Int) {
            let x = min(wide - 1, max(0, Int((point.x * CGFloat(scale)).rounded(.down))))
            let y = min(high - 1, max(0, Int((point.y * CGFloat(scale)).rounded(.down))))
            let at = y * row + x * 4
            return (Int(data[at]), Int(data[at + 1]), Int(data[at + 2]), Int(data[at + 3]))
        }
        var edge = 0
        for x in 0..<wide {
            edge = max(edge, Int(data[x * 4 + 3]), Int(data[(high - 1) * row + x * 4 + 3]))
        }
        for y in 0..<high {
            edge = max(edge, Int(data[y * row + 3]), Int(data[y * row + (wide - 1) * 4 + 3]))
        }
        return (size, { Double(pixel($0).a) / 255 },
                { let p = pixel($0); return p.a == 255 && min(p.r, p.g, p.b) >= 250 },
                Double(edge) / 255)
    }

    private static func along(_ distance: CGFloat) -> CGPoint {
        CGPoint(x: PointerLook.tip.x + PointerLook.axis.dx * distance, y: PointerLook.tip.y + PointerLook.axis.dy * distance)
    }

    /// The window is put so that the view's tip lies on the point of the act
    /// (`WindowPointerAnchor.panelFrame`): what is drawn there must be the
    /// pointer's outermost point, whatever the rim's width.
    func testTheRimEndsOnTheTipTheWindowIsPutBy() {
        MainActor.assumeIsolated {
            for agent in Self.names {
                for increaseContrast in [false, true] {
                    let drawn = Self.picture(agent: agent, increaseContrast: increaseContrast)
                    let said = "\(agent.isEmpty ? "no name" : agent) increaseContrast \(increaseContrast)"
                    // Half a point inside the tip the rim is solid; a point outside it only the shadow is.
                    XCTAssertEqual(drawn.alpha(Self.along(0.75)), 1, accuracy: 0.02, "\(said): the rim stands just inside the tip")
                    XCTAssertLessThan(drawn.alpha(Self.along(-1)), 0.5, "\(said): nothing solid stands beyond the tip")
                    // And nothing solid lies further out along the axis than the tip, anywhere in the view.
                    var furthest = CGFloat.infinity
                    var y: CGFloat = 0.125
                    while y < drawn.size.height {
                        var x: CGFloat = 0.125
                        while x < drawn.size.width {
                            if drawn.alpha(CGPoint(x: x, y: y)) > 0.9 {
                                furthest = min(furthest, (x - PointerLook.tip.x) * PointerLook.axis.dx + (y - PointerLook.tip.y) * PointerLook.axis.dy)
                            }
                            x += 0.25
                        }
                        y += 0.25
                    }
                    XCTAssertEqual(furthest, 0, accuracy: 0.5, "\(said): the outermost solid point is the tip")
                }
            }
        }
    }

    /// The tip is the one the window is put by: a point given to the anchor
    /// comes back as the place of the view's tip on the screen.
    func testTheWindowPutByTheViewsTipHasItsRimOnThePointOfTheAct() throws {
        try MainActor.assumeIsolated {
            let point = CGPoint(x: 240, y: 180)
            let anchor = try XCTUnwrap(WindowPointerAnchor(pid: 1, windowID: 2,
                bounds: CGRect(x: 100, y: 100, width: 400, height: 250), point: point))
            let size = PointerView.size(agent: "term-41")
            let frame = try XCTUnwrap(anchor.panelFrame(mainDisplayHeight: 1000, size: size, tip: PointerLook.tip))
            // The view is flipped: a point of it is `y` below the window's top edge.
            XCTAssertEqual(frame.minX + PointerLook.tip.x, point.x)
            XCTAssertEqual(1000 - (frame.maxY - PointerLook.tip.y), point.y)
        }
    }

    /// The shadow fades out inside the view: a shadow cut at the window's edge
    /// is a straight line beside the pointer.
    func testNoPartOfThePointerOrItsShadowIsCutAtTheViewsEdge() {
        MainActor.assumeIsolated {
            for agent in Self.names {
                for increaseContrast in [false, true] {
                    let drawn = Self.picture(agent: agent, increaseContrast: increaseContrast)
                    XCTAssertLessThan(drawn.edgeAlpha, 0.02, "\(agent.isEmpty ? "no name" : agent) increaseContrast \(increaseContrast)")
                }
            }
        }
    }

    /// The dart has an edge on any ground: white all round its colour, and
    /// under Increase Contrast a dark line outside the white.
    func testTheDartKeepsAWhiteRimAndGainsADarkLineUnderIncreaseContrast() {
        MainActor.assumeIsolated {
            XCTAssertEqual(PointerLook.rim(increaseContrast: false).white, ShareTakeLook.rim(increaseContrast: false).white)
            XCTAssertEqual(PointerLook.rim(increaseContrast: false).dark, ShareTakeLook.rim(increaseContrast: false).dark)
            XCTAssertEqual(PointerLook.rim(increaseContrast: true).white, ShareTakeLook.rim(increaseContrast: true).white)
            XCTAssertEqual(PointerLook.rim(increaseContrast: true).dark, ShareTakeLook.rim(increaseContrast: true).dark)
            let plain = Self.picture(agent: "term-41", increaseContrast: false)
            XCTAssertTrue(plain.isWhite(Self.along(0.75)), "the outermost line is white")
            let contrast = Self.picture(agent: "term-41", increaseContrast: true)
            XCTAssertFalse(contrast.isWhite(Self.along(0.5)), "under Increase Contrast the outermost line is dark")
            XCTAssertTrue(contrast.isWhite(Self.along(2)), "and the white stands inside it")
        }
    }

    /// The name stands beside the dart, never over it: between the rim of the
    /// dart and the line round the name there is ground to see, with the rims
    /// as they are and as Increase Contrast widens them.
    func testTheNameStandsClearOfTheDart() {
        MainActor.assumeIsolated {
            /// Points a tenth of a point apart along a closed outline of straight lines.
            func walked(_ corners: [CGPoint]) -> [CGPoint] {
                var points: [CGPoint] = []
                for (index, from) in corners.enumerated() {
                    let to = corners[(index + 1) % corners.count]
                    let steps = max(1, Int((hypot(to.x - from.x, to.y - from.y) * 10).rounded(.up)))
                    for step in 0..<steps {
                        let t = CGFloat(step) / CGFloat(steps)
                        points.append(CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t))
                    }
                }
                return points
            }
            for increaseContrast in [false, true] {
                let rim = PointerLook.rim(increaseContrast: increaseContrast)
                let dart = walked(PointerLook.corners(increaseContrast: increaseContrast))
                // The line round the name as it is drawn, its curves as short straight lines.
                let ground = PointerLook.labelGround(PointerLook.labelFrame("term-41", increaseContrast: increaseContrast),
                                                     grown: PointerLook.labelRim + rim.dark).flattened
                var corners: [CGPoint] = []
                var points = [NSPoint](repeating: .zero, count: 3)
                for index in 0..<ground.elementCount where ground.element(at: index, associatedPoints: &points) != .closePath {
                    corners.append(points[0])
                }
                let name = walked(corners)
                var nearest = CGFloat.infinity
                for a in dart { for b in name { nearest = min(nearest, hypot(a.x - b.x, a.y - b.y)) } }
                let between = nearest - (PointerLook.bodyStroke / 2 + rim.white + rim.dark)
                XCTAssertGreaterThanOrEqual(between, 1.5, "increaseContrast \(increaseContrast): ground shows between the two")
                XCTAssertLessThanOrEqual(between, 4, "increaseContrast \(increaseContrast): the name is not left far from its dart")
            }
        }
    }

    /// A pointer with no name is the dart alone; a name is cut at its limit
    /// and the view has room for all of what is written.
    func testTheViewHasRoomForTheDartAndForTheNameAsItIsWritten() {
        MainActor.assumeIsolated {
            let bare = PointerView.size(agent: "")
            let named = PointerView.size(agent: "term-41")
            XCTAssertGreaterThan(named.width, bare.width)
            XCTAssertGreaterThan(named.height, bare.height)
            let long = String(repeating: "a long agent name ", count: 3)
            let written = String(long.prefix(PointerLook.maxLabelCharacters))
            XCTAssertEqual(PointerView.size(agent: long), PointerView.size(agent: written), "only what is written takes room")
            for increaseContrast in [false, true] {
                let label = PointerLook.labelFrame(written, increaseContrast: increaseContrast)
                let around = PointerLook.labelRim + PointerLook.rim(increaseContrast: increaseContrast).dark + PointerLook.margin
                XCTAssertGreaterThanOrEqual(PointerView.size(agent: long).width, label.maxX + around)
                XCTAssertGreaterThanOrEqual(PointerView.size(agent: long).height, label.maxY + around)
            }
            // Whole points: a window is put on whole points.
            for size in [bare, named, PointerView.size(agent: long)] {
                XCTAssertEqual(size.width, size.width.rounded())
                XCTAssertEqual(size.height, size.height.rounded())
            }
        }
    }

    /// Each tone is the one for the appearance drawing it; a name it does not
    /// know is drawn as over a light desktop.
    func testATonesColourIsTheOneForItsAppearance() {
        let tone = AgentTone(light: 0x112233, dark: 0x445566, lightContrast: 0x778899, darkContrast: 0xAABBCC)
        XCTAssertEqual(tone.hex(for: .aqua), 0x112233)
        XCTAssertEqual(tone.hex(for: .darkAqua), 0x445566)
        XCTAssertEqual(tone.hex(for: .accessibilityHighContrastAqua), 0x778899)
        XCTAssertEqual(tone.hex(for: .accessibilityHighContrastDarkAqua), 0xAABBCC)
        XCTAssertEqual(tone.hex(for: .vibrantLight), 0x112233)
        XCTAssertEqual(Set(AgentTone.appearances).count, 4)
        MainActor.assumeIsolated {
            for (name, hex) in [(NSAppearance.Name.aqua, 0x112233), (.darkAqua, 0x445566)] {
                NSAppearance(named: name)!.performAsCurrentDrawingAppearance {
                    let drawn = tone.color.usingColorSpace(.sRGB)!
                    XCTAssertEqual(Int((drawn.redComponent * 255).rounded()), (hex >> 16) & 0xFF, name.rawValue)
                    XCTAssertEqual(Int((drawn.greenComponent * 255).rounded()), (hex >> 8) & 0xFF, name.rawValue)
                    XCTAssertEqual(Int((drawn.blueComponent * 255).rounded()), hex & 0xFF, name.rawValue)
                }
            }
        }
    }

    /// Over a light desktop an agent's dart, its name's ground and its app's
    /// mark are one colour: white reads on every light tone as it is. Over a
    /// dark one the tone is brighter. No tone is lost on the ground of its own
    /// appearance, and the two an ordinary desktop draws are lost on neither.
    func testEveryToneReadsUnderWhiteOverALightDesktopAndStandsOffBothGrounds() {
        MainActor.assumeIsolated {
            XCTAssertEqual(AgentColor.palette.count, AgentColor.tones.count)
            XCTAssertEqual(AgentColor.tones.count, 6, "six agents at once are told apart")
            func color(_ hex: Int) -> NSColor {
                NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                        blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
            }
            for (index, tone) in AgentColor.tones.enumerated() {
                XCTAssertGreaterThanOrEqual(AgentColor.contrast(color(tone.light), .white), AgentColor.minimumContrast, "light #\(index)")
                XCTAssertGreaterThanOrEqual(AgentColor.contrast(color(tone.lightContrast), .white), 7, "light, Increase Contrast #\(index)")
                XCTAssertGreaterThan(AgentColor.luminance(color(tone.dark)), AgentColor.luminance(color(tone.light)), "dark is brighter #\(index)")
                XCTAssertGreaterThan(AgentColor.luminance(color(tone.darkContrast)), AgentColor.luminance(color(tone.dark)), "dark, Increase Contrast is brighter still #\(index)")
                // A filled shape is told from its ground at 3:1 (WCAG 1.4.11).
                for hex in [tone.light, tone.dark, tone.lightContrast, tone.darkContrast] {
                    XCTAssertGreaterThanOrEqual(AgentColor.contrast(color(hex), .white), 3, "#\(index) on a white window")
                }
                for hex in [tone.light, tone.dark, tone.darkContrast] {
                    XCTAssertGreaterThanOrEqual(AgentColor.contrast(color(hex), .black), 3, "#\(index) on a black window")
                }
            }
        }
    }

    /// Any two agents are told apart by colour. How far apart two colours look
    /// is their distance in OKLab, where equal steps look equal: the six stand
    /// where the nearest two of an ordinary desktop are 0.14 apart or more
    /// (the system's six under white stood 0.12 apart), and the first two
    /// handed out are far apart.
    func testNoTwoTonesOfOneAppearanceAreNearEachOther() {
        MainActor.assumeIsolated {
            /// OKLab of 0xRRGGBB (Björn Ottosson, 2020).
            func oklab(_ hex: Int) -> [Double] {
                let linear = [Double((hex >> 16) & 0xFF), Double((hex >> 8) & 0xFF), Double(hex & 0xFF)].map { part -> Double in
                    let v = part / 255
                    return v <= 0.040_45 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
                }
                let (r, g, b) = (linear[0], linear[1], linear[2])
                let l = cbrt(0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b)
                let m = cbrt(0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b)
                let s = cbrt(0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b)
                return [0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
                        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
                        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s]
            }
            func apart(_ a: Int, _ b: Int) -> Double {
                zip(oklab(a), oklab(b)).map { ($0 - $1) * ($0 - $1) }.reduce(0, +).squareRoot()
            }
            XCTAssertEqual(apart(0xFFFFFF, 0x000000), 1, accuracy: 0.001, "white to black is one")
            for name in AgentTone.appearances {
                var nearest = Double.infinity
                for (index, one) in AgentColor.tones.enumerated() {
                    for other in AgentColor.tones.dropFirst(index + 1) {
                        nearest = min(nearest, apart(one.hex(for: name), other.hex(for: name)))
                    }
                }
                let ordinary = name == .aqua || name == .darkAqua
                XCTAssertGreaterThanOrEqual(nearest, ordinary ? 0.14 : 0.12, "\(name.rawValue): the nearest two")
                XCTAssertGreaterThanOrEqual(apart(AgentColor.tones[0].hex(for: name), AgentColor.tones[1].hex(for: name)), 0.3,
                                            "\(name.rawValue): the first two agents")
            }
        }
    }
}

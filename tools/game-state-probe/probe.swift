// The game-state probe: draws the owned scenes and times the helper's real perception kernel on
// them. `build.py` compiles this file with the helper's Core sources into one optimised binary, so
// what it runs is the product code, not a copy. It opens no window, captures no screen and posts no
// input: its frames are its own drawings in IOSurface-backed BGRA buffers, the same kind of buffer
// ScreenCaptureKit delivers.
//
//   probe render  <scenes.json> <png dir>
//   probe measure <scenes.json> <png dir> <out.json>

import CoreGraphics
import CoreText
import CoreVideo
import Foundation
import ImageIO
import UniformTypeIdentifiers

@main
struct GameStateProbe {
    static func main() throws {
        let arguments = CommandLine.arguments
        guard arguments.count >= 4 else {
            FileHandle.standardError.write(Data("usage: probe render|measure <scenes.json> <png dir> [out.json]\n".utf8))
            exit(2)
        }
        let fixture = try Fixture(URL(fileURLWithPath: arguments[2]))
        let pictures = URL(fileURLWithPath: arguments[3])
        switch arguments[1] {
        case "render":
            try FileManager.default.createDirectory(at: pictures, withIntermediateDirectories: true)
            for scene in fixture.scenes {
                try write(Painter(fixture: fixture, scene: scene).image(), to: pictures.appendingPathComponent("\(scene.id).png"))
            }
            print(#"{"rendered":\#(fixture.scenes.count)}"#)
        case "measure" where arguments.count == 5:
            let report = try Measure(fixture: fixture, pictures: pictures).run()
            try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys])
                .write(to: URL(fileURLWithPath: arguments[4]))
            print(#"{"measured":"\#(arguments[4])"}"#)
        default:
            exit(2)
        }
    }
}

// MARK: - The scenes

struct Scene {
    let id: String
    let split: String
    let frame: [String: String]
    let draw: [String: Any]
}

struct Fixture {
    let spec: PerceptionColorSpec
    let roi: ReflexRoi
    let palette: [Int: [Double]]
    let ground: [Double]
    let reference: (width: Int, height: Int)
    let scenes: [Scene]

    init(_ url: URL) throws {
        let json = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        func decode<T: Decodable>(_ type: T.Type, _ value: Any?) throws -> T {
            try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: value!))
        }
        spec = try decode(PerceptionColorSpec.self, json["spec"])
        roi = try decode(ReflexRoi.self, json["roi"])
        palette = Dictionary(uniqueKeysWithValues: (json["palette"] as! [String: [Double]]).map { (Int($0.key)!, $0.value) })
        ground = json["ground"] as! [Double]
        let reference = json["reference"] as! [Int]
        self.reference = (reference[0], reference[1])
        scenes = (json["scenes"] as! [[String: Any]]).map {
            Scene(id: $0["id"] as! String, split: $0["split"] as! String, frame: $0["frame"] as! [String: String],
                  draw: $0["draw"] as! [String: Any])
        }
    }
}

/// Draws one scene as the fixture game would: a HUD bar, a 4 × 4 board of rounded tiles with a
/// digit each, then anything covering it, then any turn or stretch of the finished frame.
struct Painter {
    let fixture: Fixture
    let scene: Scene

    private var draw: [String: Any] { scene.draw }

    /// The theme's version of a colour: the dark theme dims it, the light one washes it out.
    private func themed(_ rgb: [Double]) -> CGColor {
        let channels: [Double]
        switch draw["theme"] as! String {
        case "dark": channels = rgb.map { ($0 * 11 / 20).rounded(.down) }
        case "light": channels = rgb.map { ($0 + (255 - $0) * 11 / 20).rounded(.down) }
        default: channels = rgb
        }
        return CGColor(srgbRed: channels[0] / 255, green: channels[1] / 255, blue: channels[2] / 255, alpha: 1)
    }

    private var ground: CGColor {
        switch draw["theme"] as! String {
        case "dark": return CGColor(srgbRed: 6 / 255, green: 6 / 255, blue: 8 / 255, alpha: 1)
        case "light": return CGColor(srgbRed: 246 / 255, green: 246 / 255, blue: 250 / 255, alpha: 1)
        default: return themed(fixture.ground)
        }
    }

    private var ink: CGColor {
        draw["theme"] as! String == "light" ? CGColor(srgbRed: 0.16, green: 0.16, blue: 0.16, alpha: 1)
            : CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 1)
    }

    func image() throws -> CGImage {
        let scale = draw["pixels_per_point"] as! Double
        let (width, height) = (Int((Double(fixture.reference.width) * scale).rounded()),
                               Int((Double(fixture.reference.height) * scale).rounded()))
        let context = try canvas(width, height)
        context.translateBy(x: 0, y: CGFloat(height))
        context.scaleBy(x: CGFloat(scale), y: -CGFloat(scale))
        context.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        context.setFillColor(ground)
        context.fill(CGRect(x: 0, y: 0, width: fixture.reference.width, height: fixture.reference.height))
        context.setFillColor(themed(fixture.palette[5]!))
        context.fill(CGRect(x: 0, y: 0, width: fixture.reference.width, height: 60))
        text("SCORE", at: CGPoint(x: 52, y: 30), size: 16, in: context)
        board(context)
        for occluder in draw["occluders"] as! [[String: Any]] { cover(occluder, context) }
        var image = context.makeImage()!
        if let turn = draw["turn"] as? Int, turn != 0 { image = try turned(image, by: turn) }
        let stretch = draw["stretch"] as! [Double]
        if stretch != [1, 1] { image = try stretched(image, stretch) }
        return image
    }

    private func board(_ context: CGContext) {
        let roi = fixture.roi
        let size = draw["size"] as! Double, offset = draw["offset"] as! [Double]
        let (width, height) = (Double(roi.width) * size, Double(roi.height) * size)
        let left = Double(roi.x) + Double(roi.width) / 2 - width / 2 + offset[0]
        let top = Double(roi.y) + Double(roi.height) / 2 - height / 2 + offset[1]
        guard case .cells(let layout) = fixture.spec.layout else { return }
        let (pitchX, pitchY) = (width / 4, height / 4)
        let (tileX, tileY) = (pitchX * Double(layout.tile_width_permille) / 1000,
                              pitchY * Double(layout.tile_height_permille) / 1000)
        let cells = draw["cells"] as! [Int], tiles = draw["tiles"] as! [String: [Double]]
        for (at, cell) in cells.enumerated() {
            let x = left + Double(at % 4) * pitchX + (pitchX - tileX) / 2
            let y = top + Double(at / 4) * pitchY + (pitchY - tileY) / 2
            let tile = CGRect(x: x, y: y, width: tileX, height: tileY)
            context.setFillColor(themed(tiles["\(at + 1)"] ?? fixture.palette[cell]!))
            context.addPath(CGPath(roundedRect: tile, cornerWidth: 6, cornerHeight: 6, transform: nil))
            context.fillPath()
            if draw["digits"] as! Bool { text(String(cell), at: CGPoint(x: tile.midX, y: tile.midY), size: 22, in: context) }
        }
    }

    private func cover(_ occluder: [String: Any], _ context: CGContext) {
        let (x, y, w, h) = (occluder["x"] as! Double, occluder["y"] as! Double, occluder["w"] as! Double,
                            occluder["h"] as! Double)
        let rgb = occluder["rgb"] as! [Double]
        context.setFillColor(CGColor(srgbRed: rgb[0] / 255, green: rgb[1] / 255, blue: rgb[2] / 255, alpha: 1))
        switch occluder["shape"] as! String {
        case "ellipse": context.fillEllipse(in: CGRect(x: x, y: y, width: w, height: h))
        case "arrow":
            let points = [(0.0, 0.0), (0.0, 1.0), (0.3, 0.75), (0.55, 1.0), (0.7, 0.9), (0.45, 0.68), (1.0, 0.68)]
            context.addLines(between: points.map { CGPoint(x: x + $0.0 * w, y: y + $0.1 * h) })
            context.closePath()
            context.fillPath()
        default: context.fill(CGRect(x: x, y: y, width: w, height: h))
        }
    }

    private func text(_ words: String, at centre: CGPoint, size: CGFloat, in context: CGContext) {
        let font = CTFontCreateWithName("Menlo-Bold" as CFString, size, nil)
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: words, attributes: [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTForegroundColorAttributeName as String): ink,
        ]))
        let bounds = CTLineGetBoundsWithOptions(line, .useGlyphPathBounds)
        context.textPosition = CGPoint(x: centre.x - bounds.midX, y: centre.y + bounds.midY)
        CTLineDraw(line, context)
    }

    private func turned(_ image: CGImage, by degrees: Int) throws -> CGImage {
        let quarter = degrees % 180 != 0
        let (width, height) = quarter ? (image.height, image.width) : (image.width, image.height)
        let context = try canvas(width, height)
        context.translateBy(x: CGFloat(width) / 2, y: CGFloat(height) / 2)
        context.rotate(by: CGFloat(degrees) * .pi / 180)
        context.draw(image, in: CGRect(x: -CGFloat(image.width) / 2, y: -CGFloat(image.height) / 2,
                                       width: CGFloat(image.width), height: CGFloat(image.height)))
        return context.makeImage()!
    }

    private func stretched(_ image: CGImage, _ by: [Double]) throws -> CGImage {
        let (width, height) = (Int((Double(image.width) * by[0]).rounded()), Int((Double(image.height) * by[1]).rounded()))
        let context = try canvas(width, height)
        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return context.makeImage()!
    }
}

struct ProbeError: Error, CustomStringConvertible {
    let description: String
}

/// An sRGB BGRA bitmap context, the layout the helper's frames have.
func canvas(_ width: Int, _ height: Int, data: UnsafeMutableRawPointer? = nil, bytesPerRow: Int = 0) throws -> CGContext {
    guard let context = CGContext(data: data, width: width, height: height, bitsPerComponent: 8, bytesPerRow: bytesPerRow,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue |
                                      CGBitmapInfo.byteOrder32Little.rawValue)
    else { throw ProbeError(description: "no \(width)×\(height) context") }
    return context
}

func write(_ image: CGImage, to url: URL) throws {
    guard let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else {
        throw ProbeError(description: "cannot write \(url.lastPathComponent)")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else { throw ProbeError(description: "cannot finish \(url.lastPathComponent)") }
}

// MARK: - Timing

/// An IOSurface-backed BGRA pixel buffer holding a drawing — the kind of buffer a display stream
/// hands the helper — with the lock held only while a caller reads it.
final class Frame {
    let buffer: CVPixelBuffer
    let width: Int
    let height: Int

    init(_ image: CGImage) throws {
        var made: CVPixelBuffer?
        let attributes = [kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any]()] as CFDictionary
        guard CVPixelBufferCreate(nil, image.width, image.height, kCVPixelFormatType_32BGRA, attributes, &made) == kCVReturnSuccess,
              let made else { throw ProbeError(description: "no pixel buffer") }
        CVPixelBufferLockBaseAddress(made, [])
        defer { CVPixelBufferUnlockBaseAddress(made, []) }
        let context = try canvas(image.width, image.height, data: CVPixelBufferGetBaseAddress(made),
                                 bytesPerRow: CVPixelBufferGetBytesPerRow(made))
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        buffer = made
        (width, height) = (image.width, image.height)
    }

    func read<T>(_ body: (ReflexPixels) -> T) -> T {
        CVPixelBufferLockBaseAddress(buffer, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
        return body(ReflexPixels(base: CVPixelBufferGetBaseAddress(buffer)!, width: width, height: height,
                                 bytesPerRow: CVPixelBufferGetBytesPerRow(buffer)))
    }
}

func hostNanoseconds() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }

func load() -> Double {
    var averages = [0.0, 0.0, 0.0]
    return getloadavg(&averages, 3) == 3 ? averages[0] : -1
}

struct Measure {
    let fixture: Fixture
    let pictures: URL
    /// Observations timed per series after the warm-up, and the warm-up itself.
    let timed = 400, warm = 40

    func facts(_ frame: Frame, capture: UInt64, colorSpace: ReflexColorSpace = .srgb,
               orientation: ReflexOrientation = .up) -> ReflexFrameFacts {
        let captured = hostNanoseconds()
        return ReflexFrameFacts(
            run_id: "probe", display_id: "probe",
            region: ReflexRoi(x: 0, y: 0, width: Int64(frame.width), height: Int64(frame.height), space: .pixel),
            pixel_extent: ReflexPixelExtent(width: UInt32(frame.width), height: UInt32(frame.height)),
            point_transform: ReflexPointTransform(origin_x: 0, origin_y: 0,
                                                  points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
            orientation: orientation, color_space: colorSpace, status: .ready, dirty: true, capture_gap: 0,
            delivered_host_ns: captured, capture_seq: capture, repaint_seq: capture, stream_epoch: 1, owner_epoch: 1,
            geometry_epoch: 1, plan_epoch: 1, clock_domain: 1, captured_host_ns: captured)
    }

    /// Times one session over one frame: the first observation apart, then the warm ones.
    func series(_ name: String, _ session: PerceptionSession, _ frame: Frame, limits: PerceptionLimits,
                extra: [String: Any] = [:]) -> [String: Any] {
        var nanoseconds: [UInt64] = []
        var work: [UInt64] = []
        var cold: UInt64 = 0
        let before = load()
        for capture in 1...UInt64(warm + timed + 1) {
            let frameFacts = facts(frame, capture: capture)
            var budget = ReflexPerceptionBudget(samples: limits.max_tick_samples, deadlineHostNs: .max, now: hostNanoseconds)
            let start = hostNanoseconds()
            let observations = frame.read { session.observe(frame: frameFacts, pixels: $0, budget: &budget) }
            let spent = hostNanoseconds() - start
            if capture == 1 { cold = spent } else if capture > UInt64(warm + 1) {
                nanoseconds.append(spent)
                work.append(observations.reduce(0) { $0 + $1.samples })
            }
            precondition(!observations.contains { $0.unknown == .budget }, "\(name) ran out of budget")
        }
        return extra.merging([
            "series": name, "cold_ns": cold, "ns": nanoseconds, "work": work.max() ?? 0,
            "load_before": before, "load_after": load(), "frame": [frame.width, frame.height],
        ]) { current, _ in current }
    }

    /// The whole suite four times in one process, the thread's QoS in ABBA order — the default
    /// class, then user-interactive, the class a frame-rate evaluation thread can ask for — so both
    /// are read under the same machine state. The QoS the process started with is recorded apart.
    func run() throws -> [String: Any] {
        let limits = try PerceptionSpecs.decodeLimits(
            Data(try Data(contentsOf: pictures.deletingLastPathComponent().appendingPathComponent("limits.json")).dropLast()))
        let started = qos_class_self()
        var rows: [[String: Any]] = []
        for (block, qos) in [(0, QOS_CLASS_DEFAULT), (1, QOS_CLASS_USER_INTERACTIVE), (2, QOS_CLASS_USER_INTERACTIVE),
                             (3, QOS_CLASS_DEFAULT)] {
            guard pthread_set_qos_class_self_np(qos, 0) == 0, qos_class_self() == qos else {
                throw ProbeError(description: "cannot set QoS \(qos.rawValue)")
            }
            let name = qos == QOS_CLASS_USER_INTERACTIVE ? "user_interactive" : "default"
            rows += try suite(limits).map { $0.merging(["block": block, "qos": name]) { current, _ in current } }
        }
        return ["limits": try JSONSerialization.jsonObject(with: JSONEncoder().encode(limits)), "rows": rows,
                "timed": timed, "warm": warm, "started_qos": started.rawValue]
    }

    func suite(_ limits: PerceptionLimits) throws -> [[String: Any]] {
        var rows: [[String: Any]] = []
        // The fixture game's board on every clean held-out scene.
        for scene in fixture.scenes where scene.split == "heldout" && scene.id.range(of: #"^heldout-(\d\d|twice-.|three-quarters)$"#,
                                                                                      options: .regularExpression) != nil {
            let source = CGImageSourceCreateWithURL(pictures.appendingPathComponent("\(scene.id).png") as CFURL, nil)!
            let frame = try Frame(CGImageSourceCreateImageAtIndex(source, 0, nil)!)
            let session = try PerceptionSession(detectors: [("board", fixture.roi, fixture.spec)], limits: limits)
            rows.append(series("cells", session, frame, limits: limits, extra: ["scene": scene.id]))
        }
        // The largest detector the table allows, and a tick of them up to the tick's sample limit:
        // red squares on ground over a 1280 × 800 frame, few enough per ROI to be followed, sampled at
        // every pixel.
        let side = Int(Double(limits.max_detector_samples).squareRoot())
        let wide = try canvas(1280, 800)
        wide.setFillColor(CGColor(srgbRed: 18 / 255, green: 18 / 255, blue: 24 / 255, alpha: 1))
        wide.fill(CGRect(x: 0, y: 0, width: 1280, height: 800))
        wide.setFillColor(CGColor(srgbRed: 230 / 255, green: 40 / 255, blue: 40 / 255, alpha: 1))
        for at in 0..<32 { wide.fill(CGRect(x: (at * 197) % 1240, y: (at * 113) % 760, width: 12, height: 12)) }
        let frame = try Frame(wide.makeImage()!)
        let blobs = PerceptionColorSpec(
            space: .srgb, classes: [PerceptionColorClass(r: 230, g: 40, b: 40, tolerance: 28)],
            ground: PerceptionColorClass(r: 18, g: 18, b: 24, tolerance: 12), reference_width: 1280, reference_height: 800,
            layout: .blobs(PerceptionBlobs(class: 1, step: 1, min_samples: 4, gate: 16, max_blobs: limits.max_blobs)),
            confirm: 1, anchors: [])
        let detectors = Int(limits.max_tick_samples / limits.max_detector_samples)
        let rois = (0..<detectors).map { at in
            ReflexRoi(x: Int64(at % 2 * (1280 - side)), y: Int64(at / 2 % 2 * (800 - side)), width: Int64(side),
                      height: Int64(side), space: .pixel)
        }
        rows.append(series("blobs-detector", try PerceptionSession(detectors: [("blobs", rois[0], blobs)], limits: limits),
                           frame, limits: limits))
        rows.append(series("blobs-tick", try PerceptionSession(detectors: rois.enumerated().map { ("b\($0.offset)", $0.element, blobs) },
                                                               limits: limits), frame, limits: limits, extra: ["detectors": detectors]))
        // Template and motion: kernels no plan runs yet, timed over fixed ROIs and patches.
        rows.append(contentsOf: try kernels(frame))
        return rows
    }

    func kernels(_ frame: Frame) throws -> [[String: Any]] {
        var rows: [[String: Any]] = []
        let patch = (0..<256).map { at -> UInt32 in (at / 16 + at % 16) % 3 == 0 ? 0xFF_E6_28_28 : 0xFF_18_12_12 }
        let template = PerceptionTemplate(width: 16, height: 16, pixels: patch)!
        let roi = ReflexRoi(x: 64, y: 64, width: 128, height: 128, space: .pixel)
        for (name, search) in [("template-128-step2-stride2", PerceptionTemplateSearch(step: 2, stride: 2, accept: 8, margin: 16)),
                               ("template-128-step1-stride1", PerceptionTemplateSearch(step: 1, stride: 1, accept: 8, margin: 16))] {
            var nanoseconds: [UInt64] = []
            let before = load()
            for round in 0..<(warm + timed) {
                var samples = UInt64.max
                let start = hostNanoseconds()
                _ = frame.read { PerceptionTemplates.find(template, in: PerceptionPlane($0), roi: roi, search: search, samples: &samples) }
                if round >= warm { nanoseconds.append(hostNanoseconds() - start) }
            }
            rows.append(["series": name, "ns": nanoseconds, "work": PerceptionTemplates.cost(template, roi: roi, search: search),
                         "load_before": before, "load_after": load()])
        }
        var motion = PerceptionMotion(step: 4, threshold: 24, cutPermille: 500)
        let field = ReflexRoi(x: 0, y: 0, width: 256, height: 256, space: .pixel)
        var nanoseconds: [UInt64] = []
        let before = load()
        for round in 0..<(warm + timed) {
            let start = hostNanoseconds()
            _ = frame.read { motion.observe(PerceptionPlane($0), roi: field, minSamples: 2) }
            if round >= warm { nanoseconds.append(hostNanoseconds() - start) }
        }
        rows.append(["series": "motion-256-step4", "ns": nanoseconds, "work": 64 * 64, "load_before": before, "load_after": load()])
        return rows
    }
}

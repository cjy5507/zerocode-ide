import Foundation
import Vision
import ImageIO

// Persistent line protocol: excludes process launch, includes image decode.
while let path = readLine() {
    let start = DispatchTime.now().uptimeNanoseconds
    do {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = false
        try VNImageRequestHandler(url: URL(fileURLWithPath: path)).perform([request])
        let found = (request.results ?? []).compactMap { observation -> [String: Any]? in
            guard let text = observation.topCandidates(1).first?.string else { return nil }
            let b = observation.boundingBox
            return ["text": text, "x": b.midX, "y": 1 - b.midY]
        }
        let ms = Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
        let data = try JSONSerialization.data(withJSONObject: ["ms": ms, "words": found])
        print(String(decoding: data, as: UTF8.self))
        fflush(stdout)
    } catch { print("{\"error\":\"ocr failed\"}"); fflush(stdout) }
}

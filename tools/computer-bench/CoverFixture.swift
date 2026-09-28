// Another app over the reflex fixture, for a covered round (t-12979): an app
// of its own process and bundle that puts one window — an ordinary window, or
// a dialog waiting for an answer — at a place on the screen at a moment of the
// round, never activates itself, and counts every press that window takes.
// It keeps no number of its own: the runner hands it the kind, the place in
// the screen's top-left points and the moment on the host's uptime clock.
import AppKit

@MainActor
final class CoverApp: NSObject, NSApplicationDelegate {
    let kind: String
    let quartz: CGRect
    let appearNs: UInt64
    let recorder: Recorder
    var sheet: CoverSheet?
    var lifetime: FixtureLifetime?
    var timer: Timer?

    init(kind: String, quartz: CGRect, appearNs: UInt64, recorder: Recorder) {
        self.kind = kind
        self.quartz = quartz
        self.appearNs = appearNs
        self.recorder = recorder
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        sheet = CoverSheet(kind: kind, quartz: quartz)
        lifetime = FixtureLifetime(flush: { [weak self] wait in self?.flush(wait: wait) }, becameActive: {})
        let now = uptimeNs()
        let delay = appearNs > now ? Double(appearNs - now) / 1_000_000_000 : 0
        timer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.sheet?.show() }
        }
        recorder.write("ready.json", ["pid": Int(getpid()), "bundleId": Bundle.main.bundleIdentifier ?? "",
                                      "readyNs": uptimeNs()])
    }

    func flush(wait: Bool) {
        recorder.flush(state: sheet?.state ?? [:], wait: wait)
    }
}

@main
@MainActor
struct CoverFixtureMain {
    static func main() {
        let arguments = CommandLine.arguments
        guard arguments.count == 8,
              let x = Double(arguments[2]), let y = Double(arguments[3]),
              let width = Double(arguments[4]), let height = Double(arguments[5]),
              let appearNs = UInt64(arguments[6])
        else {
            FileHandle.standardError.write(Data("usage: CoverFixture <kind> <x> <y> <width> <height> <appear-uptime-ns> <state-folder>\n".utf8))
            exit(2)
        }
        let folder = URL(fileURLWithPath: arguments[7], isDirectory: true)
        let cover = CoverApp(kind: arguments[1], quartz: CGRect(x: x, y: y, width: width, height: height),
                             appearNs: appearNs, recorder: Recorder(folder: folder))
        let app = NSApplication.shared
        // A regular app, so its window is another app's to the window list; it
        // never activates itself.
        app.setActivationPolicy(.regular)
        app.delegate = cover
        withExtendedLifetime(cover) { app.run() }
    }
}

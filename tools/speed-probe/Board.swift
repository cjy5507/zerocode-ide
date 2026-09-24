import UIKit
import SpriteKit

struct Fixture: Decodable {
    let rows: Int, columns: Int
    let x0: Double, y0: Double, dx: Double, dy: Double, width: Double, height: Double
    let colours: [[Double]]
}

// An opaque, Metal-backed board with a separate oracle. No private app data.
final class Board: SKScene {
    var turn = 0
    let stimuli = CommandLine.arguments.contains("--stimuli")
    var stimulusAt = 0.0
    let fixture = try! JSONDecoder().decode(Fixture.self, from: Data(contentsOf:
        Bundle.main.url(forResource: "board", withExtension: "json")!))
    override func didMove(to view: SKView) {
        paint()
        if stimuli {
            try? Data().write(to: reactions)
            run(.repeatForever(.sequence([.wait(forDuration: 0.25), .run { [weak self] in
                guard let self else { return }
                self.turn += 1
                self.stimulusAt = ProcessInfo.processInfo.systemUptime
                self.paint()
            }])))
        }
    }
    var reactions: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("reactions.jsonl")
    }
    func paint() {
        removeAllChildren()
        backgroundColor = .black
        var cells: [Int] = []
        let colours = fixture.colours.map { UIColor(red: $0[0]/255, green: $0[1]/255,
                                                    blue: $0[2]/255, alpha: 1) }
        for i in 0..<(fixture.rows * fixture.columns) {
            let value = (i + turn) % colours.count
            cells.append(value)
            let box = SKShapeNode(rectOf: CGSize(width: size.width * fixture.width,
                                                height: size.height * fixture.height))
            box.fillColor = colours[value]
            box.strokeColor = .clear
            box.position = CGPoint(x: size.width * (fixture.x0 + Double(i % fixture.columns) * fixture.dx),
                                   y: size.height * (1 - fixture.y0 - Double(i / fixture.columns) * fixture.dy))
            let label = SKLabelNode(fontNamed: "Menlo-Bold")
            label.text = String(value + 1)
            label.fontColor = .white
            label.fontSize = 32
            label.verticalAlignmentMode = .center
            box.addChild(label)
            addChild(box)
        }
        let oracle: [String: Any] = ["turn": turn, "cells": cells,
                                      "uptime": ProcessInfo.processInfo.systemUptime]
        let file = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("oracle.json")
        try? JSONSerialization.data(withJSONObject: oracle).write(to: file, options: .atomic)
    }
    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        if stimuli {
            guard let location = touches.first?.location(in: self) else { return }
            let centres = (0..<(fixture.rows * fixture.columns)).map { i in
                CGPoint(x: size.width * (fixture.x0 + Double(i % fixture.columns) * fixture.dx),
                        y: size.height * (1 - fixture.y0 - Double(i / fixture.columns) * fixture.dy))
            }
            let target = centres.indices.min { a, b in
                hypot(centres[a].x-location.x,centres[a].y-location.y) < hypot(centres[b].x-location.x,centres[b].y-location.y)
            }!
            let firstRed = centres.indices.first { ($0 + turn) % fixture.colours.count == 0 }!
            let row: [String: Any] = ["turn":turn,"response_ms":(ProcessInfo.processInfo.systemUptime-stimulusAt)*1000,
                                       "correct":target == firstRed]
            if var data = try? JSONSerialization.data(withJSONObject:row),
               let file = try? FileHandle(forWritingTo: reactions) {
                data.append(10)
                _ = try? file.seekToEnd(); try? file.write(contentsOf:data); try? file.close()
            }
            return
        }
        turn += 1
        paint()
    }
}

final class App: UIResponder, UIApplicationDelegate {
    var window: UIWindow?
    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        let win = UIWindow(frame: UIScreen.main.bounds)
        let vc = UIViewController()
        let view = SKView(frame: win.bounds)
        view.preferredFramesPerSecond = 60
        view.isAccessibilityElement = true
        view.accessibilityLabel = "Game board controls"
        view.accessibilityTraits = .button
        vc.view = view
        win.rootViewController = vc
        window = win
        win.makeKeyAndVisible()
        view.presentScene(Board(size: view.bounds.size))
        return true
    }
}

UIApplicationMain(CommandLine.argc, CommandLine.unsafeArgv, nil, NSStringFromClass(App.self))

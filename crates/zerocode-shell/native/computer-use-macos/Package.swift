// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "ZeroCodeComputerUseMacOS",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .library(
            name: "ZeroCodeComputerUseMacOSCore",
            targets: ["ZeroCodeComputerUseMacOSCore"]
        ),
        .executable(
            name: "zerocode-computer-use-macos",
            targets: ["ZeroCodeComputerUseMacOS"]
        )
    ],
    targets: [
        .target(
            name: "ZeroCodeComputerUseMacOSCore",
            path: "Sources/ZeroCodeComputerUseMacOSCore"
        ),
        .executableTarget(
            name: "ZeroCodeComputerUseMacOS",
            dependencies: ["ZeroCodeComputerUseMacOSCore"],
            path: "Sources/ZeroCodeComputerUseMacOS"
        ),
        // The tests read the helper itself too — its dispatch, its stop road
        // and its one hand — not only the pure Core it calls.
        .testTarget(
            name: "ZeroCodeComputerUseMacOSTests",
            dependencies: ["ZeroCodeComputerUseMacOSCore", "ZeroCodeComputerUseMacOS"],
            path: "Tests/ZeroCodeComputerUseMacOSTests"
        )
    ]
)

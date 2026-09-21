// swift-tools-version: 6.0

import PackageDescription

// The helper itself is two files at this root that `build.rs` hands straight
// to `swiftc`: they reach private CoreSimulator and AXPTranslator classes at
// runtime and cannot be exercised off a booted simulator. What can be judged
// without one lives in the Core target below, which `swiftc` compiles into
// the same module and `swift test` builds on its own.
let package = Package(
    name: "ZeroCodeIosEmulatorHelper",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .library(
            name: "ZeroCodeIosEmulatorHelperCore",
            targets: ["ZeroCodeIosEmulatorHelperCore"]
        )
    ],
    targets: [
        .target(
            name: "ZeroCodeIosEmulatorHelperCore",
            path: "Sources/ZeroCodeIosEmulatorHelperCore"
        ),
        .testTarget(
            name: "ZeroCodeIosEmulatorHelperCoreTests",
            dependencies: ["ZeroCodeIosEmulatorHelperCore"],
            path: "Tests/ZeroCodeIosEmulatorHelperCoreTests"
        )
    ]
)

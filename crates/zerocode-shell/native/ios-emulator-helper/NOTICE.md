# iOS emulator input helper attribution

The SimulatorKit HID protocol and dynamic-framework loading approach in
`main.swift` are derived from the `serve-sim` project:

- Project: https://github.com/EvanBacon/serve-sim
- Reference revision: `14ad57ff922551bf7be81e907ddfcfa6191e64f2`
- License: Apache License 2.0 (see `LICENSE-APACHE-2.0`)

ZeroCode's helper is an independent, reduced implementation of that protocol.
It does not include or load Orca application artifacts.

`AccessibilityBridge.swift` also derives from Meta's `idb`
`FBSimulatorAccessibilityCommands` under the MIT license; see
`LICENSE-IDB-MIT`.

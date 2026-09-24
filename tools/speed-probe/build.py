#!/usr/bin/env python3
"""Compile the fixture and the unchanged production helper; touches no devices."""
import argparse
from pathlib import Path
import plistlib
import shutil
import subprocess


def build(output):
    source = Path(__file__).resolve().parent
    root = source.parents[1]
    out = Path(output).resolve()
    out.mkdir(parents=True, exist_ok=True)
    app = out / "SpeedProbe.app"
    app.mkdir(exist_ok=True)
    (app / "Info.plist").write_bytes(plistlib.dumps(dict(
        CFBundleIdentifier="test.zerocode.speedprobe", CFBundleExecutable="SpeedProbe",
        CFBundleName="SpeedProbe", CFBundlePackageType="APPL", CFBundleVersion="1",
        CFBundleShortVersionString="1.0", UILaunchScreen={},
        UISupportedInterfaceOrientations=["UIInterfaceOrientationPortrait"])))
    shutil.copyfile(source / "board.json", app / "board.json")
    sdk = subprocess.check_output(["xcrun", "--sdk", "iphonesimulator", "--show-sdk-path"], text=True).strip()
    subprocess.run(["xcrun", "--sdk", "iphonesimulator", "swiftc", str(source / "Board.swift"),
                    "-sdk", sdk, "-target", "arm64-apple-ios18.0-simulator", "-O", "-o", str(app / "SpeedProbe")], check=True)
    subprocess.run(["codesign", "-s", "-", "-f", str(app)], check=True)
    helper = root / "crates/zerocode-shell/native/ios-emulator-helper"
    inputs = [helper / "main.swift", helper / "AccessibilityBridge.swift",
              *sorted((helper / "Sources/ZeroCodeIosEmulatorHelperCore").glob("*.swift"))]
    subprocess.run(["xcrun", "--sdk", "macosx", "swiftc", *map(str, inputs), "-O",
                    "-whole-module-optimization", "-target", "arm64-apple-macosx14.0",
                    "-o", str(out / "helper")], check=True)
    subprocess.run(["xcrun", "--sdk", "macosx", "swiftc", str(source / "ocr.swift"),
                    "-O", "-o", str(out / "ocr")], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    build(parser.parse_args().output)

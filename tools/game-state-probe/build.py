#!/usr/bin/env python3
"""Builds the game-state probe: probe.swift and the helper's Core sources as one optimised module.

    python3 tools/game-state-probe/build.py --output <scratch>/probe

The binary runs the product's own perception code — nothing is copied — so its timings are the
kernel's. The output belongs outside git.
"""
import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "crates/zerocode-shell/native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore"


def command(output):
    sources = sorted(str(path) for path in CORE.rglob("*.swift"))
    return ["swiftc", "-O", "-swift-version", "6", "-parse-as-library", "-module-name", "GameStateProbe",
            *sources, str(Path(__file__).with_name("probe.swift")), "-o", str(output)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    output = Path(parser.parse_args().output)
    output.parent.mkdir(parents=True, exist_ok=True)
    sys.exit(subprocess.run(command(output)).returncode)


if __name__ == "__main__":
    main()

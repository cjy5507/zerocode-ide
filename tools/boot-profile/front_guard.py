#!/usr/bin/env python3
"""Whether a measured window stayed out of the person's way (t-82914).

usage: front_guard.py front         the pid of the app in front now
       front_guard.py check <pid>   exit 0 when <pid> neither took the front nor
                                    put a window above the front app's windows

A window measured while the person works must not take their keyboard and must
not stand over what they are looking at. `measure.sh --launch background`
asks this right after each start and ends the window the moment either is
untrue. What it reads is LaunchServices' front application and the window
server's front-to-back order of on-screen windows by owner pid — no window
title, no picture, nothing a window shows.

Exit codes: 0 out of the way, 4 took the front, 5 stands above the front app's
windows (or above every window, when the front app shows none).
"""
import ctypes
import re
import subprocess
import sys

# CGWindowListCopyWindowInfo options: on screen only, without desktop elements.
ON_SCREEN_ONLY = 1
EXCLUDE_DESKTOP = 16
# kCFNumberSInt64Type, kCFStringEncodingUTF8.
SINT64 = 4
UTF8 = 0x08000100
# Ordinary windows; menus, the Dock and overlays sit on other layers.
NORMAL_LAYER = 0


def front_pid():
    asn = subprocess.run(["lsappinfo", "front"], capture_output=True, text=True).stdout.strip()
    said = subprocess.run(["lsappinfo", "info", "-only", "pid", asn], capture_output=True, text=True).stdout
    found = re.search(r'"pid"\s*=\s*(\d+)', said)
    return int(found.group(1)) if found else None


def window_owners():
    """Owner pids of the on-screen ordinary windows, front to back."""
    cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    cf = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    cg.CGWindowListCopyWindowInfo.restype = ctypes.c_void_p
    cg.CGWindowListCopyWindowInfo.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    cf.CFArrayGetCount.restype = ctypes.c_long
    cf.CFArrayGetCount.argtypes = [ctypes.c_void_p]
    cf.CFArrayGetValueAtIndex.restype = ctypes.c_void_p
    cf.CFArrayGetValueAtIndex.argtypes = [ctypes.c_void_p, ctypes.c_long]
    cf.CFDictionaryGetValue.restype = ctypes.c_void_p
    cf.CFDictionaryGetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    cf.CFNumberGetValue.restype = ctypes.c_bool
    cf.CFNumberGetValue.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p]
    cf.CFStringCreateWithCString.restype = ctypes.c_void_p
    cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
    cf.CFRelease.argtypes = [ctypes.c_void_p]
    owner = cf.CFStringCreateWithCString(None, b"kCGWindowOwnerPID", UTF8)
    layer = cf.CFStringCreateWithCString(None, b"kCGWindowLayer", UTF8)

    def number(row, key):
        value = cf.CFDictionaryGetValue(row, key)
        held = ctypes.c_int64()
        return held.value if value and cf.CFNumberGetValue(value, SINT64, ctypes.byref(held)) else None

    rows = cg.CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP, 0)
    try:
        listed = [cf.CFArrayGetValueAtIndex(rows, at) for at in range(cf.CFArrayGetCount(rows))]
        return [number(row, owner) for row in listed if number(row, layer) == NORMAL_LAYER]
    finally:
        cf.CFRelease(rows)
        cf.CFRelease(owner)
        cf.CFRelease(layer)


def check(pid):
    front = front_pid()
    if front == pid:
        print(f"the measured window {pid} took the front")
        return 4
    owners = window_owners()
    mine = owners.index(pid) if pid in owners else None
    theirs = owners.index(front) if front in owners else None
    if mine is not None and (mine == 0 if theirs is None else mine < theirs):
        print(f"the measured window {pid} stands above the front app's windows")
        return 5
    print(f"the measured window {pid} stays out of the way ({'not on screen' if mine is None else 'behind'})")
    return 0


def main():
    if sys.argv[1:2] == ["front"]:
        print(front_pid() or "")
        return 0
    if len(sys.argv) == 3 and sys.argv[1] == "check":
        return check(int(sys.argv[2]))
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())

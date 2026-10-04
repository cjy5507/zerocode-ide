---
name: mobile-device
description: Use when the person says a phone is connected, open in the IDE or has the app installed (iPhone, iPad, Android phone, simulator, emulator; 아이폰, 아이패드, 안드로이드 폰, 시뮬레이터, 에뮬레이터); start with zerocode-emulator list, then boot or open the device, launch the app, see the screen, tap, press buttons, type, swipe, screenshot, and only then turn to the Mac window list, iPhone Mirroring or Xcode devicectl. 연결·설치된 기기의 화면 확인과 탭, 입력, 스와이프, 스크린샷 조작에 쓴다.
invocation: auto
keywords: [아이폰, 아이패드, 안드로이드, 시뮬레이터, 에뮬레이터, iphone, ipad, android, simulator, emulator]
---

# Mobile devices — iPhone, iPad, Android

ZeroCode's window carries its own iOS Simulator and Android Emulator and drives
them directly through one command, `zerocode-emulator`. When the person says a
phone is "connected", "open in the IDE" or "has the app installed" (아이폰
연결해놨어, 앱 설치해뒀어), start with `zerocode-emulator list`: one call tells
you which road the device takes. Do not hunt for it first in the Mac window
list, Xcode's `devicectl` or `adb`, and never click the Simulator or Emulator
window with `zerocode-computer`: the window already holds the device list and
the input road.

This is the one place that holds the mobile commands. The `computer-use` skill
points here and keeps no copy.

## 1. First step, every time

```text
zerocode-emulator list --json
```

One call answers "nothing", "these" or "that one, not supported". With `--json`
the answer is `{"ok": true, "result": {…}}`; read `result` in this order:

- `ios`, `android`: the iOS Simulators and Android emulators this window can
  drive. A row carries the id every other command takes (`udid` on iOS, `avd`
  or `serial` on Android) and `booted`. Never assume a specific phone family
  or model, AVD name, UDID, or Android serial; use what `list` gave.
- `physical`: real iPhones, iPads and Android phones the Mac sees. Every row
  has `drivable: false` and a plain `reason` that names the road the device
  takes instead; at most eight rows come, and `omitted` counts the rest. Go to
  "A device this window cannot drive".
- `unchecked`: a tool did not answer, and `why` says which. A gap is not
  "nothing is connected". When `why` says it is still looking, run `list` again
  in a few seconds.
- All empty and no `unchecked`: nothing is connected or installed. Say that.

An answer with a phone this window cannot drive and nothing it can (the names
here are made up):

```json
{"ok": true, "result": {"surface": "zerocode-built-in", "ios": [], "android": [],
 "physical": [{"platform": "ios", "name": "Synthetic Phone A", "model": "Synthetic iPhone",
   "state": "connected", "drivable": false,
   "reason": "A real iPhone. zerocode-emulator cannot drive it; the Mac's iPhone Mirroring window can, through zerocode-computer (--app com.apple.ScreenContinuity). If that window is not open, ask the person to open it once; do not open the app or unlock anything yourself."}]}}
```

## 2. Nothing booted: open one

```text
zerocode-emulator open --platform ios|android [--device <listed-id>] --json
```

Let `open` boot the device rather than booting it yourself. The mirror it opens
is seated in your own pane's checkout, not in front of whatever the person is
looking at. A device `open` boots is lent to your pane, and ZeroCode shuts it
down when your pane closes, when you send `worker_done`, or when your session
ends. A device that was already running is left as it was, and a device you
created for the task is still yours to delete. Omit `--device` to let the
window choose a running device or the first installed one.

`open` answers at once. Poll `list --json` until the chosen row says
`booted: true`; do not fall back to desktop clicks while it starts.

## 3. Look, act, look again

Numbers beat coordinates: a look numbers the controls a person could press, and
a press names one of them.

```text
zerocode-emulator marks --platform ios|android --device <id> [--text <fragment>] --json
zerocode-emulator click --platform ios|android --device <id> --mark <n> --look <lookId> [--text <fragment>] [--preview] --json
```

`marks` answers a `lookId` and a numbered `legend`. `click` presses mark `n` of
that look and is refused when the screen moved since the look; look again and
press the new number. On iOS a click answers once the screen it led to stops
changing. Look again after every action, because the tree goes stale at once.
To open an app that is installed, press its icon the same way.

Checks answer a count, where 0 means absent:

```text
zerocode-emulator find --platform ios|android --device <id> --text <fragment> --json
zerocode-emulator foreground --platform ios|android --device <id> --app <package|bundle> --json
```

When no mark fits, or a gesture is the point, act by position. Coordinates are
fractions of the device screen (0 to 1), not desktop pixels:

```text
zerocode-emulator tap --platform ios|android --device <id> --x <0..1> --y <0..1> --json
zerocode-emulator swipe --platform ios|android --device <id> --x1 <0..1> --y1 <0..1> --x2 <0..1> --y2 <0..1> [--ms <50..3000>] --json
zerocode-emulator text --platform ios|android --device <id> --text <text> --json
zerocode-emulator button --platform ios|android --device <id> --name <button> --json
zerocode-emulator rotate --platform ios|android --device <id> --rotation <0..3> --json
zerocode-emulator tree --platform ios|android --device <id> --json
```

`text` types into the field that has focus, so press the field first. `button`
names: `home`, `lock`, `volume-up`, `volume-down`, `enter` on both platforms;
`back` and `recents` on Android; `action` on iOS. `tree` is the raw
accessibility tree, for when `marks` is not enough.

## 4. Show the screen

```text
zerocode-emulator screenshot --platform ios|android --device <id> [--out <path>] --json
```

The PNG goes to a file and the answer gives its path; image bytes are never
printed. Omit `--out` for a private scratch path, or pass a destination when
the picture belongs in the worktree (a relative `--out` starts from the
shell's own folder).

## A device this window cannot drive

`physical` rows are real hardware. `zerocode-emulator` drives simulators and
emulators only: it has no road to press, type into or photograph a real device,
and `list` never offers one as drivable. Each row's `reason` says the road the
device takes instead.

**A real iPhone** is what "the iPhone is connected" nearly always means. Drive
it through the Mac's iPhone Mirroring window with `zerocode-computer`, the
desktop road in the `computer-use` skill. Name the app by its id,
`com.apple.ScreenContinuity`: the window is titled "iPhone Mirroring" in
English and "iPhone 미러링" in Korean, a name that does not match is a window
that is not found, and the id is the same in every language.

1. Look for the window:
   `zerocode-computer list-windows --app com.apple.ScreenContinuity --json`.
2. A window is up: look, press and look again as the `computer-use` skill
   says, always with that `--app`.
3. No window (`app_not_found` or `window_not_found`, not a sign the app does
   not exist): ask the person once to open iPhone Mirroring on the Mac, then
   look again; do not open the app for them and do not unlock anything, they
   set this up and they unlock it.
4. It is their own phone: do what they asked and stop before anything they did
   not (a message sent, a purchase, a deletion).

**A real iPad** has no road from this window: iPhone Mirroring shows iPhones
only. **A real Android phone** has none today either: `zerocode-emulator`
drives Android emulators only. For both, tell the person in two sentences or
fewer what you found: the device `name`, its `state` (`connected`;
`unavailable` when the Mac knows the device but cannot reach it now; on Android
also `unauthorized` when the phone has not allowed this computer, or `offline`),
and that this window cannot drive it. Then offer what you can do: open a
simulator or emulator with `open`, or work on the app's code, build and logs.
Ask before you touch such a device any other way (`adb`, `devicectl`): it is
their device, and one tap on it can send, buy or delete.

A `physical` row is not a device the window shows. If the person says the phone
is open in the IDE and `list` has an `ios` or `android` row, that row is the
one.

On Windows there is no iOS Simulator: `list` shows Android devices only.

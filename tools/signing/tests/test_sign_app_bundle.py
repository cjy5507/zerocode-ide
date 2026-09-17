"""tools/signing/sign-app-bundle.sh signs the whole app inside-out so the
ScreenCapture responsible process (dev.zerocode.app) keeps one requirement,
then runs the package smoke's nested-code gate on what it signed.
Driven with a fake `codesign` (it remembers who signed what and answers
--display from that), a fake `file`, and a fake identity, in a copy of the
repo's layout so the real scripts/native-package-smoke.macos.sh is the gate."""
import os
import pathlib
import plistlib
import shutil
import stat
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SIGNING = HERE.parent
ROOT = SIGNING.parent.parent
SCRIPT = SIGNING / "sign-app-bundle.sh"
SMOKE = ROOT / "scripts" / "native-package-smoke.macos.sh"
CEF_FRAMEWORK = "Chromium Embedded Framework.framework"
CEF_HELPERS = ("ZeroCode Helper", "ZeroCode Helper (GPU)", "ZeroCode Helper (Renderer)",
               "ZeroCode Helper (Plugin)", "ZeroCode Helper (Alerts)")
VERSION = "1.2.3"

# Remembers `--sign` per path; answers `--display` the way codesign does: an
# executable that is its bundle's CFBundleExecutable answers for the bundle,
# anything else for itself, and a Mach-O no one signed carries the linker's
# adhoc signature.
FAKE_CODESIGN = r'''#!/usr/bin/env python3
import os, plistlib, sys
CALLS, STATE = {calls!r}, {state!r}
args = sys.argv[1:]
with open(CALLS, "a") as log:
    log.write(" ".join(args) + "\n")
path = os.path.realpath(args[-1]) if args else ""
if "--sign" in args:
    runtime = "runtime" in args
    with open(STATE, "a") as state:
        state.write(f"{{path}}\t{{args[args.index('--sign') + 1]}}\t{{runtime}}\n")
    sys.exit(0)
if "--display" not in args:
    sys.exit(0)
signed = {{}}
if os.path.exists(STATE):
    for line in open(STATE):
        where, identity, runtime = line.rstrip("\n").split("\t")
        signed[where] = (identity, runtime == "True")
def owner(p):
    parts = p.split(os.sep)
    if len(parts) > 4 and parts[-2] == "MacOS" and parts[-3] == "Contents" and parts[-4].endswith(".app"):
        bundle = os.sep.join(parts[:-3])
        try:
            info = plistlib.load(open(os.path.join(bundle, "Contents", "Info.plist"), "rb"))
        except OSError:
            return p
        if info.get("CFBundleExecutable") == parts[-1]:
            return bundle
    return p
found = signed.get(path) or signed.get(owner(path))
if found is None:
    if os.path.isdir(path):
        sys.stderr.write(f"{{path}}: code object is not signed at all\n")
        sys.exit(1)
    sys.stderr.write("CodeDirectory v=20400 size=1 flags=0x20002(adhoc,linker-signed) hashes=1+0 location=embedded\n"
                     "Signature=adhoc\n")
    sys.exit(0)
identity, runtime = found
flags = "0x10000(runtime)" if runtime else ("0x2(adhoc)" if identity == "-" else "0x0(none)")
sys.stderr.write(f"CodeDirectory v=20500 size=1 flags={{flags}} hashes=1+0 location=embedded\n")
sys.stderr.write("Signature=adhoc\n" if identity == "-" else f"Authority=Local {{identity}}\n")
'''

# `file -b PATH`: a fixture says what it is on its second line.
FAKE_FILE = '''#!/bin/sh
case "$(sed -n 2p "$2" 2>/dev/null)" in
  "# MACH-O executable") echo "Mach-O 64-bit executable arm64" ;;
  "# MACH-O library") echo "Mach-O 64-bit dynamically linked shared library arm64" ;;
  *) echo "ASCII text" ;;
esac
'''


def outer_sign_calls(calls):
    """Indexes of the codesign calls that seal the outer ZeroCode.app itself —
    not a bundle nested under it, not the closing verify, not a --display."""
    return [i for i, c in enumerate(calls) if c.strip().endswith("/ZeroCode.app") and "--sign" in c]


def sign_calls(calls):
    return [c for c in calls if "--sign" in c]


class SignAppBundle(unittest.TestCase):
    def executable(self, path, body="exit 0", kind="executable"):
        """A fixture that `file` calls a Mach-O and that notes it was started."""
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f'#!/bin/sh\n# MACH-O {kind}\necho "{path.name}" >> "{self.ran}"\n{body}\n')
        path.chmod(path.stat().st_mode | stat.S_IEXEC)

    def info(self, bundle, identifier, executable=None, **more):
        (bundle / "Contents").mkdir(parents=True, exist_ok=True)
        values = {"CFBundleIdentifier": identifier, **more}
        if executable:
            values["CFBundleExecutable"] = executable
        (bundle / "Contents" / "Info.plist").write_bytes(plistlib.dumps(values))

    def make_app(self):
        app = self.tmp / "ZeroCode.app"
        contents = app / "Contents"
        self.info(app, "dev.zerocode.app", "zerocode-shell", CFBundleShortVersionString=VERSION)
        self.executable(contents / "MacOS" / "zerocode-shell")
        self.executable(contents / "MacOS" / "zerocode-mirror",
                        'echo "zerocode-mirror: real binary path missing" >&2; exit 127')
        self.executable(contents / "MacOS" / "zerocode-pick",
                        'echo "usage: zerocode-pick --kind folder|folders|file|files" >&2; exit 2')
        helper = contents / "Resources" / "ZeroCode Computer Use.app"
        self.info(helper, "dev.zerocode.app.computer-use", "zerocode-computer-use-macos")
        self.executable(helper / "Contents" / "MacOS" / "zerocode-computer-use-macos",
                        '[ "$1" = --agent ] || exit 0\n'
                        'echo "usage: zerocode-computer-use-macos --agent <socket-path>" >&2; exit 2')
        self.zo(f"zo {VERSION}")
        # The release bundle's Chromium: the framework, a library under it,
        # and its five helper apps (each returns -1 with no --type).
        framework = contents / "Frameworks" / CEF_FRAMEWORK
        self.executable(framework / "Chromium Embedded Framework", kind="library")
        self.executable(framework / "Libraries" / "libEGL.dylib", kind="library")
        for name in CEF_HELPERS:
            bundle = contents / "Frameworks" / f"{name}.app"
            self.info(bundle, "dev.zerocode.app.helper", name)
            self.executable(bundle / "Contents" / "MacOS" / name, "exit 255")
        return app

    def zo(self, says):
        app = self.tmp / "ZeroCode.app"
        self.executable(app / "Contents" / "Resources" / "bin" / "zo", f'[ "$1" = --version ] && echo "{says}"; exit 0')

    def run_with(self, fingerprint, arrange=None):
        tmp = self.tmp = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)
        self.ran = tmp / "ran.log"
        # The repo's layout, so the signer reaches the real smoke beside it.
        signing = tmp / "repo" / "tools" / "signing"
        signing.mkdir(parents=True)
        (tmp / "repo" / "scripts").mkdir()
        script_copy = signing / "sign-app-bundle.sh"
        script_copy.write_text(SCRIPT.read_text())
        (tmp / "repo" / "scripts" / "native-package-smoke.macos.sh").write_text(SMOKE.read_text())
        fake_ident = signing / "ensure-local-identity.sh"
        fake_ident.write_text(f'#!/bin/sh\n{"printf %s " + fingerprint if fingerprint else "exit 1"}\n')
        bindir = tmp / "bin"
        bindir.mkdir()
        calls = tmp / "codesign.calls"
        for name, body in (("codesign", FAKE_CODESIGN.format(calls=str(calls), state=str(tmp / "codesign.state"))),
                           ("file", FAKE_FILE)):
            (bindir / name).write_text(body)
            (bindir / name).chmod(0o755)
        app = self.make_app()
        if arrange:
            arrange(app)
        env = {**os.environ, "PATH": f"{bindir}:{os.environ['PATH']}"}
        out = subprocess.run(["bash", str(script_copy), str(app)], env=env, capture_output=True, text=True, timeout=60)
        lines = calls.read_text().splitlines() if calls.exists() else []
        return out, lines

    def test_local_identity_signs_inside_out_with_leaf_pinned_requirements(self):
        out, calls = self.run_with("ABCDEF0123456789")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        joined = "\n".join(calls)
        # The helper and the outer app each get a leaf-pinned designated requirement.
        self.assertIn('identifier "dev.zerocode.app.computer-use" and certificate leaf = H"ABCDEF0123456789"', joined)
        self.assertIn('identifier "dev.zerocode.app" and certificate leaf = H"ABCDEF0123456789"', joined)
        # The sibling helpers are signed too (no requirement needed).
        signed = sign_calls(calls)
        self.assertTrue(any(c.endswith("/zerocode-mirror") for c in signed), signed)
        self.assertTrue(any(c.endswith("/zerocode-pick") for c in signed), signed)
        # Inside-out: the outer .app is signed after the nested helper.
        helper_at = next(i for i, c in enumerate(calls) if "Computer Use.app" in c and "leaf" in c)
        outer_at = outer_sign_calls(calls)[0]
        self.assertLess(helper_at, outer_at, "nested code signs before the sealing outer app")
        self.assertTrue(any("--verify --strict" in c for c in calls), "the bundle is verified")

    def test_the_bundled_zo_is_signed_before_the_seal_and_every_nested_executable_starts(self):
        out, calls = self.run_with("ABCDEF0123456789")
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        zo_at = next(i for i, c in enumerate(calls) if "--sign" in c and c.endswith("/Resources/bin/zo"))
        self.assertLess(zo_at, outer_sign_calls(calls)[0], "zo signs before the sealing outer app")
        verify_at = next(i for i, c in enumerate(calls) if "--verify --strict" in c)
        gate = [i for i, c in enumerate(calls) if c.startswith("--display")]
        self.assertTrue(gate and min(gate) > verify_at, "the nested gate reads what was signed and verified")
        self.assertIn("every Mach-O signed as Authority=Local ABCDEF0123456789, 9 nested executables started", out.stdout)
        started = self.ran.read_text().splitlines()
        self.assertEqual(sorted(started),
                         sorted(["zo", "zerocode-mirror", "zerocode-pick", "zerocode-computer-use-macos", *CEF_HELPERS]),
                         "every nested executable once, and never the window itself")

    def test_chromium_carries_entitlements_but_never_hardened_runtime(self):
        # Library validation — a hardened-runtime rule — admits only libraries
        # signed by the process's own Team ID, and neither the local identity
        # nor adhoc has one: `--options runtime` made the shell reject its own
        # CEF framework at dlopen and exit before the window (1.3.11, 09-09).
        for fingerprint in ("ABCDEF0123456789", None):
            with self.subTest(identity=fingerprint or "adhoc"):
                out, calls = self.run_with(fingerprint)
                self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
                self.assertTrue(any(c.endswith(CEF_FRAMEWORK) for c in sign_calls(calls)), "the framework is signed")
                for name in CEF_HELPERS:
                    self.assertTrue(any(f"{name}.app" in c and "--entitlements" in c for c in calls), name)
                outer = [calls[i] for i in outer_sign_calls(calls)]
                self.assertTrue(outer and all("--entitlements" in c for c in outer), outer)
                self.assertFalse(any("--options" in c for c in calls), "hardened runtime would turn on library validation")

    def test_no_identity_falls_back_to_adhoc(self):
        out, calls = self.run_with(None)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertTrue(all("leaf" not in c for c in calls), "adhoc fallback carries no leaf requirement")
        self.assertTrue(any("--sign -" in c for c in calls), calls)
        self.assertIn("every Mach-O signed as Signature=adhoc", out.stdout)

    def test_a_bundled_zo_that_does_not_say_the_apps_version_fails_the_gate(self):
        out, _ = self.run_with("ABCDEF0123456789", lambda app: self.zo("zo 0.0.1"))
        self.assertNotEqual(out.returncode, 0, out.stdout)
        self.assertIn(f'want 0 and "zo {VERSION}"', out.stderr)
        self.assertIn("/Resources/bin/zo", out.stderr)

    def test_a_mach_o_the_signer_does_not_know_fails_the_gate(self):
        # A new resource executable nobody taught the signer: it keeps the
        # linker's adhoc signature, which --verify --strict never reads.
        stray = lambda app: self.executable(app / "Contents" / "Resources" / "bin" / "stray-tool")
        out, _ = self.run_with("ABCDEF0123456789", stray)
        self.assertNotEqual(out.returncode, 0, out.stdout)
        self.assertIn("not signed like the app (Authority=Local ABCDEF0123456789): "
                      "Resources/bin/stray-tool says Signature=adhoc", out.stderr)

    def test_a_nested_executable_with_no_probe_fails_the_gate(self):
        # The signer signs every Mach-O under the Chromium framework, so a CEF
        # that ships a new executable there is signed — and must bring a probe.
        crashpad = lambda app: self.executable(
            app / "Contents" / "Frameworks" / CEF_FRAMEWORK / "Helpers" / "chrome_crashpad_handler")
        out, _ = self.run_with("ABCDEF0123456789", crashpad)
        self.assertNotEqual(out.returncode, 0, out.stdout)
        self.assertIn(f"no launch probe: Frameworks/{CEF_FRAMEWORK}/Helpers/chrome_crashpad_handler", out.stderr)


if __name__ == "__main__":
    unittest.main()

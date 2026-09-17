import os
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
SIGNING = ROOT / 'tools/signing'
FINGERPRINT = 'A' * 40

class Signing(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.log = self.root / 'calls'
        self.env = dict(os.environ, PATH=f'{self.root}:{os.environ["PATH"]}',
                        SIGNING_TEST_LOG=str(self.log), ZEROCODE_SIGNING_KEYCHAIN=str(self.root / 'login.keychain-db'))
        self.env.pop('APPLE_SIGNING_IDENTITY', None)

    def stub(self, name, body):
        path = self.root / name
        path.write_text('#!/bin/bash\nprintf "%s\\n" "$*" >> "$SIGNING_TEST_LOG"\n' + body)
        path.chmod(0o755)

    def test_existing_identity_is_reused_without_import_or_new_key(self):
        self.stub('security', f'echo \'1) {FINGERPRINT} "ZeroCode Local Signing"\'\n')
        result = subprocess.run(['bash', str(SIGNING / 'ensure-local-identity.sh')], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), FINGERPRINT)
        self.assertNotIn('import', self.log.read_text())

    def test_unusable_existing_certificate_never_rotates_the_identity(self):
        self.stub('security', 'case "$1" in find-identity) exit 0;; find-certificate) exit 0;; *) exit 9;; esac\n')
        result = subprocess.run(['bash', str(SIGNING / 'ensure-local-identity.sh')], env=self.env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('import', self.log.read_text())
        self.assertIn('instead of replacing', result.stderr)

    def test_signer_pins_certificate_and_falls_back_only_when_identity_is_unavailable(self):
        import plistlib
        app = self.root / 'helper.app'
        (app / 'Contents').mkdir(parents=True)
        (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier': 'test.helper'}))
        self.stub('codesign', 'exit 0\n')
        for available in (True, False):
            with self.subTest(available=available):
                self.log.unlink(missing_ok=True)
                self.stub('security', f'echo \'1) {FINGERPRINT} "ZeroCode Local Signing"\'\n' if available else 'exit 1\n')
                result = subprocess.run(['bash', str(SIGNING / 'sign-computer-use.sh'), str(app)], env=self.env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = self.log.read_text()
                if available:
                    self.assertIn(f'certificate leaf = H"{FINGERPRINT}"', calls)
                    self.assertNotIn('--deep', calls)
                else:
                    self.assertIn('--sign -', calls)
                    self.assertIn('identity=adhoc', result.stderr)

    def test_stage_zo_signs_under_a_developer_id_before_tauri_seals_the_app(self):
        # tauri copies bundle.resources as data and signs only the code it
        # places itself, so under a Developer ID the staged zo is signed with
        # hardened runtime and a secure timestamp before `tauri build`, and
        # the signed bytes must still say the version before they are staged.
        root = self.root / 'repo'
        (root / 'scripts').mkdir(parents=True)
        (root / 'scripts/stage-zo.mjs').write_text((ROOT / 'scripts/stage-zo.mjs').read_text())
        (root / 'scripts/native-signing.mjs').write_text((ROOT / 'scripts/native-signing.mjs').read_text())
        (root / 'tools/signing').mkdir(parents=True)
        (root / 'tools/signing/sign-developer-id.sh').write_text((SIGNING / 'sign-developer-id.sh').read_text())
        (root / 'Cargo.toml').write_text('[workspace.package]\nversion = "9.9.9"\n')
        built = self.root / 'built-zo'
        built.write_text('#!/bin/sh\nprintf "%s\\n" "zo-ran $*" >> "$SIGNING_TEST_LOG"\necho "zo 9.9.9"\n')
        built.chmod(0o755)
        staged = root.resolve() / 'crates/zerocode-shell/bin/zo'  # stage-zo names its own real path
        identity = 'Developer ID Application: Test (TEAM123)'
        env = {k: v for k, v in self.env.items() if k != 'APPLE_SIGNING_IDENTITY'}
        env['ZO_STAGE_BIN'] = str(built)
        stage = ['node', str(root / 'scripts/stage-zo.mjs')]
        self.stub('codesign', 'exit 0\n')
        for signing in (identity, None):
            with self.subTest(identity=signing):
                self.log.unlink(missing_ok=True)
                result = subprocess.run(stage, env=dict(env, APPLE_SIGNING_IDENTITY=signing) if signing else env,
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = self.log.read_text().splitlines()
                if signing:
                    self.assertEqual(calls, ['zo-ran --version',
                                             f'--force --options runtime --timestamp --identifier zo --sign {identity} {staged}.new',
                                             f'--verify --strict {staged}.new',
                                             'zo-ran --version'])
                else:
                    self.assertEqual(calls, ['zo-ran --version'], 'no identity, no codesign')
                self.assertTrue(staged.is_file())
                self.assertFalse(Path(f'{staged}.new').exists())
        # A zo the identity would not sign is never staged.
        staged.unlink()
        self.stub('codesign', 'exit 1\n')
        result = subprocess.run(stage, env=dict(env, APPLE_SIGNING_IDENTITY=identity), capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('codesign of staged code failed', result.stderr)
        self.assertFalse(staged.exists())

    def test_computer_helper_uses_distribution_identity_without_local_fallback(self):
        import plistlib
        app = self.root / 'helper.app'
        (app / 'Contents').mkdir(parents=True)
        (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier': 'test.helper'}))
        identity = 'Developer ID Application: Test (TEAM123)'
        env = dict(self.env, APPLE_SIGNING_IDENTITY=identity)
        self.stub('security', 'exit 9\n')
        self.stub('codesign', 'exit 0\n')
        command = ['bash', str(SIGNING / 'sign-computer-use.sh'), str(app)]
        result = subprocess.run(command, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.log.read_text().splitlines(), [
            f'--force --options runtime --timestamp --identifier test.helper --sign {identity} {app}',
            f'--verify --strict {app}',
        ])
        self.log.unlink()
        self.stub('codesign', 'exit 7\n')
        result = subprocess.run(command, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('find-identity', self.log.read_text())
        self.assertNotIn('--sign - ', self.log.read_text())

    def test_chromium_helpers_are_signed_before_publish_and_a_refusal_keeps_the_old_stage(self):
        cef = self.root / 'cef'
        framework = cef / 'Chromium Embedded Framework.framework'
        (cef / 'include').mkdir(parents=True)
        (framework / 'Resources').mkdir(parents=True)
        (framework / 'Chromium Embedded Framework').write_text('fixture')
        (framework / 'Resources/Info.plist').write_text('fixture')
        (cef / 'CREDITS.html').write_text('fixture')
        (cef / 'include/cef_version.h').write_text('// Redistribution and use; DISCLAIMERS\n// ---------------------------------------------------------------------------\n#define CEF_VERSION "1.2.3+fixture"\n'.replace('DISCLAIMERS', 'DISCLAIMED'))
        helper = self.root / 'helper'
        helper.write_text('fixture')
        destination = self.root / 'stage'
        module = (ROOT / 'scripts/stage-chromium.mjs').as_uri()
        fixture = dict(cefDir=str(cef), helper=str(helper), destination=str(destination),
                       expectedVersion='1.2.3', identity=dict(identifier='test.app', version='9.9.9'))
        script = f'import {{stageChromium}} from {json.dumps(module)}; await stageChromium({json.dumps(fixture)});'
        env = dict(self.env, APPLE_SIGNING_IDENTITY='Developer ID Application: Test (TEAM123)')
        self.stub('codesign', 'exit 0\n')
        result = subprocess.run(['node', '--input-type=module', '-e', script], env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        signed = [line for line in self.log.read_text().splitlines() if '--sign ' in line]
        bundles = sorted((destination / 'Frameworks').glob('*.app'))
        self.assertEqual(len(signed), len(bundles))
        self.assertTrue(bundles)
        for bundle in bundles:
            self.assertTrue(any(line.endswith('/' + bundle.name) for line in signed))
        self.assertTrue(all('--options runtime --timestamp' in line and '--entitlements ' in line for line in signed))
        marker = (destination / 'stage.json').read_bytes()
        self.stub('codesign', 'exit 7\n')
        failed = subprocess.run(['node', '--input-type=module', '-e', script], env=env, capture_output=True, text=True)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual((destination / 'stage.json').read_bytes(), marker)
        self.assertFalse(list(self.root.glob('.chromium-stage-*')))

    def test_build_and_release_use_the_same_signer(self):
        build = (ROOT / 'crates/zerocode-shell/build.rs').read_text()
        lane = (ROOT / 'tools/release/lane.sh').read_text()
        self.assertIn('tools/signing/sign-computer-use.sh', build)
        # The lane signs the whole app inside-out (sign-app-bundle.sh), which
        # itself signs the helper — one signer for build and release.
        self.assertIn('tools/signing/sign-app-bundle.sh', lane)

if __name__ == '__main__':
    unittest.main()

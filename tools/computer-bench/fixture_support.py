"""Build a fixture into a fresh app identity; preparing never launches it."""
import plistlib
import pathlib
import shutil
import subprocess
import uuid

from fixture_reflex import write_atomic

SUPPORT = pathlib.Path(__file__).with_name('FixtureSupport.swift')


def prepare(folder, executable_name, source, bundle_prefix):
    folder.mkdir(mode=0o700, parents=False, exist_ok=False)
    owner = uuid.uuid4().hex[:12]
    app = folder / f'{executable_name}-{owner}.app'
    executable = app / 'Contents/MacOS' / executable_name
    executable.parent.mkdir(parents=True)
    resources = app / 'Contents/Resources'
    resources.mkdir()
    shutil.copyfile(SUPPORT.with_name('bench.json'), resources / 'bench.json')
    bundle = f'{bundle_prefix}.{owner}'
    with (app / 'Contents/Info.plist').open('wb') as handle:
        plistlib.dump({'CFBundleIdentifier': bundle, 'CFBundleExecutable': executable_name,
                      'CFBundleName': f'{executable_name}-{owner}', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, handle)
    built = subprocess.run(['swiftc', '-O', '-swift-version', '6', '-warnings-as-errors', '-parse-as-library',
                            str(SUPPORT), str(source), '-o', str(executable)])
    write_atomic(folder / 'session.json', {'owner': owner, 'app': str(app), 'executable': str(executable),
                                         'bundle': bundle, 'swiftc': built.returncode})
    return built.returncode

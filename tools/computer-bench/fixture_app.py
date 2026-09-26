"""Build a fixture into a fresh app identity; preparing never launches it."""
import plistlib
import subprocess
import uuid

from fixture_reflex import write_atomic


def prepare(folder, executable_name, source, bundle_prefix, flags=()):
    folder.mkdir(mode=0o700, parents=False, exist_ok=False)
    owner = uuid.uuid4().hex[:12]
    app = folder / f'{executable_name}-{owner}.app'
    executable = app / 'Contents/MacOS' / executable_name
    executable.parent.mkdir(parents=True)
    bundle = f'{bundle_prefix}.{owner}'
    with (app / 'Contents/Info.plist').open('wb') as handle:
        plistlib.dump({'CFBundleIdentifier': bundle, 'CFBundleExecutable': executable_name,
                      'CFBundleName': f'{executable_name}-{owner}', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, handle)
    built = subprocess.run(['swiftc', '-O', '-swift-version', '6', '-warnings-as-errors', *flags,
                            str(source), '-o', str(executable)])
    write_atomic(folder / 'session.json', {'owner': owner, 'app': str(app), 'executable': str(executable),
                                         'bundle': bundle, 'swiftc': built.returncode})
    return built.returncode

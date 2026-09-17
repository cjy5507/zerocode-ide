#!/usr/bin/env python3
"""Turn-end live malloc histogram; deliberately never uses ps/footprint RSS."""
import json
from pathlib import Path
import re
import subprocess
import sys

TABLE = json.loads(Path(__file__).with_name('baseline.json').read_text())


def histogram(text):
    """--showSizes emits exact count/bytes rows; Sizes: has rounded KB labels."""
    classes = {}
    for count, size in re.findall(r'^\s*(\d+)\s+(\d+)\s+\d+(?:\.\d+)?\s+\S', text, re.M):
        count, size = int(count), int(size)
        if count == 0 or size % count:
            raise ValueError('heap row is not a malloc size class')
        width = size // count
        classes[width] = classes.get(width, 0) + count
    total = re.search(r'All zones: (\d+) nodes \((\d+) bytes\)', text)
    if not total or not classes:
        raise ValueError('missing all-zones malloc size histogram')
    nodes, live_bytes = map(int, total.groups())
    if sum(classes.values()) != nodes or sum(size * count for size, count in classes.items()) != live_bytes:
        raise ValueError('malloc size classes disagree with all-zones total')
    return {'classes': [{'size_bytes': size, 'count': count} for size, count in sorted(classes.items())],
            'live_bytes': live_bytes}


def sample(pid, output):
    if sys.platform != 'darwin':
        raise RuntimeError('malloc size-class sampling requires macOS heap; no RSS fallback')
    result = subprocess.run(['/usr/bin/heap', '--showSizes', '--noContent', str(pid)],
                            capture_output=True, text=True, check=True,
                            timeout=TABLE['Limits']['timeout_ms'] / 1000)
    Path(str(output) + '.txt').write_text(result.stdout)
    value = histogram(result.stdout)
    value.update(pid=pid, source='heap --showSizes', host_mem=int(subprocess.check_output(
        ['/usr/sbin/sysctl', '-n', 'hw.memsize'], text=True)))
    Path(output).write_text(json.dumps(value))
    return value


if __name__ == '__main__':
    print(json.dumps(sample(int(sys.argv[1]), sys.argv[2])))

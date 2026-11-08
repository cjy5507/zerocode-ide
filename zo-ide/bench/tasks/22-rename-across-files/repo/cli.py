"""`python3 cli.py 2x1.5 3x2` prints the total of those lines."""

import sys

from billing import calc_total
from report import summary


def main(argv):
    lines = [{"qty": int(qty), "unit": float(unit)} for qty, unit in (pair.split("x") for pair in argv)]
    print(f"total {calc_total(lines):.2f}")
    print("\n".join(summary([{"id": "cli", "lines": lines}])))


if __name__ == "__main__":
    main(sys.argv[1:])

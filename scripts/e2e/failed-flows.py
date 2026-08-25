#!/usr/bin/env python3
"""Print the flow files whose testcases failed in a maestro JUnit report."""

import sys
import xml.etree.ElementTree as ET
from pathlib import Path


def main() -> int:
    report, flow_dir = Path(sys.argv[1]), Path(sys.argv[2])
    if not report.is_file():
        return 0

    failed = {
        case.get("name", "")
        for case in ET.parse(report).getroot().iter("testcase")
        if case.find("failure") is not None or case.find("error") is not None
    }

    by_name = {}
    for flow in sorted(flow_dir.glob("*.yaml")):
        for line in flow.read_text().splitlines():
            if line.startswith("name:"):
                by_name[line.split(":", 1)[1].strip()] = flow
                break

    for name in sorted(failed):
        if name in by_name:
            print(by_name[name])
        else:
            print(f"no flow file declares name {name!r}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

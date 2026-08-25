#!/usr/bin/env python3
"""Print the cargo -p flags for every workspace member a diff can affect.

Reads the same manifests cargo does, so it cannot drift the way a hand-written path
glob does. Anything that can change every crate's build prints --workspace instead.
"""

import json
import subprocess
import sys
from pathlib import Path

WIDE = ("Cargo.lock", "Cargo.toml", "rust-toolchain", ".cargo/", ".github/")

SKIP = frozenset({"bridgething-desktop"})
WIDE_FLAGS = "--workspace " + " ".join(f"--exclude {name}" for name in sorted(SKIP))


def changed_files(base: str) -> list[str]:
    diff = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return [line for line in diff.stdout.splitlines() if line]


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: ci-affected-crates.py <base-ref>", file=sys.stderr)
        return 2

    changed = changed_files(sys.argv[1])
    if any(path.startswith(WIDE) for path in changed):
        print(WIDE_FLAGS)
        return 0

    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    root = Path(meta["workspace_root"])
    members = {}
    for package in meta["packages"]:
        rel = Path(package["manifest_path"]).parent.relative_to(root)
        members[package["name"]] = str(rel)

    # longest prefix wins, so crates/delivery/core beats crates/delivery
    owners = sorted(members.items(), key=lambda kv: len(kv[1]), reverse=True)
    touched = set()
    for path in changed:
        for name, directory in owners:
            if path == directory or path.startswith(f"{directory}/"):
                touched.add(name)
                break

    if not touched:
        return 0

    # every workspace member that depends on a touched one has to build too
    dependents: dict[str, set[str]] = {name: set() for name in members}
    for package in meta["packages"]:
        for dep in package["dependencies"]:
            if dep["name"] in dependents:
                dependents[dep["name"]].add(package["name"])

    affected = set()
    queue = list(touched)
    while queue:
        name = queue.pop()
        if name in affected:
            continue
        affected.add(name)
        queue.extend(dependents.get(name, ()))

    affected -= SKIP
    if not affected:
        return 0

    print(" ".join(f"-p {name}" for name in sorted(affected)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

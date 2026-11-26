#!/usr/bin/env python3
"""Fail if a production source file is over the size limit.

Production files are the source files under ROOTS that are not tests. The limit
is a ratchet: files that were already over it when the gate was added are listed
in `scripts/file-size-baseline.txt` with their current size, and may only shrink.
A file that grows past its baseline, a new file over the limit, and a baseline
entry that is no longer needed all fail. Fix an offender by splitting the module,
never by raising the limit or the baseline. Run from the repository root.
"""

from __future__ import annotations

import sys
from pathlib import Path

LIMIT = 800
ROOTS = ("daemon/src", "app")
SUFFIXES = (".rs", ".qml")
BASELINE = Path("scripts/file-size-baseline.txt")


def is_test(path: Path) -> bool:
    name = path.name
    return (
        name.endswith("_test.rs")
        or name in ("tests.rs", "test.rs")
        or "tests" in path.parts
    )


def line_count(path: Path) -> int:
    with path.open(encoding="utf-8") as source:
        return sum(1 for _ in source)


def production_files() -> list[Path]:
    found: list[Path] = []
    for root in ROOTS:
        base = Path(root)
        if not base.is_dir():
            print(f"[file-size] missing source directory: {root}", file=sys.stderr)
            raise SystemExit(2)
        found.extend(
            path
            for path in base.rglob("*")
            if path.is_file() and path.suffix in SUFFIXES and not is_test(path)
        )
    return sorted(found)


def read_baseline() -> dict[str, int]:
    entries: dict[str, int] = {}
    if BASELINE.is_file():
        for raw in BASELINE.read_text(encoding="utf-8").splitlines():
            line = raw.split("#", 1)[0].strip()
            if line:
                name, size = line.rsplit(None, 1)
                entries[name] = int(size)
    return entries


def main() -> int:
    baseline = read_baseline()
    sizes = {path.as_posix(): line_count(path) for path in production_files()}
    problems: list[str] = []

    for name, lines in sorted(sizes.items(), key=lambda item: -item[1]):
        allowed = baseline.get(name, LIMIT)
        if lines > allowed:
            kind = "grew past its baseline" if name in baseline else "is over the limit"
            problems.append(f"{name}: {lines} lines {kind} ({allowed}); split the module")
    for name, allowed in sorted(baseline.items()):
        lines = sizes.get(name)
        if lines is None:
            problems.append(f"{name}: in the baseline but gone; remove the entry")
        elif lines <= LIMIT:
            problems.append(f"{name}: now {lines} lines, within {LIMIT}; remove the entry")
        elif lines < allowed:
            problems.append(f"{name}: shrank to {lines}; lower its baseline from {allowed}")

    if problems:
        print("\n".join(problems))
        return 1
    print(f"[file-size] production files are within {LIMIT} lines ({len(baseline)} baselined)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

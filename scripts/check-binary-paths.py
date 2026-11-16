"""Reject release binaries containing known build-machine directory prefixes."""
from __future__ import annotations

import argparse
from pathlib import Path


def embedded_roots(data: bytes, roots: list[str]) -> list[str]:
    # Windows paths are case insensitive; rustc and native tools can emit either
    # separator, UTF-8 strings, or UTF-16 resource/debug strings.
    data = data.lower()
    found = []
    for root in roots:
        variants = {root.replace("\\", "/"), root.replace("/", "\\")}
        if any(value.encode(encoding).lower() in data
               for value in variants if value
               for encoding in ("utf-8", "utf-16le")):
            found.append(root)
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--private-root", action="append", required=True)
    parser.add_argument("--files", type=Path, nargs="+", required=True)
    args = parser.parse_args()
    failed = False
    for path in args.files:
        if embedded_roots(path.read_bytes(), args.private_root):
            print(f"FAIL: {path.name} contains a build-machine directory")
            failed = True
    if not failed:
        print(f"Build path check passed: {len(args.files)} binaries")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())

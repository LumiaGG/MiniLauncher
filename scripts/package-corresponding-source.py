"""Package tracked working-tree source plus unmodified vendored Cargo dependencies."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def package(output, vendor):
    # Only Git-tracked inputs, read from the working tree so staged/unstaged
    # changes used by the build are included, without local untracked secrets.
    raw = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT)
    names = [name.decode("utf-8") for name in raw.split(b"\0") if name]
    required = {"LICENSE", "COPYRIGHT", "THIRD_PARTY_NOTICES.html", "SOURCE_BUILD.md",
                "scripts/package-corresponding-source.py", "scripts/collect-third-party-licenses.py"}
    if not required.issubset(names):
        raise RuntimeError("Stage the new release source/license files before packaging")
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    dependencies = [pkg for pkg in lock["package"] if pkg.get("source")]
    for pkg in dependencies:
        if not pkg["source"].startswith("registry+"):
            raise RuntimeError("Non-registry dependency needs explicit source packaging support")
        directory = vendor / f"{pkg['name']}-{pkg['version']}"
        checksum_file = directory / ".cargo-checksum.json"
        data = json.loads(checksum_file.read_text(encoding="utf-8"))
        if data["package"] != pkg["checksum"]:
            raise RuntimeError(f"Vendored package does not match lockfile: {directory.name}")
    output.parent.mkdir(parents=True, exist_ok=True)
    hashes = {}
    prefix = "MiniLauncher-source/"
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for name in sorted(names):
            if name.split("/")[0] in {".git", ".claude", ".codex", ".agents", ".tools", "target", "artifacts", "data", "vendor"}:
                continue
            if name == ".cargo/config.toml":
                configuration = tomllib.loads((ROOT / name).read_text(encoding="utf-8"))
                if configuration != {"source": {"crates-io": {"replace-with": "vendored-sources"},
                                                "vendored-sources": {"directory": "vendor"}}}:
                    raise RuntimeError("Review custom Cargo configuration before source packaging")
                continue
            path = ROOT / name
            if not path.exists():
                continue
            if not path.resolve().is_relative_to(ROOT):
                raise RuntimeError(f"Source path escapes project: {name}")
            content = path.read_bytes()
            hashes[name] = hashlib.sha256(content).hexdigest()
            archive.writestr(prefix + name, content)
        for pkg in dependencies:
            directory = vendor / f"{pkg['name']}-{pkg['version']}"
            expected = json.loads((directory / ".cargo-checksum.json").read_text(encoding="utf-8"))["files"]
            for path in sorted(directory.rglob("*")):
                if not path.is_file():
                    continue
                relative = path.relative_to(directory).as_posix()
                content = path.read_bytes()
                if relative != ".cargo-checksum.json" and (
                    relative not in expected or hashlib.sha256(content).hexdigest() != expected[relative]
                ):
                    raise RuntimeError(f"Modified/unexpected vendored file: {directory.name}/{relative}")
                archive.writestr(prefix + f"vendor/{directory.name}/{relative}", content)
            if any(not (directory / name).is_file() for name in expected):
                raise RuntimeError(f"Incomplete vendored source: {directory.name}")
        archive.writestr(prefix + ".cargo/config.toml", '''[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
''')
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        rust = subprocess.check_output(["rustc", "-vV"], cwd=ROOT, text=True).strip()
        archive.writestr(prefix + "SOURCE_MANIFEST.json", json.dumps(
            {"base_revision": revision, "rustc": rust, "source_sha256": hashes,
             "dependency_packages": len(dependencies), "license": "GPL-3.0-or-later"},
            indent=2, ensure_ascii=False) + "\n")
    print(f"Corresponding source: {output} ({len(hashes)} project files, {len(dependencies)} dependency packages)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--vendor-directory", type=Path, required=True)
    args = parser.parse_args()
    package(args.output.resolve(), args.vendor_directory.resolve())

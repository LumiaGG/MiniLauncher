"""Create offline notices from locked Cargo sources; never fetch during a build.

Missing upstream texts are pinned in legal/supplements with provenance and hashes.
--fetch-missing is a maintainer-only refresh, not part of release packaging.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import html
import json
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
SUPPLEMENTS = ROOT / "legal" / "supplements"
LEGAL_NAME = re.compile(r"^(licen[cs]e|copying|copyright|notice|unlicense)([._-]|$)", re.I)
UPSTREAM_CACHE = {}


def cargo_metadata():
    return json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
         "--filter-platform", "x86_64-pc-windows-msvc"], cwd=ROOT, encoding="utf-8"))


def packages(metadata):
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    seen, pending = set(), list(metadata["workspace_members"])
    while pending:
        ident = pending.pop()
        if ident in seen:
            continue
        seen.add(ident)
        pending.extend(dep["pkg"] for dep in nodes[ident]["deps"])
    return sorted((pkg for pkg in metadata["packages"]
                   if pkg["source"] and pkg["id"] in seen),
                  key=lambda pkg: (pkg["name"], pkg["version"]))


def license_files(directory):
    # Include nested native-code/resource licenses and NOTICE files as well.
    return sorted(path for path in directory.rglob("*") if path.is_file() and (
        LEGAL_NAME.match(path.name) or
        any(part.lower() in ("licenses", "licences") for part in path.relative_to(directory).parts[:-1])))


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "MiniLauncher-license-review"})
    with urllib.request.urlopen(request, timeout=45) as response:
        return response.read()


def supplement_missing(pkg, directory):
    repository_url = pkg.get("repository") or pkg.get("homepage")
    if not repository_url:
        raise RuntimeError(f"No upstream repository for {pkg['name']}")
    repo = re.sub(r"/tree/.*$", "", repository_url).removesuffix(".git").rstrip("/")
    vcs_path = directory / ".cargo_vcs_info.json"
    if vcs_path.is_file():
        sha = json.loads(vcs_path.read_text(encoding="utf-8"))["git"]["sha1"]
    else:
        version = pkg["version"]
        tags = [f"refs/tags/{tag}{suffix}" for tag in (version, f"v{version}", f"{pkg['name']}-{version}")
                for suffix in ("", "^{}")]
        refs = subprocess.check_output(["git", "ls-remote", repo, *tags], text=True).splitlines()
        if not refs:
            raise RuntimeError(f"No pinned release commit for {pkg['name']} {version}; manual supplement required")
        refs.sort(key=lambda line: not line.endswith("^{}"))
        sha = refs[0].split()[0]
    package_commit = sha
    # Reviewed upstream fixes supplying the MIT text already declared by these
    # old releases. Keep the package commit distinct from the supplemental one.
    reviewed_fixes = {
        ("simd_helpers", "0.1.0"): "82040194cd05affb060bf94d6f19f82a771d07fb",
        ("seahash", "4.1.0"): "3088c5c912b70b586d27bf553fbe964e025a2c89",
    }
    sha = reviewed_fixes.get((pkg["name"], pkg["version"]), sha)
    gitlab = repo.startswith("https://gitlab.redox-os.org/")
    if not repo.startswith("https://github.com/") and not gitlab:
        raise RuntimeError(f"Manual license review required for {pkg['name']}: {repo}")
    repository = repo.removeprefix("https://github.com/")
    cache_key = (repository, sha)
    if cache_key not in UPSTREAM_CACHE:
        names = ["LICENSE", "LICENSE.txt", "LICENSE.md", "LICENSE-MIT", "LICENSE-APACHE",
                 "LICENSE-MIT.txt", "LICENSE-APACHE.txt", "COPYING", "COPYING.md", "COPYING.txt",
                 "NOTICE", "NOTICE.txt", "COPYRIGHT", "UNLICENSE", "license", "license.txt",
                 "LICENSE-ZLIB", "LICENSE.ZLIB", "LICENSE-MIT.md", "LICENSE-APACHE.md"]
        def download(name):
            url = f"{repo}/-/raw/{sha}/{name}" if gitlab else f"https://raw.githubusercontent.com/{repository}/{sha}/{name}"
            try:
                return name, url, fetch(url)
            except urllib.error.HTTPError as error:
                if error.code == 404 or (error.code == 403 and not gitlab):
                    return None
                raise
        with ThreadPoolExecutor(max_workers=8) as executor:
            UPSTREAM_CACHE[cache_key] = [item for item in executor.map(download, names) if item]
    records = []
    target = SUPPLEMENTS / f"{pkg['name']}-{pkg['version']}"
    target.mkdir(parents=True, exist_ok=True)
    for name, url, data in UPSTREAM_CACHE[cache_key]:
        (target / name).write_bytes(data)
        records.append({"file": name, "url": url, "sha256": hashlib.sha256(data).hexdigest()})
    if not records and pkg["name"] == "mac" and pkg["version"] == "0.1.1":
        # This release grants MIT/Apache-2.0 in Cargo.toml and README but never
        # shipped a license file. Preserve the grant and supply the canonical
        # Apache text, selecting its explicitly offered Apache alternative.
        for name, url in [
            ("UPSTREAM-README.md", f"https://raw.githubusercontent.com/{repository}/{sha}/README.md"),
            ("LICENSE-APACHE", "https://www.apache.org/licenses/LICENSE-2.0.txt"),
        ]:
            data = fetch(url)
            (target / name).write_bytes(data)
            records.append({"file": name, "url": url, "sha256": hashlib.sha256(data).hexdigest()})
    if not records:
        raise RuntimeError(f"No upstream license text at {repo}, commit {sha}")
    (target / "provenance.json").write_text(json.dumps(
        {"package": pkg["name"], "version": pkg["version"], "package_commit": package_commit,
         "commit": sha, "files": records},
        indent=2) + "\n", encoding="utf-8")


def generate(output, fetch_missing=False):
    entries, failures = [], []
    for pkg in packages(cargo_metadata()):
        directory = Path(pkg["manifest_path"]).parent
        texts = license_files(directory)
        # A nested dependency's license does not substitute for the crate license.
        has_primary = any(path.parent == directory for path in texts)
        license_file = pkg.get("license_file")
        if license_file and (directory / license_file).is_file():
            texts = sorted(set(texts + [directory / license_file]))
            has_primary = True
        supplement = SUPPLEMENTS / f"{pkg['name']}-{pkg['version']}"
        if not has_primary and not (supplement / "provenance.json").is_file() and fetch_missing:
            print(f"Fetching pinned license: {pkg['name']} {pkg['version']}", flush=True)
            try:
                supplement_missing(pkg, directory)
            except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
                failures.append(str(error))
        if (supplement / "provenance.json").is_file():
            provenance = json.loads((supplement / "provenance.json").read_text(encoding="utf-8"))
            for record in provenance["files"]:
                path = supplement / record["file"]
                if hashlib.sha256(path.read_bytes()).hexdigest() != record["sha256"]:
                    raise RuntimeError(f"Supplement checksum mismatch: {path}")
                texts.append(path)
            has_primary = True
        if not has_primary or not pkg.get("license"):
            failures.append(f"{pkg['name']} {pkg['version']}: missing primary license text or declaration")
            continue
        blocks = []
        for path in texts:
            label = path.relative_to(supplement) if path.is_relative_to(supplement) else path.relative_to(directory)
            content = path.read_text(encoding="utf-8-sig", errors="replace")
            blocks.append(f"<h3>{html.escape(str(label))}</h3><pre>{html.escape(content)}</pre>")
        name = f"{pkg['name']} {pkg['version']}"
        source = f"https://crates.io/crates/{pkg['name']}/{pkg['version']}"
        authors = ", ".join(pkg.get("authors", []))
        entries.append(f'<details><summary>{html.escape(name)} — {html.escape(pkg["license"])}</summary>'
                       f'<p>Source: <a href="{html.escape(source)}">{html.escape(source)}</a></p>'
                       f'<p>Package authors: {html.escape(authors)}</p>'
                       + "\n".join(blocks) + "</details>")
    if failures:
        raise RuntimeError("License collection incomplete:\n" + "\n".join(failures))
    # Data license omitted from the pinyin crate archive; pinned separately.
    data_directory = SUPPLEMENTS / "pinyin-data-0.13.0"
    if fetch_missing and not (data_directory / "provenance.json").is_file():
        pinyin = next(pkg for pkg in packages(cargo_metadata()) if pkg["name"] == "pinyin")
        data_path = Path(pinyin["manifest_path"]).parent / "pinyin-data" / "pinyin.txt"
        if not data_path.read_text(encoding="utf-8").startswith("# version: 0.13.0\n"):
            raise RuntimeError("pinyin-data version changed; review its license supplement")
        supplement_missing({"name": "pinyin-data", "version": "0.13.0",
                            "repository": "https://github.com/mozillazg/pinyin-data"}, data_path.parent)
        provenance = json.loads((data_directory / "provenance.json").read_text(encoding="utf-8"))
        remote = fetch(f"https://raw.githubusercontent.com/mozillazg/pinyin-data/{provenance['commit']}/pinyin.txt")
        if remote != data_path.read_bytes():
            raise RuntimeError("pinyin-data differs from the upstream release; review data provenance")
    if not data_directory.is_dir():
        raise RuntimeError("Missing pinned pinyin-data license supplement")
    provenance = json.loads((data_directory / "provenance.json").read_text(encoding="utf-8"))
    blocks = []
    for record in provenance["files"]:
        path = data_directory / record["file"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != record["sha256"]:
            raise RuntimeError(f"Supplement checksum mismatch: {path}")
        blocks.append("<pre>" + html.escape(path.read_text(encoding="utf-8")) + "</pre>")
    entries.append('<details><summary>pinyin-data — MIT</summary>' + "".join(blocks) + '</details>')
    document = """<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>MiniLauncher — Third-party licenses</title>
<style>body{font:16px system-ui;max-width:1000px;margin:32px auto;padding:0 20px}
summary{cursor:pointer;padding:12px}details{border:1px solid #aaa;margin:12px 0;padding:8px}
pre{white-space:pre-wrap;overflow-wrap:anywhere;font:13px monospace}a{overflow-wrap:anywhere}</style>
<h1>MiniLauncher — Third-party licenses</h1>
<p>MiniLauncher original code: GPL-3.0-or-later. See LICENSE and COPYRIGHT.</p>
<p>Windows dependency inventory, including build and development dependencies.
Third-party components retain their original licenses and notices below.</p>
<p>Bundled GPUI Kit icons: Lucide ISC and Feather MIT; see gpui-kit-assets / LICENSE-LUCIDE below.
System fonts are supplied by Windows and are not redistributed.</p>
""" + "\n".join(entries) + "\n</html>\n"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(document, encoding="utf-8", newline="\n")
    print(f"Collected {len(entries)} component/data entries: {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "THIRD_PARTY_NOTICES.html")
    parser.add_argument("--fetch-missing", action="store_true")
    arguments = parser.parse_args()
    generate(arguments.output, arguments.fetch_missing)

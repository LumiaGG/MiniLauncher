"""Build and publish a GitHub release, resuming incomplete draft uploads."""
from __future__ import annotations

import argparse
import json
import mimetypes
import os
from pathlib import Path
import re
import subprocess
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
API = "https://api.github.com"


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def ensure_clean() -> None:
    # Untracked local tools are never source-package inputs.
    for args in (("diff", "--quiet"), ("diff", "--cached", "--quiet")):
        if subprocess.run(["git", *args], cwd=ROOT).returncode:
            raise RuntimeError("Commit tracked changes before publishing")


def repository_from_origin() -> str:
    origin = git("remote", "get-url", "origin")
    match = re.fullmatch(r"(?:https://github\.com/|git@github\.com:)([^/]+/[^/]+?)(?:\.git)?/?", origin)
    if not match:
        raise RuntimeError("Cannot infer GitHub repository; pass --repository owner/repo")
    return match[1]


def credentials() -> str:
    for name in ("GH_TOKEN", "GITHUB_TOKEN"):
        if os.environ.get(name):
            return os.environ[name]
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never")
    result = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n",
                            text=True, capture_output=True, env=env, cwd=ROOT)
    values = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
    if result.returncode or not values.get("password"):
        raise RuntimeError("Sign in through Git credential manager or set GH_TOKEN / GITHUB_TOKEN")
    return values["password"]


def request(path: str, token: str, method="GET", data=None, content_type="application/json"):
    url = path if path.startswith("https://") else API + path
    if isinstance(data, dict):
        data = json.dumps(data, ensure_ascii=False).encode()
    headers = {"Authorization": f"Bearer {token}", "User-Agent": "MiniLauncher-release",
               "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
    if data is not None:
        headers["Content-Type"] = content_type
    with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers, method=method), timeout=300) as response:
        return None if response.status == 204 else json.load(response)


def release_assets(output: Path, revision: str) -> list[Path]:
    # Use the build's explicit inventory, never glob a directory with old files.
    # SHA256SUMS is a local inventory; publishing does not recompute hashes.
    names = []
    for line in (output / "SHA256SUMS.txt").read_text(encoding="utf-8-sig").splitlines():
        _, name = line.split("  ", 1)
        if not name or "/" in name or "\\" in name or Path(name).suffix not in (".zip", ".exe"):
            raise RuntimeError("Invalid release inventory filename")
        names.append(name)
    if len(names) != len(set(names)):
        raise RuntimeError("Duplicate release inventory entry")
    sources = [name for name in names if name.endswith("-Source.zip")]
    if len(sources) != 1 or not {"MiniLauncher-Setup-windows-x64.exe", "MiniLauncher-Portable-windows-x64.zip"}.issubset(names):
        raise RuntimeError("Missing installer, portable archive or corresponding source")
    with zipfile.ZipFile(output / sources[0]) as archive:
        manifest = json.loads(archive.read("MiniLauncher-source/SOURCE_MANIFEST.json"))
        if manifest["base_revision"] != revision:
            raise RuntimeError("Source package does not match HEAD; rebuild before publishing")
    assets = [output / name for name in names]
    if any(not path.is_file() or not path.stat().st_size for path in assets):
        raise RuntimeError("Missing or empty release asset")
    return assets


def publish(repository: str, tag: str, revision: str, notes: str, assets: list[Path], token: str, *, latest=True) -> str:
    endpoint = f"/repos/{repository}/releases"
    # Lookup by tag also includes drafts visible to the authenticated maintainer.
    existing = None
    for page in range(1, 101):
        releases = request(f"{endpoint}?per_page=100&page={page}", token)
        existing = next((release for release in releases if release["tag_name"] == tag), None)
        if existing or len(releases) < 100:
            break
    if existing and (not existing["draft"] or existing["target_commitish"] != revision):
        raise RuntimeError(f"Tag {tag} already belongs to a published release or another revision; choose a new version")
    release = existing or request(endpoint, token, "POST", {
        "tag_name": tag, "target_commitish": revision,
        "name": f"MiniLauncher {tag}" if latest else f"MiniLauncher corresponding source {tag}",
        "body": notes, "draft": True, "prerelease": False})
    print(f"Draft: {release['html_url']}", flush=True)
    uploaded = {asset["name"]: asset for asset in release["assets"]}
    if set(uploaded) - {path.name for path in assets}:
        raise RuntimeError("Draft contains unexpected assets; review it before resuming")
    upload_url = release["upload_url"].split("{")[0]
    for path in assets:
        asset = uploaded.get(path.name)
        # Retry broken/incomplete uploads, retaining already uploaded files.
        if asset and (asset["state"] != "uploaded" or asset["size"] != path.stat().st_size):
            request(f"/repos/{repository}/releases/assets/{asset['id']}", token, "DELETE")
            asset = None
        if not asset:
            print(f"Uploading {path.name} ({path.stat().st_size} bytes)", flush=True)
            asset = request(upload_url + "?name=" + urllib.parse.quote(path.name), token, "POST",
                            path.read_bytes(), mimetypes.guess_type(path.name)[0] or "application/octet-stream")
        if asset["state"] != "uploaded" or asset["size"] != path.stat().st_size:
            raise RuntimeError(f"Incomplete upload: {path.name}")
    release = request(f"{endpoint}/{release['id']}", token, "PATCH", {
        "draft": False, "make_latest": "true" if latest else "false", "body": notes})
    return release["html_url"]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", help="Defaults to v<version> from Cargo.toml")
    parser.add_argument("--repository", help="Defaults to the GitHub origin repository")
    parser.add_argument("--notes-file", type=Path, help="UTF-8 Markdown release notes")
    parser.add_argument("--skip-build", action="store_true", help="Use this revision's existing release directory")
    args = parser.parse_args()
    ensure_clean()
    revision, short = git("rev-parse", "HEAD"), git("rev-parse", "--short", "HEAD")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    tag = args.tag or f"v{version}"
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", tag):
        raise RuntimeError("Tag must be a simple version identifier")
    repository = args.repository or repository_from_origin()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise RuntimeError("Repository must be owner/repo")
    notes = args.notes_file.read_text(encoding="utf-8-sig") if args.notes_file else (
        f"MiniLauncher {tag} · Windows 10/11 x64\n\n"
        "- Setup：宿主、设置程序及自带插件。Portable：宿主与设置程序，插件 ZIP 可按需安装。\n"
        "- 插件 ZIP：按需下载并通过设置程序安装。\n\n"
        f"源码提交：`{revision}`。GPL-3.0-or-later；详见随附许可和第三方声明。\n")
    branch = git("branch", "--show-current")
    remote = git("ls-remote", f"https://github.com/{repository}.git", f"refs/heads/{branch}")
    if not branch or not remote or remote.split()[0] != revision:
        raise RuntimeError("Push the current branch to the release repository before publishing")
    token = credentials()
    repo = request(f"/repos/{repository}", token)
    if repo["private"]:
        raise RuntimeError("Expected a public release repository")
    try:
        current_release = request(f"/repos/{repository}/releases/tags/{tag}", token)
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        current_release = None
    if current_release and not current_release["draft"]:
        raise RuntimeError(f"Release {tag} is already published; choose a new version before building")
    output = ROOT / "target" / "packages" / f"{tag}-{short}"
    source_tag = f"source-{tag}"
    source_url = f"https://github.com/{repository}/releases/tag/{source_tag}"
    if not args.skip_build:
        subprocess.run(["pwsh", "-NoProfile", "-File", str(ROOT / "scripts/build-release.ps1"),
                        "-OutputDirectory", str(output), "-SourceDownloadUrl", source_url], cwd=ROOT, check=True)
    ensure_clean()
    if git("rev-parse", "HEAD") != revision:
        raise RuntimeError("HEAD changed during build; restart publishing")
    assets = release_assets(output, revision)
    sources = [path for path in assets if path.name.endswith("-Source.zip")]
    binaries = [path for path in assets if path not in sources]
    # Source downloads live separately so the normal release lists only apps/plugins.
    # A completed source release can be reused after interruption without reuploading it.
    try:
        existing_source = request(f"/repos/{repository}/releases/tags/{source_tag}", token)
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        existing_source = None
    if existing_source and not existing_source["draft"]:
        remote_assets = existing_source["assets"]
        if (existing_source["target_commitish"] != revision or len(remote_assets) != 1
                or remote_assets[0]["name"] != sources[0].name
                or remote_assets[0]["state"] != "uploaded"
                or remote_assets[0]["size"] != sources[0].stat().st_size):
            raise RuntimeError("Published source release does not match the local source package")
    else:
        publish(repository, source_tag, revision,
                f"对应 {tag} 的完整源码、锁定依赖及离线构建说明。源码提交：`{revision}`。",
                sources, token, latest=False)
    notes += f"\n\n对应源码、锁定依赖及离线构建说明：[独立源码下载页]({source_url})。\n"
    print("Published: " + publish(repository, tag, revision, notes, binaries, token), flush=True)


if __name__ == "__main__":
    try:
        main()
    except urllib.error.HTTPError as error:
        raise SystemExit(f"GitHub API HTTP {error.code}: {error.reason}; incomplete uploads remain in the draft")
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error))

"""Targeted checks for offline notices and release asset selection."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


notices = load("notices", "collect-third-party-licenses.py")
publisher = load("publisher", "publish-release.py")


class LicensePackagingTests(unittest.TestCase):
    def setUp(self):
        fixtures = ROOT / "artifacts" / "license-packaging-tests"
        fixtures.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=fixtures)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def test_notices_preserve_nested_resources_and_reject_missing_primary_license(self):
        package = self.root / "example"
        package.mkdir()
        (package / "LICENSE").write_text("Permission & copyright <owner>", encoding="utf-8")
        (package / "native").mkdir()
        (package / "native" / "NOTICE").write_text("Native copyright", encoding="utf-8")
        supplements = self.root / "supplements"
        data = supplements / "pinyin-data-0.13.0"
        data.mkdir(parents=True)
        content = b"Data permission"
        (data / "LICENSE").write_bytes(content)
        (data / "provenance.json").write_text(json.dumps({"files": [{
            "file": "LICENSE", "sha256": hashlib.sha256(content).hexdigest()}]}), encoding="utf-8")
        pkg = {"id": "example", "name": "example", "version": "1.0.0", "source": "registry+test",
               "manifest_path": str(package / "Cargo.toml"), "license": "MIT", "authors": ["Author"]}
        metadata = {"packages": [pkg], "workspace_members": ["example"],
                    "resolve": {"nodes": [{"id": "example", "deps": []}]}}
        with patch.object(notices, "cargo_metadata", return_value=metadata), patch.object(notices, "SUPPLEMENTS", supplements):
            output = self.root / "notices.html"
            notices.generate(output)
            text = output.read_text(encoding="utf-8")
            self.assertIn("Native copyright", text)
            self.assertIn("&lt;owner&gt;", text)
            self.assertIn("Package authors: Author", text)
            (package / "LICENSE").unlink()
            with self.assertRaisesRegex(RuntimeError, "missing primary license"):
                notices.generate(self.root / "invalid.html")
            self.assertFalse((self.root / "invalid.html").exists())
            (package / "LICENSE").write_text("MIT", encoding="utf-8")
            (data / "LICENSE").write_text("tampered", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                notices.generate(self.root / "tampered.html")

    def test_release_inventory_excludes_unlisted_files_and_rejects_stale_build(self):
        names = ["MiniLauncher-Setup-windows-x64.exe", "MiniLauncher-Portable-windows-x64.zip",
                 "example-windows-x64.zip"]
        for name in names + ["old-Source.zip", "private-secret.txt"]:
            (self.root / name).write_bytes(b"fixture")
        manifest = self.root / "release-manifest.json"
        manifest.write_text(json.dumps({"revision": "current", "assets": names}), encoding="utf-8")
        self.assertEqual([path.name for path in publisher.release_assets(self.root, "current")], names)
        with self.assertRaisesRegex(RuntimeError, "does not match HEAD"):
            publisher.release_assets(self.root, "stale")
        manifest.write_text(json.dumps({"revision": "current", "assets": names + ["old-Source.zip"]}), encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "Invalid release inventory"):
            publisher.release_assets(self.root, "current")


if __name__ == "__main__":
    unittest.main()

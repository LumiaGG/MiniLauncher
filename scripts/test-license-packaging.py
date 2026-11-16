"""Targeted checks for offline notices and corresponding-source release guards."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile


ROOT = Path(__file__).resolve().parent.parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


notices = load("notices", "collect-third-party-licenses.py")
source = load("source", "package-corresponding-source.py")


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

    def test_source_includes_working_tree_changes_excludes_untracked_data_and_checks_vendor(self):
        project = self.root / "project"
        project.mkdir()
        subprocess.run(["git", "init", "-q", str(project)], check=True)
        for name in ("LICENSE", "COPYRIGHT", "THIRD_PARTY_NOTICES.html", "SOURCE_BUILD.md",
                     "scripts/package-corresponding-source.py", "scripts/collect-third-party-licenses.py", "src/lib.rs"):
            path = project / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("tracked source", encoding="utf-8")
        vendor = self.root / "vendor" / "example-1.0.0"
        vendor.mkdir(parents=True)
        data = b"original dependency"
        (vendor / "lib.rs").write_bytes(data)
        checksum = "a" * 64
        (vendor / ".cargo-checksum.json").write_text(json.dumps({
            "package": checksum, "files": {"lib.rs": hashlib.sha256(data).hexdigest()}}), encoding="utf-8")
        (project / "Cargo.lock").write_text(f'''version = 4
[[package]]
name = "example"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "{checksum}"
''', encoding="utf-8")
        subprocess.run(["git", "-C", str(project), "add", "."], check=True, capture_output=True)
        subprocess.run(["git", "-C", str(project), "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                        "commit", "-qm", "Fixture"], check=True, capture_output=True)
        (project / "src/lib.rs").write_text("modified build input", encoding="utf-8")
        (project / "private-secret.txt").write_text("do not distribute", encoding="utf-8")
        output = self.root / "source.zip"
        with patch.object(source, "ROOT", project):
            source.package(output, vendor.parent)
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(archive.read("MiniLauncher-source/src/lib.rs"), b"modified build input")
                self.assertEqual(archive.read("MiniLauncher-source/vendor/example-1.0.0/lib.rs"), data)
                self.assertFalse(any("private-secret" in name for name in archive.namelist()))
                self.assertIn(b'directory = "vendor"', archive.read("MiniLauncher-source/.cargo/config.toml"))
            (vendor / "lib.rs").write_text("modified dependency", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "Modified/unexpected vendored file"):
                source.package(self.root / "invalid-source.zip", vendor.parent)


if __name__ == "__main__":
    unittest.main()

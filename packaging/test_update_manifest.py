"""Release signing and artifact selection use temporary files and test-only keys."""

import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import update_manifest


class UpdateManifestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="commander-update-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.key = self.root / "private.pem"
        subprocess.run(["openssl", "genpkey", "-algorithm", "Ed25519", "-out", str(self.key)],
                       capture_output=True, check=True)
        self.public = update_manifest.public_key(self.key)
        for architecture in ("x86_64", "aarch64"):
            assets = []
            for kind, suffix in update_manifest.SUFFIXES.items():
                name = f"commander-1.0.0-1-{architecture}{suffix}"
                payload = f"Package fixture: {name}".encode()
                (self.assets / name).write_bytes(payload)
                assets.append({"name": name, "architecture": architecture, "format": kind,
                               "size": len(payload), "sha256": hashlib.sha256(payload).hexdigest(),
                               "glibc_minimum": "2.35"})
            (self.assets / f"release-{architecture}.json").write_text(json.dumps(
                {"version": "1.0.0", "architecture": architecture, "assets": assets}))

    def create(self, public=None):
        update_manifest.create(self.assets, "1.0.0", "a" * 40, self.key.read_text(), public or self.public)

    def test_signed_manifest_covers_all_packages_and_verifies_with_public_key(self):
        self.create()
        manifest = json.loads((self.assets / "update.json").read_text())
        self.assertEqual(manifest["repository"], "aheinze/Commander")
        self.assertEqual(manifest["tag"], "v1.0.0")
        self.assertEqual(len(manifest["assets"]), 8)
        self.assertEqual((self.assets / "update.json.sig").stat().st_size, 64)
        public = self.root / "public.pem"
        public.write_bytes(update_manifest.openssl("pkey", "-in", self.key, "-pubout"))
        result = subprocess.run(["openssl", "pkeyutl", "-verify", "-rawin", "-pubin", "-inkey", str(public),
                                 "-in", str(self.assets / "update.json"), "-sigfile", str(self.assets / "update.json.sig")],
                                capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        for line in (self.assets / "SHA256SUMS").read_text().splitlines():
            expected, name = line.split("  ", 1)
            self.assertEqual(expected, hashlib.sha256((self.assets / name).read_bytes()).hexdigest())

    def test_wrong_signing_key_cannot_publish_a_manifest(self):
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.create("00" * 32)
        self.assertFalse((self.assets / "update.json").exists())

    def test_modified_or_missing_package_prevents_signing(self):
        package = next(self.assets.glob("*.deb"))
        package.write_bytes(b"changed package")
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.create()
        package.unlink()
        with self.assertRaisesRegex(ValueError, "regular file"):
            self.create()

    def test_malformed_duplicate_or_incomplete_inventory_is_rejected(self):
        path = self.assets / "release-x86_64.json"
        original = json.loads(path.read_text())
        for mutation in ("traversal", "duplicate", "missing", "wrong_arch", "wrong_version"):
            data = json.loads(json.dumps(original))
            if mutation == "traversal":
                data["assets"][0]["name"] = "../package.deb"
            elif mutation == "duplicate":
                data["assets"].append(data["assets"][0])
            elif mutation == "missing":
                data["assets"].pop()
            elif mutation == "wrong_arch":
                data["assets"][0]["architecture"] = "aarch64"
            else:
                data["version"] = "1.1.0"
            path.write_text(json.dumps(data))
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                self.create()
        self.assertFalse((self.assets / "update.json").exists())


if __name__ == "__main__":
    unittest.main()

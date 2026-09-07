"""Regression checks for release orchestration; all outputs live in temporary directories."""

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="commander-release-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.binary = self.root / 'binary with "quotes"'
        self.binary.write_bytes(b"\x7fELF\x02\x01" + b"\0" * 10 + struct.pack("<HH", 3, 62))
        self.output = self.root / "output with spaces"
        self.args = release.arguments(["--formats", "deb,rpm,arch", "--binary", str(self.binary),
                                       "--output", str(self.output), "--maintainer", "Test <test@example.org>"])
        self.enterContext(patch.object(release.platform, "system", return_value="Linux"))
        self.enterContext(patch.object(release.platform, "machine", return_value="x86_64"))
        self.enterContext(contextlib.redirect_stdout(io.StringIO()))

    def package_stub(self, command, **kwargs):
        self.assertEqual(command[0:2], ["nfpm", "package"])
        config = json.loads(Path(command[command.index("--config") + 1]).read_text())
        self.assertEqual(config["arch"], "amd64")
        self.assertIn("libc6 (>= 2.43)", config["overrides"]["deb"]["depends"])
        executable = next(item for item in config["contents"] if item["dst"] == "/usr/bin/commander")
        self.assertEqual(Path(executable["src"]).read_bytes(), self.binary.read_bytes())
        self.assertEqual(executable["file_info"]["mode"], 0o755)
        kind = command[command.index("--packager") + 1]
        suffix = {"deb": ".deb", "rpm": ".rpm", "archlinux": ".pkg.tar.zst"}[kind]
        destination = Path(command[command.index("--target") + 1]) / ("commander" + suffix)
        destination.write_bytes(b"test package payload")

    def test_success_publishes_selected_formats_and_valid_checksums(self):
        with patch.object(release, "prerequisites", return_value=None), \
             patch.object(release, "glibc_requirement", return_value="2.43"), \
             patch.object(release, "run", side_effect=self.package_stub):
            release.create_release(self.args)
        self.assertEqual({p.name for p in self.output.iterdir()},
                         {"commander.deb", "commander.rpm", "commander.pkg.tar.zst", "release.json", "SHA256SUMS"})
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            expected, name = line.split("  ", 1)
            self.assertEqual(expected, hashlib.sha256((self.output / name).read_bytes()).hexdigest())
        self.assertFalse(list(self.root.glob(".commander-release-*")))

    def test_later_packaging_failure_never_publishes_partial_release(self):
        count = 0

        def fail_second(command, **kwargs):
            nonlocal count
            count += 1
            if count == 2:
                raise subprocess.CalledProcessError(1, command)
            self.package_stub(command)

        with patch.object(release, "prerequisites", return_value=None), \
             patch.object(release, "glibc_requirement", return_value="2.43"), \
             patch.object(release, "run", side_effect=fail_second):
            with self.assertRaises(subprocess.CalledProcessError):
                release.create_release(self.args)
        self.assertFalse(self.output.exists())
        self.assertFalse(list(self.root.glob(".commander-release-*")))

    def test_successful_tool_without_artifact_is_an_error(self):
        with patch.object(release, "prerequisites", return_value=None), \
             patch.object(release, "glibc_requirement", return_value="2.43"), \
             patch.object(release, "run"):
            with self.assertRaisesRegex(ValueError, "exactly one"):
                release.create_release(self.args)
        self.assertFalse(self.output.exists())

    def test_existing_release_is_never_modified(self):
        self.output.mkdir()
        marker = self.output / "important"
        marker.write_text("keep this")
        with self.assertRaisesRegex(ValueError, "already exists"):
            release.create_release(self.args)
        self.assertEqual(marker.read_text(), "keep this")

    def test_check_does_not_create_files_or_build(self):
        self.args.check = True
        before = set(self.root.rglob("*"))
        with patch.object(release, "prerequisites", return_value=None), \
             patch.object(release, "glibc_requirement", return_value="2.43"), \
             patch.object(release, "run") as run:
            release.create_release(self.args)
        self.assertEqual(set(self.root.rglob("*")), before)
        run.assert_not_called()

    def test_mismatched_architecture_is_rejected_before_build(self):
        with patch.object(release.platform, "machine", return_value="aarch64"):
            with self.assertRaisesRegex(ValueError, "must match"):
                release.create_release(self.args)

    def test_invalid_binary_is_rejected(self):
        self.binary.write_text("#!/bin/sh\nexit 0\n")
        with self.assertRaisesRegex(ValueError, "ELF"):
            release.create_release(self.args)

    def test_invalid_arguments_are_rejected(self):
        for formats in ("", "deb,", "deb,unknown", "all,rpm"):
            with self.assertRaises(argparse.ArgumentTypeError):
                release.formats_argument(formats)
        for revision in ("0", "-1", "1.5", "../2"):
            with self.assertRaises(argparse.ArgumentTypeError):
                release.positive_integer(revision)
        self.assertEqual(release.formats_argument("deb,AppImage,deb"), ["deb", "appimage"])

    def test_glibc_versions_are_compared_numerically(self):
        result = subprocess.CompletedProcess([], 0, stdout="GLIBC_2.9 GLIBC_2.43 GLIBC_2.3.4")
        with patch.object(release, "run", return_value=result):
            self.assertEqual(release.glibc_requirement(self.binary), "2.43")

    def test_gtk_resources_support_builtin_and_external_pixbuf_loaders(self):
        schemas = self.root / "schemas"
        schemas.mkdir()
        (schemas / "test.gschema.xml").write_text("schema fixture")
        library_dir = self.root / "libraries"
        pixbuf_modules = library_dir / "pixbuf/loaders"

        def variable(module, name, fallback=""):
            return {"schemasdir": schemas, "libdir": library_dir,
                    "giomoduledir": library_dir / "gio/modules",
                    "gdk_pixbuf_moduledir": pixbuf_modules,
                    "gdk_pixbuf_query_loaders": self.root / "query-loaders"}[name]

        def command(command, **kwargs):
            if str(command[0]).endswith("query-loaders"):
                return subprocess.CompletedProcess(command, 0, stdout=f'"{pixbuf_modules}/loader.so"\n')
            self.assertEqual(command[0], "glib-compile-schemas")
            return subprocess.CompletedProcess(command, 0, stdout="")

        for external in (False, True):
            if external:
                pixbuf_modules.mkdir(parents=True)
                (pixbuf_modules / "loader.so").write_bytes(b"module fixture")
            appdir = self.root / str(external)
            with patch.object(release, "pkg_variable", side_effect=variable), \
                 patch.object(release, "run", side_effect=command):
                extra = release.stage_gtk_resources(appdir)
            self.assertTrue((appdir / "usr/share/glib-2.0/schemas/test.gschema.xml").is_file())
            cache = appdir / "usr/lib/pixbuf-loaders.cache"
            if external:
                self.assertEqual(cache.read_text(), '"loader.so"\n')
                self.assertIn(pixbuf_modules / "loader.so", extra)
            else:
                self.assertFalse(cache.exists())

    def test_appimage_only_does_not_require_nfpm_or_a_maintainer(self):
        self.args.formats = ["appimage"]
        self.args.maintainer = ""

        def image_stub(stage, work, destination, *args):
            destination.write_bytes(b"AppImage fixture")
            return "2.44"

        with patch.object(release, "prerequisites", return_value="linuxdeploy"), \
             patch.object(release, "glibc_requirement", return_value="2.43"), \
             patch.object(release, "run") as command, \
             patch.object(release, "build_appimage", side_effect=image_stub):
            release.create_release(self.args)
        command.assert_not_called()
        manifest = json.loads((self.output / "release.json").read_text())
        self.assertEqual(manifest["appimage_glibc_minimum"], "2.44")
        self.assertEqual(len(list(self.output.glob("*.AppImage"))), 1)


if __name__ == "__main__":
    unittest.main()

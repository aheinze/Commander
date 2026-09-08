#!/usr/bin/env python3
"""Build local Commander packages. Requires Python 3.11+; see packaging/README.md."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parent
APP_ID = "org.example.Dualpane"
FORMATS = ("deb", "rpm", "arch", "appimage")
ARCHITECTURES = {
    "x86_64": ("amd64", "x86_64-unknown-linux-gnu", 62),
    "aarch64": ("arm64", "aarch64-unknown-linux-gnu", 183),
}


def run(command, *, capture=False, quiet=False, **kwargs):
    if not quiet:
        print("+ " + shlex.join(str(arg) for arg in command), flush=True)
    return subprocess.run(command, check=True, text=True, capture_output=capture, **kwargs)


def formats_argument(value):
    if value.lower() == "all":
        return list(FORMATS)
    formats = list(dict.fromkeys(part.strip().lower() for part in value.split(",")))
    if not formats or any(item not in FORMATS for item in formats):
        raise argparse.ArgumentTypeError("use all or a comma-separated list of deb,rpm,arch,appimage")
    return formats


def positive_integer(value):
    if not re.fullmatch(r"[1-9][0-9]*", value):
        raise argparse.ArgumentTypeError("must be a positive integer")
    return value


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--formats", type=formats_argument, default=list(FORMATS),
                        help="all (default), or deb,rpm,arch,appimage")
    parser.add_argument("--binary", type=Path, help="package an existing native release binary instead of building")
    parser.add_argument("--output", type=Path, help="new release directory (default: dist/releases/commander-VERSION-REVISION-ARCH)")
    parser.add_argument("--revision", type=positive_integer, default="1", help="package revision (default: 1)")
    parser.add_argument("--maintainer", default=os.environ.get("RELEASE_MAINTAINER", ""),
                        help="Name <email>; required for native packages; also RELEASE_MAINTAINER")
    parser.add_argument("--check", action="store_true", help="check prerequisites and print the plan without creating files")
    return parser.parse_args(argv)


def elf_architecture(binary):
    with binary.open("rb") as source:
        header = source.read(20)
    if len(header) < 20 or header[:6] != b"\x7fELF\x02\x01":
        raise ValueError(f"expected a 64-bit little-endian Linux ELF binary: {binary}")
    kind, machine = struct.unpack("<HH", header[16:20])
    if kind not in (2, 3):
        raise ValueError(f"expected an executable ELF binary: {binary}")
    for arch, (_, _, identifier) in ARCHITECTURES.items():
        if machine == identifier:
            return arch
    raise ValueError(f"unsupported ELF architecture {machine}: {binary}")


def glibc_requirement(binary, *, required=True):
    env = dict(os.environ, LC_ALL="C")
    result = run(["readelf", "--version-info", binary], capture=True, quiet=True, env=env)
    versions = re.findall(r"\bGLIBC_([0-9]+(?:\.[0-9]+)+)\b", result.stdout)
    if not versions and required:
        raise ValueError(f"cannot determine the glibc requirement of {binary}")
    return max(versions, key=lambda version: tuple(map(int, version.split("."))), default=None)


def package_config(stage, version, revision, arch, maintainer, license_name, glibc):
    contents = [
        {"src": str(source), "dst": "/" + source.relative_to(stage).as_posix(),
         "file_info": {"mode": 0o755 if source.parent == stage / "usr/bin" else 0o644}}
        for source in sorted(stage.rglob("*")) if source.is_file()
    ]
    return {
        "name": "commander", "arch": ARCHITECTURES[arch][0], "platform": "linux",
        "version": version, "release": revision, "maintainer": maintainer,
        "description": "Fast native dual-pane file manager", "license": license_name,
        "section": "utils", "priority": "optional", "contents": contents,
        "overrides": {
            "deb": {"depends": [f"libc6 (>= {glibc})", "libgcc-s1", "libgtk-4-1 (>= 4.14)",
                                "libadwaita-1-0 (>= 1.5)", "libglib2.0-0t64 (>= 2.80)",
                                "libcairo2 (>= 1.16)", "libpango-1.0-0 (>= 1.52)",
                                "libgdk-pixbuf-2.0-0 (>= 2.42)", "adwaita-icon-theme", "shared-mime-info"],
                    "recommends": ["gvfs", "gvfs-backends"]},
            "rpm": {"depends": [f"glibc >= {glibc}", "libgcc", "gtk4 >= 4.14", "libadwaita >= 1.5",
                                "glib2 >= 2.80", "cairo >= 1.16", "pango >= 1.52",
                                "gdk-pixbuf2 >= 2.42", "adwaita-icon-theme", "shared-mime-info"],
                    "recommends": ["gvfs"]},
            "archlinux": {"depends": [f"glibc>={glibc}", "gcc-libs", "gtk4>=4.14", "libadwaita>=1.5",
                                      "glib2>=2.80", "cairo>=1.16", "pango>=1.52",
                                      "gdk-pixbuf2>=2.42", "adwaita-icon-theme", "shared-mime-info"]},
        },
        "archlinux": {"packager": maintainer},
    }


def prerequisites(args):
    required = ["readelf"]
    if args.binary is None:
        required += ["cargo", "pkg-config"]
    if any(item != "appimage" for item in args.formats):
        required += ["nfpm"]
    linuxdeploy = os.environ.get("LINUXDEPLOY", "linuxdeploy")
    if "appimage" in args.formats:
        required += [linuxdeploy, "pkg-config", "file", "find", "glib-compile-schemas"]
    missing = [tool for tool in dict.fromkeys(required) if not shutil.which(tool)]
    if missing:
        raise ValueError("missing release tools: " + ", ".join(missing) + "; see packaging/README.md")
    if args.binary is None or "appimage" in args.formats:
        modules = ["gtk4 >= 4.14", "libadwaita-1 >= 1.5", "gio-2.0 >= 2.80"]
        if "appimage" in args.formats:
            modules += ["librsvg-2.0", "gdk-pixbuf-2.0", "pango", "pangocairo", "pangoft2"]
        run(["pkg-config", "--print-errors", "--exists", *modules])
    if "appimage" in args.formats:
        for resource in ("/usr/share/icons/Adwaita", "/usr/share/icons/hicolor/index.theme", "/usr/share/mime"):
            if not Path(resource).exists():
                raise ValueError(f"missing AppImage resource: {resource}; install adwaita-icon-theme and shared-mime-info")
    deploy_path = shutil.which(linuxdeploy)
    return str(Path(deploy_path).resolve()) if deploy_path else None


def stage_application(stage, binary):
    files = {
        "usr/bin/commander": binary,
        f"usr/share/applications/{APP_ID}.desktop": ROOT / "packaging" / f"{APP_ID}.desktop",
        f"usr/share/icons/hicolor/512x512/apps/{APP_ID}.png": ROOT / "packaging" / f"{APP_ID}.png",
        f"usr/share/icons/hicolor/scalable/apps/{APP_ID}.svg": ROOT / "crates/app/assets/branding/commander.svg",
        "usr/share/doc/commander/README.md": ROOT / "README.md",
    }
    for source in ROOT.glob("LICENSE*"):
        if source.is_file():
            files[f"usr/share/licenses/commander/{source.name}"] = source
    for destination, source in files.items():
        target = stage / destination
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        target.chmod(0o755 if destination == "usr/bin/commander" else 0o644)


def pkg_variable(module, variable, fallback=""):
    value = run(["pkg-config", f"--variable={variable}", module], capture=True, quiet=True).stdout.strip()
    if not value and not fallback:
        raise ValueError(f"pkg-config did not provide {module}'s {variable}")
    return Path(value or fallback)


def stage_gtk_resources(appdir):
    schemas = pkg_variable("gio-2.0", "schemasdir", "/usr/share/glib-2.0/schemas")
    schema_target = appdir / "usr/share/glib-2.0/schemas"
    shutil.copytree(schemas, schema_target)
    run(["glib-compile-schemas", schema_target])
    extra = []
    # These libraries may be on linuxdeploy's default exclusion list, but this
    # GTK application needs versions matching the bundled widgets and modules.
    for module, soname in [("glib-2.0", "libglib-2.0.so.0"), ("gobject-2.0", "libgobject-2.0.so.0"),
                           ("gio-2.0", "libgio-2.0.so.0"), ("gdk-pixbuf-2.0", "libgdk_pixbuf-2.0.so.0"),
                           ("pango", "libpango-1.0.so.0"), ("pangocairo", "libpangocairo-1.0.so.0"),
                           ("pangoft2", "libpangoft2-1.0.so.0"), ("librsvg-2.0", "librsvg-2.so.2")]:
        extra += ["--library", pkg_variable(module, "libdir") / soname]
    module_trees = [(pkg_variable("gtk4", "libdir") / "gtk-4.0", "usr/lib/gtk-4.0"),
                    (pkg_variable("gio-2.0", "giomoduledir", str(pkg_variable("gio-2.0", "libdir") / "gio/modules")),
                     "usr/lib/gio/modules")]
    pixbuf_modules = pkg_variable("gdk-pixbuf-2.0", "gdk_pixbuf_moduledir")
    for source, destination in module_trees:
        target = appdir / destination
        target.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            shutil.copytree(source, target, dirs_exist_ok=True)
            for library in sorted(target.rglob("*.so")):
                extra += ["--deploy-deps-only", library]
    # New GdkPixbuf builds can have only built-in loaders and no module directory.
    loaders = sorted(pixbuf_modules.glob("*.so"))
    if loaders:
        query = pkg_variable("gdk-pixbuf-2.0", "gdk_pixbuf_query_loaders")
        cache = run([query, *loaders], capture=True).stdout
        # linuxdeploy places these libraries next to libgdk_pixbuf with an RPATH
        # pointing there. Basenames in the cache remain valid after relocation.
        cache = cache.replace(str(pixbuf_modules) + "/", "")
        (appdir / "usr/lib/pixbuf-loaders.cache").write_text(cache)
        for library in loaders:
            extra += ["--library", library]
    return extra


def build_appimage(stage, work, destination, arch, version, linuxdeploy):
    appdir = work / "Commander.AppDir"
    shutil.copytree(stage, appdir)
    extra = stage_gtk_resources(appdir)
    shutil.copytree("/usr/share/icons/Adwaita", appdir / "usr/share/icons/Adwaita")
    shutil.copyfile("/usr/share/icons/hicolor/index.theme", appdir / "usr/share/icons/hicolor/index.theme")
    shutil.copytree("/usr/share/mime", appdir / "usr/share/mime")
    apprun = work / "AppRun"
    apprun.write_text('''#!/bin/sh
set -eu
APPDIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export APPDIR
export XDG_DATA_DIRS="$APPDIR/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="$APPDIR/usr/share/glib-2.0/schemas"
export GTK_EXE_PREFIX="$APPDIR/usr"
export GTK_DATA_PREFIX="$APPDIR/usr"
export GTK_PATH="$APPDIR/usr/lib/gtk-4.0"
export GIO_MODULE_DIR="$APPDIR/usr/lib/gio/modules"
if [ -f "$APPDIR/usr/lib/pixbuf-loaders.cache" ]; then
    export GDK_PIXBUF_MODULE_FILE="$APPDIR/usr/lib/pixbuf-loaders.cache"
fi
exec "$APPDIR/usr/bin/commander" "$@"
''')
    apprun.chmod(0o755)
    env = dict(os.environ, ARCH=arch, LDAI_OUTPUT=str(destination),
               LINUXDEPLOY_OUTPUT_VERSION=version, APPIMAGE_EXTRACT_AND_RUN="1")
    run([linuxdeploy, "--appdir", appdir, "--executable", appdir / "usr/bin/commander",
         "--desktop-file", appdir / f"usr/share/applications/{APP_ID}.desktop",
         "--icon-file", appdir / f"usr/share/icons/hicolor/512x512/apps/{APP_ID}.png",
         "--custom-apprun", apprun, *extra, "--output", "appimage"], cwd=work, env=env)
    if not destination.is_file() or not destination.stat().st_size:
        raise ValueError("linuxdeploy did not produce the requested AppImage")
    destination.chmod(0o755)
    # Bundled libraries can raise the baseline beyond the executable's requirement.
    requirements = [glibc_requirement(appdir / "usr/bin/commander")]
    for library in sorted((appdir / "usr/lib").rglob("*.so*")):
        if library.is_file() and not library.is_symlink():
            with library.open("rb") as source:
                if source.read(4) == b"\x7fELF":
                    requirement = glibc_requirement(library, required=False)
                    if requirement:
                        requirements.append(requirement)
    return max(requirements, key=lambda value: tuple(map(int, value.split("."))))


def create_release(args):
    if platform.system() != "Linux" or platform.machine() not in ARCHITECTURES:
        raise ValueError("run on x86_64 or aarch64 Linux; packages target the build machine's architecture")
    arch = platform.machine()
    package = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]
    version = package["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version):
        raise ValueError("workspace version must be X.Y.Z, optionally with a prerelease suffix")
    if any(item != "appimage" for item in args.formats):
        if not re.fullmatch(r"[^<>\r\n]+ <[^<>\s]+@[^<>\s]+>", args.maintainer):
            raise ValueError("set --maintainer 'Name <email>' or RELEASE_MAINTAINER for native packages")
    name = f"commander-{version}-{args.revision}-{arch}"
    output = (args.output or ROOT / "dist/releases" / name).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError(f"release directory already exists: {output}; choose a new --output or --revision")
    binary = args.binary.resolve() if args.binary else None
    if binary and elf_architecture(binary) != arch:
        raise ValueError("the supplied binary must match the build machine's architecture")
    print(f"Release: {name}\nFormats: {', '.join(args.formats)}\nOutput: {output}", flush=True)
    linuxdeploy = prerequisites(args)
    if args.check:
        if binary:
            print(f"Binary requires glibc >= {glibc_requirement(binary)}")
        print("Prerequisites passed; no files created.")
        return
    if binary is None:
        target = ARCHITECTURES[arch][1]
        target_dir = ROOT / "target/package-release"
        run(["cargo", "build", "--release", "--locked", "--package", "dualpane-app",
             "--bin", "commander", "--target", target, "--target-dir", target_dir], cwd=ROOT)
        binary = target_dir / target / "release/commander"
    if elf_architecture(binary) != arch:
        raise ValueError("compiled binary architecture does not match the host")
    output.parent.mkdir(parents=True, exist_ok=True)
    # Build everything privately; publish only once every selected format succeeds.
    with tempfile.TemporaryDirectory(prefix=".commander-release-", dir=output.parent) as directory:
        work = Path(directory)
        stage = work / "stage"
        artifacts = work / "artifacts"
        artifacts.mkdir()
        stage_application(stage, binary)
        glibc = glibc_requirement(stage / "usr/bin/commander")
        print(f"Native packages require glibc >= {glibc}", flush=True)
        config = package_config(stage, version, args.revision, arch, args.maintainer, package["license"], glibc)
        config_path = work / "nfpm.yaml"
        # JSON is valid YAML, and safely represents names and paths containing quotes/spaces.
        config_path.write_text(json.dumps(config, indent=2) + "\n")
        appimage_glibc = None
        packaged_assets = []
        for kind in args.formats:
            if kind == "appimage":
                appimage_glibc = build_appimage(stage, work, artifacts / f"{name}.AppImage", arch,
                                              f"{version}-{args.revision}", linuxdeploy)
            else:
                before = set(artifacts.iterdir())
                run(["nfpm", "package", "--config", config_path, "--packager",
                     "archlinux" if kind == "arch" else kind, "--target", artifacts], cwd=work)
                created = set(artifacts.iterdir()) - before
                suffix = ".pkg.tar.zst" if kind == "arch" else f".{kind}"
                if len(created) != 1 or not all(path.name.endswith(suffix) and path.stat().st_size for path in created):
                    raise ValueError(f"nfpm did not produce exactly one nonempty {suffix} package")
            asset = artifacts / f"{name}.AppImage" if kind == "appimage" else next(iter(created))
            with asset.open("rb") as source:
                asset_digest = hashlib.file_digest(source, "sha256").hexdigest()
            packaged_assets.append({"name": asset.name, "architecture": arch, "format": kind,
                                    "size": asset.stat().st_size, "sha256": asset_digest,
                                    "glibc_minimum": appimage_glibc if kind == "appimage" else glibc})
        with (stage / "usr/bin/commander").open("rb") as source:
            binary_digest = hashlib.file_digest(source, "sha256").hexdigest()
        manifest = {"name": "commander", "version": version, "revision": args.revision,
                    "architecture": arch, "formats": args.formats, "glibc_minimum": glibc,
                    "appimage_glibc_minimum": appimage_glibc,
                    "binary_sha256": binary_digest, "assets": packaged_assets}
        (artifacts / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
        checksums = []
        for artifact in sorted(artifacts.iterdir()):
            with artifact.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            checksums.append(f"{digest}  {artifact.name}\n")
        (artifacts / "SHA256SUMS").write_text("".join(checksums))
        if output.exists() or output.is_symlink():
            raise ValueError(f"release directory appeared during the build: {output}")
        artifacts.rename(output)
    print(f"Release created: {output}")


def main():
    try:
        create_release(arguments())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"release: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("release: interrupted", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())

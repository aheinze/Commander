# Linux releases

Run `release.py` from any directory to build Commander once and create `.deb`,
`.rpm`, Arch Linux `.pkg.tar.zst`, and `.AppImage` files, plus `SHA256SUMS` and a
`release.json` manifest. The version comes from the workspace `Cargo.toml`.
Packages target the host CPU: x86_64 or aarch64. Run separately on each CPU to
produce both architectures. This script creates local artifacts; it does not
publish them or sign them.

## Requirements

- Python 3.11+, Rust 1.96+, a C compiler/linker, `pkg-config`, and GNU `readelf`.
- GTK 4.14+, libadwaita 1.5+, and GLib 2.80+ development packages.
- [nFPM](https://nfpm.goreleaser.com/install/) in `PATH` for deb, rpm, and Arch
  packages. Native `dpkg-deb`, `rpmbuild`, and `makepkg` are not required.
- For AppImage: [linuxdeploy](https://github.com/linuxdeploy/linuxdeploy/releases)
  with its AppImage output plugin (included in the official linuxdeploy AppImage).
  Set `LINUXDEPLOY` to an absolute executable path when using a filename such as
  `linuxdeploy-x86_64.AppImage`. Also install librsvg, GDK Pixbuf, and Pango
  development packages, `file`, `find`, `glib-compile-schemas`, GDK Pixbuf loader
  tools when using external loaders, Adwaita icons, and `shared-mime-info`.
  The release script handles GTK 4 resources directly; no GTK deployment plugin
  is required.

Use pinned, checksum-verified tool releases in a release build environment. The
script checks for tools before building; it does not install missing tools.
AppImage tools run with `APPIMAGE_EXTRACT_AND_RUN=1` to avoid requiring FUSE during
packaging. The generated launcher preserves GTK's normal Wayland/X11 selection.

## Commands

From the repository root:

```sh
export RELEASE_MAINTAINER='Your Name <you@example.org>'
python3 release.py --check
python3 release.py

# Select formats or change the package revision.
python3 release.py --formats deb,rpm,arch --revision 2
python3 release.py --formats appimage

# Package a binary you already built; no Cargo build is run.
python3 release.py --binary target/release/commander --output dist/my-release
```

The default output is `dist/releases/commander-VERSION-REVISION-ARCH/`. Existing
release directories are refused. All requested formats must succeed before the
directory is published; failed staging directories are removed. Cargo builds
use `target/package-release/` to keep development builds separate. `--check`
checks prerequisites without creating directories or building packages.

## Compatibility and verification

Build inside the oldest Linux environment you intend to support, with the required
GTK development stack. Ubuntu 24.04 is a possible baseline for this application's
GTK requirements. A binary built on a newer Arch system can require a newer glibc
than Ubuntu or Fedora ships; changing the package extension cannot fix that.
The script reads ELF version requirements and declares the native binary's glibc
minimum in package dependencies. The manifest also records the highest glibc
requirement found in AppImage's bundled libraries. Follow the
[AppImage compatibility guidance](https://docs.appimage.org/introduction/concepts.html#build-on-old-systems-run-on-newer-systems).

Debian dependencies target Debian/Ubuntu releases with the `t64` GLib package;
RPM dependency names target Fedora-compatible distributions. Arch output is a
binary package installable with `pacman -U`, not an AUR submission. Native
packages use system GTK libraries. AppImage bundles libraries, GTK resources,
icons, and MIME data using linuxdeploy and the script's GTK staging; host graphics drivers,
desktop services, and optional GVfs remote backends still come from the host.

Before distributing a release, run the workspace checks described in the main
README, install each native package in a clean target distribution, and launch
the AppImage on the oldest supported system. Check file operations, icons, and
remote mounts there. Reusing `--binary` assumes it is a release build matching
the current workspace version; the script checks its architecture, not its source
provenance. Verify downloads with `sha256sum -c SHA256SUMS` in the release directory.

Packaging regression checks (no packaging tools or Cargo build required):

```sh
python3 -B -m unittest discover -s packaging -p 'test_release.py'
```

## Automatic GitHub releases

The `.github/workflows/release.yml` workflow builds all four formats for x86_64
and ARM64 on Ubuntu 24.04, then attaches the eight packages, two architecture
manifests, and a combined `SHA256SUMS` file to a GitHub Release.

One-time setup: in the repository's **Settings → Secrets and variables → Actions
→ Variables**, add `RELEASE_MAINTAINER` with a value such as
`Your Name <you@example.org>`. GitHub's built-in `GITHUB_TOKEN` handles publication;
no personal access token is needed. Repository or organization policies must allow
the publishing job's `contents: write` permission.

Commit the workflow, packaging files, and application changes before creating a
tag. Set the workspace version in `Cargo.toml`, keep `Cargo.lock` in sync, then
push a matching tag:

```sh
git tag -a v0.3.0 -m 'Commander 0.3.0'
git push origin v0.3.0
```

Replace `0.3.0` with the version being released. The tag must match the workspace
version exactly. A suffix such as `v0.3.0-rc.1` produces a GitHub prerelease.
Formatting, Clippy, workspace tests, packaging tests, and an AppImage launch under
Xvfb must pass on both architectures before publication. Packaging tools have
pinned versions and SHA-256 checksums; actions are pinned to commit hashes.

The publishing job uploads all files to a draft and then publishes automatically.
Failed uploads leave a draft that can be completed by rerunning the failed job.
Published releases are left unchanged on reruns; use a new version tag to ship
changed binaries. The workflow can also be run from **Actions → Release → Run
workflow** with an existing version tag once it is on the default branch.
Tag pushes made by another workflow's `GITHUB_TOKEN` do not trigger this workflow;
push the tag with your normal Git credentials or use the manual action.

Install testing on other Linux distributions is still separate from these build
and smoke checks. Update the two tool hashes for both architectures together when
changing a packaging tool version in the workflow.

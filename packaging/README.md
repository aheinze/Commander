# Linux releases

Run `release.py` from any directory to build Commander once and create `.deb`,
`.rpm`, Arch Linux `.pkg.tar.zst`, and `.AppImage` files, plus `SHA256SUMS` and a
`release.json` manifest. The version comes from the workspace `Cargo.toml`.
Packages target the host CPU: x86_64 or aarch64. Run separately on each CPU to
produce both architectures. This script creates local artifacts; it does not
publish them or sign them.

## Requirements

- Python 3.11+, Rust 1.96+, a C compiler/linker, `pkg-config`, and GNU `readelf`.
  Publishing signed update metadata also requires OpenSSL 3.
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
python3 -B -m unittest discover -s packaging -p 'test_*.py'
```

## Automatic GitHub releases

The `.github/workflows/release.yml` workflow builds all four formats for x86_64
and ARM64 on Ubuntu 24.04, then attaches the eight packages, two architecture
manifests, a signed `update.json` with its `update.json.sig`, and a combined
`SHA256SUMS` file to a GitHub Release.

Version-tag pushes (`v*`) start the Release workflow only. Ordinary branch pushes
and pull requests do not start workflows. The separate CI suite remains available
from **Actions → CI → Run workflow** for an explicit validation run.

One-time setup: in the repository's **Settings → Secrets and variables → Actions
→ Variables**, add `RELEASE_MAINTAINER` with a value such as
`Your Name <you@example.org>`. GitHub's built-in `GITHUB_TOKEN` handles publication;
no personal access token is needed. Repository or organization policies must allow
the publishing job's `contents: write` permission.

Configure the [update signing key](#update-signing-key) before the first release
using the updater. Release builds embed its public key; publication requires the
matching private key. Unsigned releases remain accessible through GitHub, but the
app does not offer their assets as verified downloads.

Commit the workflow, packaging files, and application changes before creating a
tag. Set the workspace version in `Cargo.toml`, keep `Cargo.lock` in sync, then
push a matching tag:

```sh
git tag -a v0.3.0 -m 'Commander 0.3.0'
git push origin v0.3.0
```

Replace `0.3.0` with the version being released. The tag must match the workspace
version exactly. A suffix such as `v0.3.0-rc.1` produces a GitHub prerelease.
Formatting, Clippy, workspace tests, packaging tests, clean Ubuntu/Fedora native
package install/upgrade/removal checks, and an AppImage launch under Xvfb must pass
on both architectures before publication. Packaging tools have
pinned versions and SHA-256 checksums; actions are pinned to commit hashes.

The publishing job uploads all files to a draft and then publishes automatically.
Failed uploads leave a draft that can be completed by rerunning the failed job.
Published releases are left unchanged on reruns; use a new version tag to ship
changed binaries. The workflow can also be run from **Actions → Release → Run
workflow** with an existing version tag once it is on the default branch.
Tag pushes made by another workflow's `GITHUB_TOKEN` do not trigger this workflow;
push the tag with your normal Git credentials or use the manual action.

The [release checks](../docs/RELEASE_CHECKS.md#clean-native-package-lifecycle)
document how to run the container package checks locally. Full desktop-session
testing and other Linux distributions remain separate. Update the two tool hashes
for both architectures together when changing a packaging tool version in the workflow.

## Update signing key

Generate an Ed25519 key outside the checkout and keep a secure backup. OpenSSL 3
is used only by release tooling; the application verifies signatures in Rust.

```sh
umask 077
mkdir -p "$HOME/.config/commander-release"
openssl genpkey -algorithm Ed25519 -out "$HOME/.config/commander-release/private.pem"
python3 packaging/update_manifest.py public-key "$HOME/.config/commander-release/private.pem"
```

In the repository's **Settings → Secrets and variables → Actions**, set:

- Variable `COMMANDER_UPDATE_PUBLIC_KEY`: the 64-character hexadecimal public key
  printed by the command.
- Secret `COMMANDER_UPDATE_PRIVATE_KEY`: the complete PEM contents of `private.pem`.
  Keep this private key out of source control and release assets.

The public key is passed to both architecture builds. The private key is available
only to the publishing job's signing step, which confirms the two keys match,
rechecks all eight package hashes and sizes, and signs the exact manifest bytes.
The signature is a raw 64-byte Ed25519 signature. `SHA256SUMS` includes both new
metadata files. No production key is generated or configured by the workflow.

Enable [immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
in GitHub repository settings. The workflow already uploads to a draft before
publishing. Release changes require a new version tag, including packaging fixes.
Keep the signing key stable: changing the repository variable alone will prevent
older clients from trusting new releases. Key rotation needs an explicit transition
release or a manual reinstall; automatic key rotation is not implemented.

For a local build with verified downloads enabled, export the same public key as
`COMMANDER_UPDATE_PUBLIC_KEY` before running Cargo. Builds without it can check
versions and show release notes, but direct verified downloads remain unavailable.

## In-app update behavior

Settings → About offers manual stable-release checks, release notes, a link to the
release, and downloads for compatible architectures and glibc versions. Discovery
uses GitHub's public API without a token, with ETag caching and rate-limit backoff.
Checks are throttled to once per minute. API errors never report “up to date.”

Downloads stream over HTTPS to a temporary file in the chosen local directory.
Commander checks the signed size and SHA-256 before publishing the file, refuses
to overwrite existing files, and removes partial files on handled failures or
cancellation. Verified AppImages are saved executable for the current user;
other packages remain private, non-executable files. Closing Settings cancels pending work; cancellation is checked
between network reads, which can remain blocked until the request times out
(30 seconds for metadata, 10 minutes for a package). Abrupt termination can leave
a hidden `.commander-download-*` temporary file; it is never offered as a verified
download. These files can be removed manually after Commander exits.

The app recognizes AppImage context and package-manager ownership to suggest a
format. Native packages still need the matching system libraries; the package
manager resolves these dependencies during installation. Source/unknown builds
offer compatible formats for manual installation. Distribution-managed users
should prefer their distribution's updater. No package manager, installer, shell,
or downloaded executable is launched automatically. Daily checks, prerelease
channels, AppImage replacement, restart, and rollback are later increments.

#!/usr/bin/env python3
"""Install, upgrade, launch, and remove native packages in disposable containers."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

COMMON = r'''
mkdir -p /tmp/commander-fixture /tmp/commander-state/config
printf 'package test\n' > /tmp/commander-fixture/example.txt
printf 'keep my preferences\n' > /tmp/commander-state/config/retention-proof
export XDG_CONFIG_HOME=/tmp/commander-state/config
export XDG_CACHE_HOME=/tmp/commander-state/cache
export XDG_DATA_HOME=/tmp/commander-state/data
export XDG_STATE_HOME=/tmp/commander-state/state
export GSETTINGS_BACKEND=memory GTK_A11Y=none GSK_RENDERER=cairo GIO_USE_VFS=local GDK_BACKEND=x11
export LIBGL_ALWAYS_SOFTWARE=1
smoke() {
  commander --version
  desktop-file-validate /usr/share/applications/org.example.Dualpane.desktop
  test -s /usr/share/icons/hicolor/512x512/apps/org.example.Dualpane.png
  test -s /usr/share/icons/hicolor/scalable/apps/org.example.Dualpane.svg
  timeout 30s xvfb-run -a dbus-run-session -- commander --benchmark /tmp/commander-fixture --quit-after-first-paint > "/artifacts/$1.log" 2>&1
  cat "/artifacts/$1.log"
  if grep -E 'Theme parser error|CRITICAL|reported min (width|height) -|error while loading shared libraries|undefined symbol' "/artifacts/$1.log"; then
    echo 'Native launch reported a runtime compatibility error' >&2
    exit 1
  fi
}
'''
UBUNTU = r'''
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends xvfb xauth dbus-daemon desktop-file-utils
apt-get install -y --no-install-recommends /initial/*.deb
smoke initial
initial_version=$(dpkg-query -W -f='${Version}' commander)
apt-get install -y --no-install-recommends /upgrade/*.deb
upgraded_version=$(dpkg-query -W -f='${Version}' commander)
test "$initial_version" != "$upgraded_version"
smoke upgraded
apt-get remove -y commander
test ! -e /usr/bin/commander
test ! -e /usr/share/applications/org.example.Dualpane.desktop
test "$(cat /tmp/commander-state/config/retention-proof)" = 'keep my preferences'
'''
FEDORA = r'''
dnf install -y --setopt=install_weak_deps=False xorg-x11-server-Xvfb xorg-x11-xauth dbus-daemon desktop-file-utils
dnf install -y --setopt=install_weak_deps=False /initial/*.rpm
smoke initial
initial_version=$(rpm -q commander)
dnf upgrade -y --setopt=install_weak_deps=False /upgrade/*.rpm
upgraded_version=$(rpm -q commander)
test "$initial_version" != "$upgraded_version"
smoke upgraded
dnf remove -y commander
test ! -e /usr/bin/commander
test ! -e /usr/share/applications/org.example.Dualpane.desktop
test "$(cat /tmp/commander-state/config/retention-proof)" = 'keep my preferences'
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--initial", type=Path, required=True)
    parser.add_argument("--upgrade", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New artifact directory")
    parser.add_argument("--runtime", default="podman")
    parser.add_argument("--ubuntu-image", default="docker.io/library/ubuntu:24.04")
    parser.add_argument("--fedora-image", default="registry.fedoraproject.org/fedora:44")
    args = parser.parse_args()
    for folder in (args.initial, args.upgrade):
        for extension in ("deb", "rpm"):
            if len(list(folder.glob(f"*.{extension}"))) != 1:
                parser.error(f"{folder} must contain exactly one .{extension} package")
        subprocess.run(["sha256sum", "--check", "SHA256SUMS"], cwd=folder, check=True)
    manifests = [json.loads((folder / "release.json").read_text()) for folder in (args.initial, args.upgrade)]
    if manifests[0]["architecture"] != manifests[1]["architecture"]:
        parser.error("Initial and upgrade packages must target the same architecture")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    results = []
    for name, image, commands in [("ubuntu", args.ubuntu_image, UBUNTU), ("fedora", args.fedora_image, FEDORA)]:
        directory = output / name
        directory.mkdir()
        print(f"Checking {name}: install → launch → upgrade → launch → remove", flush=True)
        started = time.monotonic()
        with (directory / "install.log").open("w") as log:
            result = subprocess.run(
                [args.runtime, "run", "--rm", "--security-opt", "label=disable",
                 "--name", f"commander-package-check-{name}-{os.getpid()}",
                 "-v", f"{args.initial.resolve()}:/initial:ro", "-v", f"{args.upgrade.resolve()}:/upgrade:ro",
                 "-v", f"{directory}:/artifacts:rw", image, "sh", "-euxc", COMMON + commands],
                stdout=log, stderr=subprocess.STDOUT, check=False,
            )
        results.append({"distribution": name, "image": image, "exit_code": result.returncode, "seconds": round(time.monotonic() - started, 2)})
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(f"{'PASS' if result.returncode == 0 else 'FAIL'}: {directory / 'install.log'}", flush=True)
    return int(any(result["exit_code"] for result in results))

if __name__ == "__main__":
    raise SystemExit(main())

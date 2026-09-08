#!/usr/bin/env python3
"""Run ignored GTK regressions, one process and disposable session per test."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
# This integration test needs a provisioned FTP server and GVfs backend. It is
# deliberately separate from the self-contained native suite.
EXTERNAL_TESTS = {
    "app::remote::tests::ftp_mount_uses_supplied_login_and_opens_requested_folder",
    "app::remote::tests::ftp_disconnect_preserves_destination_and_reports_failure",
}


def execute(binary, test, backend):
    command = [binary, test, "--exact", "--ignored", "--test-threads=1", "--nocapture"]
    if backend == "x11":
        return subprocess.call(
            ["xvfb-run", "-a", "-s", "-screen 0 1800x1400x24", *command]
        )
    compositor_log = Path(os.environ["COMMANDER_TEST_ARTIFACTS"]) / "compositor.log"
    with compositor_log.open("w") as log:
        compositor = subprocess.Popen(
            ["mutter", "--headless", "--no-x11", "--wayland-display", "commander-test",
             "--virtual-monitor", "1800x1400"], stdout=log, stderr=subprocess.STDOUT
        )
        try:
            socket = Path(os.environ["XDG_RUNTIME_DIR"]) / "commander-test"
            deadline = time.monotonic() + 15
            while not socket.is_socket():
                if compositor.poll() is not None or time.monotonic() > deadline:
                    print(compositor_log.read_text(), flush=True)
                    raise RuntimeError("Headless Mutter did not start")
                time.sleep(0.05)
            return subprocess.call(command)
        finally:
            if compositor.poll() is None:
                compositor.terminate()
                try:
                    compositor.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    compositor.kill()
                    compositor.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11")
    parser.add_argument("--filter", default="", help="Run test names containing this text")
    parser.add_argument("--timeout", type=int, default=120, help="Seconds allowed per test")
    parser.add_argument("--output", type=Path, help="New directory for logs and screenshots")
    parser.add_argument("--execute", nargs=2, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.execute:
        return execute(*args.execute, args.backend)
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    required = ["cargo", "dbus-run-session", "timeout"]
    required += ["xvfb-run", "Xvfb", "xauth"] if args.backend == "x11" else ["mutter"]
    missing = [tool for tool in required if shutil.which(tool) is None]
    if missing:
        parser.error("Missing tools: " + ", ".join(missing))

    output = args.output or ROOT / "target/native-tests" / f"{time.strftime('%Y%m%d-%H%M%S')}-{os.getpid()}"
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    print(f"Native test artifacts: {output}", flush=True)
    build = subprocess.run(
        ["cargo", "test", "--package", "dualpane-app", "--locked", "--no-run", "--message-format=json"],
        cwd=ROOT, text=True, stdout=subprocess.PIPE, check=False,
    )
    (output / "build.jsonl").write_text(build.stdout)
    artifacts = []
    for line in build.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            print(message["message"].get("rendered", ""), end="", file=sys.stderr)
        if message.get("reason") == "compiler-artifact" and message.get("executable") and message["profile"]["test"]:
            artifacts.append(message["executable"])
    if build.returncode:
        return build.returncode
    if len(artifacts) != 1:
        raise RuntimeError(f"Expected one app test executable; found {len(artifacts)}")
    # Cargo can replace target/debug/deps during another workspace build.
    # Every test in this run must exercise the same executable.
    binary = str(output / "commander-tests")
    shutil.copy2(artifacts[0], binary)
    listing = subprocess.check_output([binary, "--list", "--ignored", "--format", "terse"], text=True)
    tests = [line.removesuffix(": test") for line in listing.splitlines() if line.endswith(": test")]
    tests = [test for test in tests if test not in EXTERNAL_TESTS and args.filter in test]
    if not tests:
        parser.error("No native tests matched; refusing an empty successful run")
    results = []
    for index, test in enumerate(tests, 1):
        directory = output / test.replace("::", "-")
        directory.mkdir()
        environment = os.environ.copy()
        for key in ("CONFIG", "CACHE", "STATE", "DATA"):
            location = directory / key.lower()
            location.mkdir()
            environment[f"XDG_{key}_HOME"] = str(location)
        # Do not inherit test instrumentation from a developer's other session.
        for key in list(environment):
            if key.startswith("COMMANDER_") and ("SNAPSHOT" in key or key.endswith("PREVIEW_DIR")):
                del environment[key]
        snapshots = directory / "snapshots"
        snapshots.mkdir()
        for name in ("MILLER", "SEARCH", "COMPARE", "REMOTE", "JOB", "TERMINAL"):
            environment[f"COMMANDER_{name}_SNAPSHOT_DIR"] = str(snapshots)
        environment.update(
            COMMANDER_ISOLATED_TEST="1", COMMANDER_TEST_ARTIFACTS=str(directory),
            GSETTINGS_BACKEND="memory", GTK_A11Y="none", GSK_RENDERER="cairo",
            GIO_USE_VFS="local", GIO_USE_VOLUME_MONITOR="unix", NO_AT_BRIDGE="1",
            GDK_BACKEND=args.backend, SHELL="/bin/sh", RUST_BACKTRACE="1",
        )
        environment.pop("DISPLAY", None)
        environment.pop("WAYLAND_DISPLAY", None)
        if args.backend == "wayland":
            environment["WAYLAND_DISPLAY"] = "commander-test"
        print(f"[{index}/{len(tests)}] {test}", flush=True)
        started = time.monotonic()
        with tempfile.TemporaryDirectory(prefix="commander-native-") as runtime:
            # Short, private runtime paths also stay within Unix socket limits.
            environment["XDG_RUNTIME_DIR"] = runtime
            # No host service activation: portals, keyrings, accounts and GVfs
            # mounts must not outlive or interfere with the disposable session.
            bus_config = Path(runtime) / "session.conf"
            bus_config.write_text('''<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>''')
            with (directory / "test.log").open("w") as log:
                run = subprocess.run(
                    ["timeout", "--kill-after=10s", f"{args.timeout}s", "dbus-run-session",
                     "--config-file", str(bus_config), "--",
                     sys.executable, "-B", str(Path(__file__).resolve()), "--backend", args.backend,
                     "--execute", binary, test],
                    cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT, check=False,
                )
        result = {"test": test, "exit_code": run.returncode, "seconds": round(time.monotonic() - started, 2)}
        results.append(result)
        (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        if run.returncode:
            print((directory / "test.log").read_text(), file=sys.stderr)
            print(f"FAIL ({run.returncode}): {test}", flush=True)
        else:
            print(f"PASS ({result['seconds']}s)", flush=True)
    failures = sum(result["exit_code"] != 0 for result in results)
    print(f"{len(results) - failures} passed, {failures} failed. Logs: {output}")
    return int(failures > 0)


if __name__ == "__main__":
    sys.exit(main())

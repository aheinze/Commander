#!/usr/bin/env python3
"""Run real GVfs FTP authentication and interrupted-transfer checks on loopback."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

TESTS = [
    "app::remote::tests::ftp_mount_uses_supplied_login_and_opens_requested_folder",
    "app::remote::tests::ftp_disconnect_preserves_destination_and_reports_failure",
]

def serve(directory):
    from pyftpdlib.authorizers import DummyAuthorizer
    from pyftpdlib.handlers import FTPHandler, ThrottledDTPHandler
    from pyftpdlib.servers import FTPServer
    from pyftpdlib.ioloop import IOLoop
    authorizer = DummyAuthorizer()
    authorizer.add_user("commander-test", "test-password", str(directory / "files"), perm="elr")
    authorizer.add_anonymous(str(directory / "files"), perm="elr")
    class SlowData(ThrottledDTPHandler):
        write_limit = 512 * 1024
    class Handler(FTPHandler):
        dtp_handler = SlowData
    Handler.authorizer = authorizer
    server = FTPServer(("127.0.0.1", 0), Handler)
    (directory / "ready.json").write_text(json.dumps({"port": server.socket.getsockname()[1]}))
    def drop_when_requested():
        if (directory / "disconnect").exists():
            server.close_all()
            os._exit(0)
    IOLoop.instance().call_every(0.05, drop_when_requested)
    server.serve_forever(timeout=0.1)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--serve", type=Path)
    parser.add_argument("--client", nargs=2)
    args = parser.parse_args()
    if args.serve:
        serve(args.serve)
        return 0
    if args.client:
        binary, output = args.client
        for index, test in enumerate(TESTS):
            with (Path(output) / f"test-{index}.log").open("w") as log:
                result = subprocess.run(["timeout", "--kill-after=5s", "90s", binary, test, "--exact", "--ignored", "--test-threads=1", "--nocapture"], stdout=log, stderr=subprocess.STDOUT)
            if result.returncode:
                return result.returncode
        return 0
    try:
        import pyftpdlib  # noqa: F401
    except ImportError:
        parser.error("Install pyftpdlib==2.1.0 in a test virtual environment first")
    root = Path(__file__).resolve().parents[1]
    output = (args.output or root / "target/ftp-tests" / f"{time.strftime('%Y%m%d-%H%M%S')}-{os.getpid()}").resolve()
    output.mkdir(parents=True, exist_ok=False)
    build = subprocess.run(["cargo", "test", "--package", "dualpane-app", "--locked", "--no-run", "--message-format=json"], cwd=root, text=True, stdout=subprocess.PIPE)
    (output / "build.jsonl").write_text(build.stdout)
    if build.returncode:
        print(build.stdout)
        return build.returncode
    binaries = [item["executable"] for item in map(json.loads, build.stdout.splitlines()) if item.get("reason") == "compiler-artifact" and item.get("executable") and item["profile"]["test"]]
    if len(binaries) != 1:
        raise RuntimeError("Expected exactly one app test executable")
    binary = str(output / "commander-tests")
    shutil.copy2(binaries[0], binary)
    files = output / "files/nested"
    files.mkdir(parents=True)
    (files / "proof.txt").write_text("FTP connection verified\n")
    (files / "large.bin").write_bytes(b"N" * (32 * 1024 * 1024))
    with (output / "server.log").open("w") as log:
        server = subprocess.Popen([sys.executable, "-B", str(Path(__file__).resolve()), "--serve", str(output)], stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 10
            while not (output / "ready.json").exists():
                if server.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("Loopback FTP fixture did not start")
                time.sleep(0.05)
            environment = os.environ.copy()
            for key in ("CONFIG", "CACHE", "DATA", "STATE"):
                directory = output / key.lower(); directory.mkdir()
                environment[f"XDG_{key}_HOME"] = str(directory)
            environment.update(COMMANDER_FTP_TEST_PORT=str(json.loads((output / "ready.json").read_text())["port"]), COMMANDER_FTP_DROP_FILE=str(output / "disconnect"), GIO_USE_VFS="gvfs", GIO_USE_VOLUME_MONITOR="unix", RUST_BACKTRACE="1")
            environment.pop("GVFS_DISABLE_FUSE", None)
            with tempfile.TemporaryDirectory(prefix="commander-ftp-") as runtime:
                environment["XDG_RUNTIME_DIR"] = runtime
                result = subprocess.run(["dbus-run-session", "--", sys.executable, "-B", str(Path(__file__).resolve()), "--client", binary, str(output)], env=environment)
                # The private GVfs mount belongs exclusively to this test bus.
                subprocess.run(["fusermount3", "-u", str(Path(runtime) / "gvfs")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            assert (files / "large.bin").stat().st_size == 32 * 1024 * 1024
            (output / "results.json").write_text(json.dumps({"tests": TESTS, "exit_code": result.returncode}, indent=2) + "\n")
            print(f"{'PASS' if result.returncode == 0 else 'FAIL'}: FTP checks. Logs: {output}")
            return result.returncode
        finally:
            if server.poll() is None:
                server.terminate()
            server.wait(timeout=5)

if __name__ == "__main__":
    raise SystemExit(main())

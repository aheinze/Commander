#!/usr/bin/env python3
"""Exercise transfers, browsing and deletion through a disposable OpenSSH/GVfs mount."""
import argparse
import pwd
import re
import signal
import json
import os
from pathlib import Path
import shutil
import socket
import shlex
import subprocess
import sys
import tempfile
import time
from urllib.parse import quote

PREFIX = "app::remote::sftp_tests::"
ROUND_TRIP = PREFIX + "sftp_move_browse_copy_back_and_delete_preserve_contents"
TESTS = [ROUND_TRIP,
    PREFIX + "sftp_disconnect_during_upload_preserves_sources_and_retries_after_reconnect",
    PREFIX + "sftp_disconnect_during_download_preserves_original_and_retries_after_reconnect",
    PREFIX + "sftp_stalled_listing_times_out_and_cancels_without_waiting_for_io",
    PREFIX + "sftp_stalled_transfer_cancel_keeps_the_source_and_original",
]


def run_tests(binary, fixture, output, uri, tests, wrappers, *, real=False, browse=None):
    if not tests:
        raise ValueError("No SFTP tests matched")
    results = []
    for index, test in enumerate(tests):
        directory = fixture / f"client-{index}"
        directory.mkdir()
        environment = os.environ.copy()
        environment["PATH"] = str(wrappers) + os.pathsep + environment["PATH"]
        for key in ("CONFIG", "CACHE", "DATA", "STATE"):
            location = directory / key.lower()
            location.mkdir()
            environment[f"XDG_{key}_HOME"] = str(location)
        # OpenSSH appends a 40-character control key and a temporary suffix.
        # Keep this shorter than the Unix-domain socket path limit.
        runtime = fixture / f"r{index}"
        runtime.mkdir(mode=0o700)
        environment.update(XDG_RUNTIME_DIR=str(runtime), COMMANDER_SFTP_TEST_URI=uri,
            COMMANDER_SFTP_CONTROL=str(fixture), COMMANDER_SFTP_ROOT=str(fixture / "files"),
            GIO_USE_VFS="gvfs", GIO_USE_VOLUME_MONITOR="unix", RUST_BACKTRACE="1")
        environment.pop("GVFS_DISABLE_FUSE", None)
        environment.pop("COMMANDER_SFTP_REAL_SERVER", None)
        environment.pop("COMMANDER_SFTP_BROWSE_URI", None)
        if real:
            environment["COMMANDER_SFTP_REAL_SERVER"] = "1"
            if browse:
                environment["COMMANDER_SFTP_BROWSE_URI"] = browse
        else:
            for name in ("SSH_AUTH_SOCK", "SSH_AGENT_PID"):
                environment.pop(name, None)
            request = fixture / "control.tmp"
            request.write_text(json.dumps({"serial": f"reset-{index}", "mode": "forward", "rate": 0}))
            request.replace(fixture / "control.json")
            deadline = time.monotonic() + 5
            while not (fixture / "ack.json").exists() or json.loads((fixture / "ack.json").read_text())["serial"] != f"reset-{index}":
                if time.monotonic() > deadline:
                    raise RuntimeError("SFTP fault proxy did not reset")
                time.sleep(0.01)
        print(f"[{index+1}/{len(tests)}] {test}", flush=True)
        started = time.monotonic()
        command = ["dbus-run-session", "--", "timeout", "--kill-after=5s", "150s", str(binary), test, "--exact", "--ignored", "--test-threads=1", "--nocapture"]
        try:
            with (output / f"test-{index}.log").open("w") as log:
                result = subprocess.run(command, env=environment, stdout=log, stderr=subprocess.STDOUT)
        finally:
            subprocess.run(["fusermount3", "-u", str(runtime / "gvfs")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
        results.append({"test": test, "exit_code": result.returncode, "seconds": round(time.monotonic()-started, 2)})
        if result.returncode:
            print((output / f"test-{index}.log").read_text())
        print("PASS" if result.returncode == 0 else "FAIL", flush=True)
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return int(any(result["exit_code"] for result in results))


def remote_test(args, binary, output):
    # Explicit opt-in: use the user's existing key/agent and pinned host key.
    if not re.fullmatch(r"[A-Za-z0-9_.-]+@[A-Za-z0-9.-]+", args.remote):
        raise ValueError("--remote must be user@hostname, without credentials or shell syntax")
    ssh = [shutil.which("ssh"), "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=10", args.remote]
    location = subprocess.check_output([*ssh, "mktemp -d /tmp/commander-sftp-XXXXXXXXXXXX"], text=True, timeout=20).strip()
    if not re.fullmatch(r"/tmp/commander-sftp-[A-Za-z0-9]+", location):
        raise RuntimeError("Server returned an unexpected temporary directory")
    print(f"Testing {args.remote} in disposable directory {location}", flush=True)
    try:
        with tempfile.TemporaryDirectory(prefix="commander-sftp-") as temporary:
            fixture = Path(temporary)
            private_binary = fixture / "commander-tests"
            shutil.copy2(binary, private_binary)
            wrappers = fixture / "bin"
            wrappers.mkdir()
            wrapper = wrappers / "ssh"
            wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(shutil.which("ssh")) + ' -o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=10 "$@"\n')
            wrapper.chmod(0o700)
            uri = f"sftp://{args.remote}{quote(location)}"
            tests = [ROUND_TRIP]
            browse = None
            if args.browse:
                if not args.browse.startswith("/"):
                    raise ValueError("--browse must be an absolute read-only remote directory")
                browse = f"sftp://{args.remote}{quote(args.browse)}"
                tests.append(PREFIX + "sftp_existing_folder_is_readable")
            return run_tests(private_binary, fixture, output, uri, tests, wrappers, real=True, browse=browse)
    finally:
        # Only remove the exact newly allocated directory; never a user's destination.
        subprocess.run([*ssh, "rm -rf -- " + shlex.quote(location)], check=True, timeout=20)
        print("Removed the disposable remote test directory", flush=True)



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--filter", default="", help="Run matching loopback scenarios")
    parser.add_argument("--remote", help="Opt in to a real-server round trip in a newly allocated /tmp directory (user@host)")
    parser.add_argument("--browse", help="Also list this existing remote directory read-only; requires --remote")
    args = parser.parse_args()
    if args.browse and not args.remote:
        parser.error("--browse requires --remote")
    required = ("cargo", "sshd", "ssh-keygen", "ssh", "dbus-run-session", "timeout", "fusermount3")
    missing = [name for name in required if shutil.which(name) is None]
    if missing:
        parser.error("Missing tools: " + ", ".join(missing))
    if os.getuid() == 0:
        parser.error("Run the disposable SSH fixture as a normal user")
    root = Path(__file__).resolve().parents[1]
    output = (args.output or root / "target/sftp-tests" / f"{time.strftime('%Y%m%d-%H%M%S')}-{os.getpid()}").resolve()
    output.mkdir(parents=True, exist_ok=False)
    build = subprocess.run(["cargo", "test", "--package", "dualpane-app", "--locked", "--no-run", "--message-format=json"], cwd=root, text=True, stdout=subprocess.PIPE)
    (output / "build.jsonl").write_text(build.stdout)
    if build.returncode:
        for line in build.stdout.splitlines():
            item = json.loads(line)
            if item.get("reason") == "compiler-message":
                print(item["message"].get("rendered", ""), file=sys.stderr)
        return build.returncode
    binaries = [item["executable"] for item in map(json.loads, build.stdout.splitlines()) if item.get("reason") == "compiler-artifact" and item.get("executable") and item["profile"]["test"]]
    if len(binaries) != 1:
        raise RuntimeError("Expected exactly one app test executable")
    binary = output / "commander-tests"
    shutil.copy2(binaries[0], binary)
    if args.remote:
        return remote_test(args, binary, output)
    # Keep credentials, SSH state and the server PID outside permanent artifacts.
    with tempfile.TemporaryDirectory(prefix="commander-sftp-") as temporary:
        fixture = Path(temporary)
        private_binary = fixture / "commander-tests"
        shutil.copy2(binary, private_binary)
        home = fixture / "home"
        ssh = home / ".ssh"
        ssh.mkdir(parents=True, mode=0o700)
        files = fixture / "files"
        files.mkdir()
        for key in (fixture / "host-key", ssh / "id_ed25519"):
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)], check=True)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        account = pwd.getpwuid(os.getuid())
        user = account.pw_name
        config = fixture / "sshd_config"
        config.write_text(f'''ListenAddress 127.0.0.1
Port {port}
HostKey "{fixture / 'host-key'}"
PidFile "{fixture / 'sshd.pid'}"
AuthorizedKeysFile "{ssh / 'id_ed25519.pub'}"
AllowUsers {user}
UsePAM no
StrictModes no
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
DisableForwarding yes
ForceCommand internal-sftp
Subsystem sftp internal-sftp
''')
        host_key = (fixture / "host-key.pub").read_text().split()
        (ssh / "known_hosts").write_text(f"[127.0.0.1]:{port} {host_key[0]} {host_key[1]}\n")
        client_config = fixture / "ssh_config"
        client_config.write_text(f'''Host *
    IdentityFile "{ssh / 'id_ed25519'}"
    IdentitiesOnly yes
    UserKnownHostsFile "{ssh / 'known_hosts'}"
    GlobalKnownHostsFile /dev/null
    StrictHostKeyChecking yes
    BatchMode yes
''')
        wrappers = fixture / "bin"
        wrappers.mkdir()
        wrapper = wrappers / "ssh"
        wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(shutil.which("ssh")) + " -F " + shlex.quote(str(client_config)) + ' "$@"\n')
        wrapper.chmod(0o700)
        with (output / "server.log").open("w") as log:
            server = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(config)], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            proxy = None
            try:
                deadline = time.monotonic() + 10
                while True:
                    if server.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("Loopback sshd did not start: " + (output / "server.log").read_text())
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                            break
                    except OSError:
                        time.sleep(0.05)
                with (output / "proxy.log").open("w") as proxy_log:
                    proxy = subprocess.Popen([sys.executable, "-B", str(root / "scripts/sftp-proxy.py"), str(port), str(fixture)], stdout=proxy_log, stderr=subprocess.STDOUT)
                    deadline = time.monotonic() + 5
                    while not (fixture / "proxy.json").exists():
                        if proxy.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("SFTP fault proxy failed to start")
                        time.sleep(0.01)
                    proxy_port = json.loads((fixture / "proxy.json").read_text())["port"]
                    (ssh / "known_hosts").write_text(f"[127.0.0.1]:{proxy_port} {host_key[0]} {host_key[1]}\n")
                    result = run_tests(private_binary, fixture, output, f"sftp://{user}@127.0.0.1:{proxy_port}{quote(str(files))}", [test for test in TESTS if args.filter in test], wrappers)
                    print(f"{'PASS' if result == 0 else 'FAIL'}: SFTP checks. Logs: {output}")
                    return result
            finally:
                if proxy is not None:
                    proxy.terminate()
                    proxy.wait(timeout=5)
                if server.poll() is None:
                    os.killpg(server.pid, signal.SIGTERM)
                server.wait(timeout=5)


if __name__ == "__main__":
    raise SystemExit(main())

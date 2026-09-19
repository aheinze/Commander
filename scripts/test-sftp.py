#!/usr/bin/env python3
"""Exercise transfers, browsing and deletion through a disposable OpenSSH/GVfs mount."""
import argparse
import pwd
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

TEST = "app::remote::sftp_tests::sftp_move_browse_copy_back_and_delete_preserve_contents"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
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
            server = subprocess.Popen([shutil.which("sshd"), "-D", "-e", "-f", str(config)], stdout=log, stderr=subprocess.STDOUT)
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
                environment = os.environ.copy()
                environment["PATH"] = str(wrappers) + os.pathsep + environment["PATH"]
                for key in ("CONFIG", "CACHE", "DATA", "STATE"):
                    directory = fixture / key.lower()
                    directory.mkdir()
                    environment[f"XDG_{key}_HOME"] = str(directory)
                runtime = fixture / "runtime"
                runtime.mkdir(mode=0o700)
                environment.update(XDG_RUNTIME_DIR=str(runtime), COMMANDER_SFTP_TEST_URI=f"sftp://{user}@127.0.0.1:{port}{quote(str(files))}", GIO_USE_VFS="gvfs", GIO_USE_VOLUME_MONITOR="unix", RUST_BACKTRACE="1")
                for name in ("GVFS_DISABLE_FUSE", "SSH_AUTH_SOCK", "SSH_AGENT_PID"):
                    environment.pop(name, None)
                # GVfs resolves ssh from PATH. The private wrapper supplies only
                # this fixture's identity and pinned host key; user SSH files are
                # neither read nor written. FUSE stays in the original namespace.
                command = ["dbus-run-session", "--", "timeout", "--kill-after=5s", "120s", str(private_binary), TEST, "--exact", "--ignored", "--test-threads=1", "--nocapture"]
                with (output / "test.log").open("w") as test_log:
                    result = subprocess.run(command, env=environment, stdout=test_log, stderr=subprocess.STDOUT)
                subprocess.run(["fusermount3", "-u", str(runtime / "gvfs")], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                (output / "results.json").write_text(json.dumps({"test": TEST, "exit_code": result.returncode}, indent=2) + "\n")
                print(f"{'PASS' if result.returncode == 0 else 'FAIL'}: SFTP checks. Logs: {output}")
                if result.returncode:
                    print((output / "test.log").read_text())
                return result.returncode
            finally:
                if server.poll() is None:
                    server.terminate()
                server.wait(timeout=5)


if __name__ == "__main__":
    raise SystemExit(main())

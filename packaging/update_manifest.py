#!/usr/bin/env python3
"""Create the signed update feed from both verified architecture builds."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

REPOSITORY = "aheinze/Commander"
SUFFIXES = {"deb": ".deb", "rpm": ".rpm", "arch": ".pkg.tar.zst", "appimage": ".AppImage"}


def openssl(*args):
    result = subprocess.run(["openssl", *map(str, args)], capture_output=True, check=False)
    if result.returncode:
        raise ValueError("OpenSSL could not process the Ed25519 release signing key")
    return result.stdout


def public_key(key):
    der = openssl("pkey", "-in", key, "-pubout", "-outform", "DER")
    if len(der) != 44 or der[:12] != bytes.fromhex("302a300506032b6570032100"):
        raise ValueError("The release signing key must be Ed25519")
    return der[12:].hex()


def build_manifest(directory, version, commit):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version):
        raise ValueError("Invalid release version")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("Invalid release commit")
    assets = []
    names = set()
    for architecture in ("x86_64", "aarch64"):
        metadata = json.loads((directory / f"release-{architecture}.json").read_text())
        if metadata["version"] != version or metadata["architecture"] != architecture:
            raise ValueError("Architecture metadata does not match this release")
        formats = set()
        for asset in metadata["assets"]:
            name = asset["name"]
            kind = asset["format"]
            if (not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,199}", name)
                    or kind not in SUFFIXES or not name.endswith(SUFFIXES[kind])
                    or name in names or kind in formats
                    or asset["architecture"] != architecture
                    or not re.fullmatch(r"[0-9]+(?:\.[0-9]+){1,3}", asset["glibc_minimum"])):
                raise ValueError("Invalid or duplicate update package")
            path = directory / name
            if path.is_symlink() or not path.is_file():
                raise ValueError("Update package must be a regular file")
            size = path.stat().st_size
            with path.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            if not 0 < size <= 2 * 1024**3 or size != asset["size"] or digest != asset["sha256"]:
                raise ValueError("Update package does not match its build manifest")
            names.add(name)
            formats.add(kind)
            assets.append({field: asset[field] for field in
                           ("name", "architecture", "format", "size", "sha256", "glibc_minimum")})
        if formats != SUFFIXES.keys():
            raise ValueError("Each architecture must include all four package formats")
    return {"schema": 1, "repository": REPOSITORY, "version": version, "tag": f"v{version}",
            "commit": commit, "assets": sorted(assets, key=lambda asset: asset["name"])}


def create(directory, version, commit, private_pem, expected_public):
    if not private_pem or not re.fullmatch(r"[0-9a-fA-F]{64}", expected_public):
        raise ValueError("Configure COMMANDER_UPDATE_PRIVATE_KEY (secret) and COMMANDER_UPDATE_PUBLIC_KEY (variable)")
    payload = (json.dumps(build_manifest(directory, version, commit), indent=2) + "\n").encode()
    with tempfile.TemporaryDirectory(prefix="commander-sign-") as temporary:
        private = Path(temporary) / "private.pem"
        descriptor = os.open(private, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(descriptor, "w") as output:
            output.write(private_pem)
        if public_key(private) != expected_public.lower():
            raise ValueError("The signing key does not match the public key embedded in release builds")
        message = Path(temporary) / "update.json"
        message.write_bytes(payload)
        signature = openssl("pkeyutl", "-sign", "-rawin", "-inkey", private, "-in", message)
    if len(signature) != 64:
        raise ValueError("Invalid Ed25519 signature length")
    (directory / "update.json").write_bytes(payload)
    (directory / "update.json.sig").write_bytes(signature)
    with (directory / "SHA256SUMS").open("w") as checksums:
        for asset in sorted(directory.iterdir()):
            if asset.name == "SHA256SUMS":
                continue
            if asset.is_symlink() or not asset.is_file():
                raise ValueError("Release assets must be regular files")
            with asset.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            checksums.write(f"{digest}  {asset.name}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    public = commands.add_parser("public-key", help="Print the public key for the repository variable")
    public.add_argument("key", type=Path)
    sign = commands.add_parser("create", help="Sign release-assets using the environment's signing key")
    sign.add_argument("directory", type=Path)
    sign.add_argument("--version", required=True)
    sign.add_argument("--commit", required=True)
    args = parser.parse_args()
    try:
        if args.command == "public-key":
            print(public_key(args.key))
        else:
            create(args.directory, args.version, args.commit,
                   os.environ.get("COMMANDER_UPDATE_PRIVATE_KEY", ""),
                   os.environ.get("COMMANDER_UPDATE_PUBLIC_KEY", ""))
    except (ValueError, KeyError, OSError) as error:
        parser.exit(1, f"Release signing failed: {error}\n")


if __name__ == "__main__":
    main()

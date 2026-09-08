#!/usr/bin/env python3
"""Export the shared launcher PNGs from Commander's vector icon master."""

from pathlib import Path
import shutil
import subprocess


def main():
    root = Path(__file__).resolve().parent.parent
    branding = root / "crates/app/assets/branding"
    master = branding / "commander.svg"
    magick = shutil.which("magick")
    if magick is None:
        raise SystemExit("Install ImageMagick to export the app icon (magick).")
    # Render each size directly from the vector source for clean capsule edges.
    for size, destination in (
        (1024, branding / "commander-icon.png"),
        (512, root / "packaging/org.example.Dualpane.png"),
    ):
        subprocess.run(
            [magick, "-background", "none", "-density", str(size * 96 // 128),
             str(master), "-resize", f"{size}x{size}", str(destination)],
            check=True,
        )


if __name__ == "__main__":
    main()

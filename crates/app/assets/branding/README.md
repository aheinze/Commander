# Commander app icon

`commander-icon-transparent.png` is the user's selected app icon: the smooth
blue and ice split face with a navy commander's cap, gold trim, and double
chevrons. It has a real alpha channel, so the surrounding area blends into the
desktop instead of displaying a pale square. The generation brief is recorded
in `commander-icon.prompt.md`.

`commander-icon.png` is the 1024 × 1024 export.
`packaging/org.example.Dualpane.png` is the 512 × 512 launcher export used by
AppImage and other raster consumers. `commander.svg` embeds the 1024-pixel PNG
as a self-contained SVG for the existing GTK resource, About page, and Linux
scalable-icon paths. It contains raster artwork, not editable vector shapes.

Run `python3 scripts/export-app-icon.py` from the repository to regenerate all
three exports from `commander-icon-transparent.png` (requires ImageMagick). Each
PNG is resized directly from the source with Lanczos scaling, preserving alpha
transparency. Repeated exports do not degrade the image.

`commander-icon-source.png` preserves the original smooth cap icon before
background removal. `commander-icon-8bit-bold.png`, `commander-logo*.png`, and
`commander-mark-source.png` preserve previous concepts and are not used by the app.

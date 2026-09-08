# Commander app icon

`commander.svg` is the editable vector master of the user's chosen mark: three
left-aligned black capsules, with matching long top and bottom bars and a short
middle bar. The bars preserve the supplied mark's proportions and spacing. A
white rounded-square background keeps the black mark visible in both themes.

`commander-mark-source.png` preserves the user's original image unchanged.
`commander-icon.png` is a 1024 × 1024 raster export of the vector master.
`packaging/org.example.Dualpane.png` is the 512 × 512 launcher export used by
AppImage and other raster consumers. Run `python3 scripts/export-app-icon.py`
from the repository to regenerate both assets (requires ImageMagick).

The older `commander-logo*.png` files and `commander-icon.prompt.md` describe
previous concepts. The current icon is drawn directly from the supplied mark;
it does not use generated artwork.

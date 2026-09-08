# Commander app icon

`commander.svg` is the current, original vector artwork: a slate-blue folder with
two panels representing dual-pane browsing. Its transparent 128 × 128 canvas
includes the padding used by desktop launchers. Keep that padding when exporting.

The SVG is embedded in the app and installed into the scalable hicolor icon theme
by Linux packages. `packaging/org.example.Dualpane.png` is its 512 × 512 export,
used by AppImage and launchers that require a raster icon. Regenerate that PNG
from the SVG when changing the artwork.

The older `commander-logo*.png` files are previous concepts.

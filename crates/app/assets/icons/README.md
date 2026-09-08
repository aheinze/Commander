# Bundled Commander icons

Selected Lucide 1.42.0 SVGs, pinned to commit `3859eb20fabe7fd95652fcd4395843b6c0bcdd01`.
Source: https://github.com/lucide-icons/lucide/tree/3859eb20fabe7fd95652fcd4395843b6c0bcdd01/icons

The original 24 × 24 geometry, 2 px stroke, and rounded caps/joins are preserved.
Strokes are expanded into filled paths using `normalize.py` for compatibility with
GTK 4.14's symbolic loader and newer GTK versions' native SVG parser. GTK applies
the control/file color at render time and caches scaled icons. Path coordinates
are rounded to four decimal places; no raster images are used.

The adjacent LICENSE contains Lucide's ISC license and the MIT notice for icons
derived from Feather. It is also embedded in the executable's resource bundle.

`eject.svg`, `scissors.svg`, and `clipboard.svg` are original Commander artwork
matching the same size and line weight.

To add an icon, download its original SVG from the pinned source, run
`python normalize.py original.svg new-icon.svg`, and add a namespaced
`commander-…-symbolic` alias under `scalable/actions/` in `../icons.gresource.xml`.
The converter uses `fonttools==4.64.0` and `skia-pathops==0.9.2`; these are only
needed when changing assets. Normal app builds use the checked-in paths. No
runtime downloads or desktop icon-theme changes are involved.

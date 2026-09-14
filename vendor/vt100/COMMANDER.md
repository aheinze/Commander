# Commander terminal patch

This directory vendors the MIT-licensed `vt100` 0.16.2 crate, from
https://github.com/doy/vt100-rust at commit
`eb66ffaf7d771c13303ef73b29f6f2a56fdacecf`. The workspace pins it with
`[patch.crates-io]`. Upstream source and its license are retained.

Upstream cells contain one spacing scalar followed by zero-width marks. That
splits skin-tone emoji, flags and ZWJ sequences, and gives variation selectors
and keycaps the wrong width. Commander makes these bounded changes:

- `screen.rs` extends consecutive printed graphemes using `unicode-segmentation`
  and computes their column width using `unicode-width`. Width changes reuse the
  existing overwrite and wrap logic, including growth at the right margin.
- `perform.rs` ends grapheme extension on control/escape sequences. Resizing
  also ends extension; separate PTY reads do not.
- `cell.rs` stores complete graphemes in a fixed 54-byte buffer, large enough for
  standard emoji sequences, while bounding pathological combining-mark input.
  Cells occupy 64 bytes instead of upstream's 32 bytes; at 120 columns the
  10,000-row scrollback limit therefore uses at most about 77 MB for cell data.
- The manifest adds `unicode-segmentation` and uses `unicode-width` 0.2.2.

Commander regression tests live in `crates/app/src/app/terminal_view.rs` and run
with `cargo test -p dualpane-app terminal_view::tests`. The ignored rendering test
runs through `scripts/test-native.py --backend wayland --filter terminal` and
requires a color emoji font. It checks actual Pango/Cairo output, not just UTF-8
preservation. Keep these tests when rebasing or removing the local patch.

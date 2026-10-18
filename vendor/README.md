# vendor/crossterm

crossterm 0.28.1, unmodified except `src/event/sys/unix/parse.rs` (search `DCT PATCH`)
and `Cargo.toml` (examples removed). Wired in by `[patch.crates-io]` in the root Cargo.toml.

Why: upstream parses any `ESC [ <digits> R` as a cursor-position reply and `event::read`
filters it out, so xterm-style Shift/Alt/Ctrl+F3 (`ESC[1;2R`, `ESC[1;3R`, `ESC[1;5R`...)
never arrive as key events. The patch maps exactly `ESC [ 1 ; <2..8> R` to F3 with those
modifiers. Trade-off: a real cursor reply for row 1, column 2..8 would be read as a key.
dct never asks the terminal for the cursor position (`cursor::position`) while the key loop
runs. Drop this directory when upstream fixes it.

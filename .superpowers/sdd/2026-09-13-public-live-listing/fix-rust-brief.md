# Fix wave (rust): paste, daemon restart race, small guards
Worktree /Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live. A parallel agent edits container/classroom/* in the same worktree — commit only your Rust paths (`git commit -m ... -- <paths>`); if git reports index.lock, wait and retry. Never git stash. English commit messages, no AI attribution/Co-Authored-By lines. Reads ≤60 lines per call (grep -n first; files are large). Do NOT run `cargo test --workspace` or workspace clippy (25 min; the controller runs them). Use focused: `cargo test --lib ui::` filters, `cargo test --lib live::`, `cargo test --lib daemon::`, `cargo test -p dct-srv`, and `cargo clippy --lib --tests -- -D warnings` / `cargo clippy -p dct-srv --all-targets -- -D warnings`. Run cargo with PATH including ~/.cargo/bin. Format only code you touch (repo isn't fmt-clean; never run cargo fmt on the whole tree).

## I3 — paste into the live panel input
src/ui/mod.rs ~1093: `if let Event::Paste(text) = ev { match &mut app.view { ... _ => {} } }`. Bracketed paste is on, so pasted text never arrives as key chars. Add an arm for `View::Live { input: Some(inp), .. }` (check actual shape in src/ui/view.rs LiveInput{Title{..}, Key{buf, then_publish}}): Key → append the pasted text cleaned the same way EnterSecret paste does (grep `clean_secret`); Title → append with newlines/control chars removed. Respect whatever length caps the keypress path enforces (grep edit_live_input in src/ui/live.rs). Test: pasting a 64-hex key into Key input fills buf; pasting "a\nb" into Title gives no newline.

## M1 — daemon marks a valid key "stuck" after a relay restart race
src/live.rs pusher ~620-840: a 401 from put_public sets `public_stuck` (≈line 742). But the relay also answers 401 when it doesn't have the room (crates/dct-srv/src/live.rs:313). On a PUT 401, probe with fetch_lanes (~1011, uses the room's viewer token): if lanes is also 401 → the room is gone: clear `started` (same as the existing push/lanes 401 handling) and do NOT set public_stuck, so the re-registered room is published on the next loop. Only if lanes succeeds is it a real key rejection → stuck as today. Test with the existing fake-relay test harness in src/live.rs tests (grep how the 401-clears-started test is written).

## M3 — daemon refuses an empty title
src/daemon.rs live_publish ~1067: after trim+truncate, if empty → return an error Response without touching live state. Reuse an existing ErrorCode that fits (grep ErrorCode enum in src/proto.rs; e.g. a BadRequest/InvalidArgument). Do NOT bump PROTOCOL_VERSION or add a new variant unless none fits — if none fits, report BLOCKED on that item instead. Test.

## T2 — `dct-srv key add --file X` creates a key named "--file"
crates/dct-srv/src/lib.rs parse_cli ~824: name() takes args.get(2); reject names starting with "--" ("缺少名字"). Test in the existing parse_cli tests.

## T4 — misplaced doc comment
grep `LiveLanesResponse` in crates/dct-link and crates/dct-srv: a doc comment describing the lanes response sits on `PublicTitle` (or similar). Move it to the item it describes. No behavior change.

Commits: one or several, your choice. Report: .superpowers/sdd/2026-09-13-public-live-listing/fix-rust-report.md (≤40 lines): SHAs, each item done/blocked, focused test commands + results. Final message ≤10 lines.

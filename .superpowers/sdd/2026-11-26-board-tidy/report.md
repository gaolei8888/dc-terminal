# Report: board tidy (feat/board-tidy)

Commits: 786d81c (activity_noise + footer cleaning), dd6841e (one aligned row per
single-session project, ●/○/✕ status, idle duration, full-row selection), a4858fa
(tree connector attached to the id).

Controller checks: clippy 1.99 clean; `cargo test --lib ui::` 591 passed.
`an_unused_session_of_an_unresumable_profile_needs_no_handoff` fails intermittently
with OperationFailed(Checkpoint) — also 2/4 on unmodified main, so pre-existing.

Adopted from modern TUIs: one accent, dim secondary text, full-row selection, colored
status glyphs, right-aligned end marks, single grid. Skipped: rounded borders/section
titles (cost rows), new key-hint footer (dct already has one; `?` shows keys).
Noise filtering runs in the daemon, so it needs a restart.

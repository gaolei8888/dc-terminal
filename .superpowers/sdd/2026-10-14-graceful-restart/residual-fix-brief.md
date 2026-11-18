# Residual fix pass — graceful restart (BASE 477befc)

Inputs in this folder: final-rereview.md (N1–N8 with file:line), final-acceptance.md (codex real-machine re-run, 18 rounds), progress.md (rulings — binding; read the last ~10 lines).

## Fix
- **N1 (Important).** The new-daemon guard must trip on the first real restart: record that the instance question was asked; trip on any change including unknown→known (and an error→answer after reconnect). Add to `RestartOldDaemonPlain` (i18n.rs ~1159, zh+en) and both READMEs: close every open dct window before confirming, reopen afterwards. Test it (must fail without the fix).
- **N3** cap the wait for an idle unresumable session that never becomes type-ready at 60 s, then ask anyway (gated send still applies? no — mark it done and warn) — pick the safe one: mark done, record interrupted, warn.
- **N4** check "never submitted" before the profile check (session.rs ~1003) so an unused codex isn't asked for a handoff; test.
- **N5** cancel-note give-up warning must not say 「重启后」; own wording zh+en.
- **N6** a dropped held mesh message raises HeldMessageDropped.
- **N7** fix the two misplaced doc comments (ui/mod.rs ~2058-2066, ~4098-4101).
- **N8** READMEs: the first (plain) restart restores sessions automatically with new numbers (no question). Code: for an id-less record, keep `--continue` if its last_active is newer than every sibling's in the same (dir, profile) group; test.
- **Codex (ruling a–d in progress.md):**
  (a) a handoff request counts as answered only when the handoff file exists, or the deadline passes (then interrupted=true as now);
  (b) idle must hold ≥3 s continuously before a busy session counts as finished;
  (c) after restart, look for the handoff file when the note is due and keep checking until the note window ends, not once at startup;
  (d) re-measure codex 0.163 busy indicators by capturing the screen during a real turn, both default and with `--no-alt-screen` (another branch will add that flag to the codex profile), and widen `busy_pattern` in profiles/codex.toml so it holds through a whole turn in both modes (candidates seen: "tab to queue message", the spinner line, "Working"); dated comment with what was measured. Test the regex against captured text fixtures.
- Then re-run the isolated real check from final-acceptance.md for 3 rounds (codex + `dct restart -y --wait 1 --nudge-wait 1`): handoff file written before swap, note delivered and submitted after. Report per round.

## Park (do not fix)
N2 (drain lock held while typing), Enter lost on ordinary first send to a fresh codex (outside this feature — just record evidence in the report).

## Constraints
Same as final-fix-brief.md: English commit messages, no AI attribution; cargo test green; `cargo +1.99.0 clippy --all-targets -- -D warnings` clean; NO `cargo fmt`; generic code (no profile-name branching); never touch the real daemon, ~/.dct, ~/.local/bin; real checks with a short temp HOME, `env -u DCT_SESSION_ID`, private `tmux -L` server; codex trust prompt writes to the real ~/.codex/config.toml — back it up and restore it byte-identically; tests must fail without their fix.

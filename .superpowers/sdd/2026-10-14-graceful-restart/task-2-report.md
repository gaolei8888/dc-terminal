# Task 2 report: record conversation ids, resume each session into its own conversation

Status: DONE. Commit `ae1b857` feat(session): record each agent conversation id and resume every session into its own conversation after a daemon restart

## What was done

- `src/conversation_id.rs` (new, `pub mod conversation_id;` in `src/lib.rs`): `new()` returns a lowercase UUID v4 (36 chars) from `getrandom` (already a dependency), timestamp fallback if the OS RNG fails.
- `src/last_sessions.rs`:
  - `RecordedSession` gained `conversation_id: Option<String>`, `interrupted: bool`, `handoff: Option<PathBuf>`, all `#[serde(default)]`.
  - The struct doc that said "we do not record the agent's conversation id" was rewritten: since 2026-10-14 we do, because with several sessions in one dir only one could get its conversation back; points at the spec section 1.
  - New `continue_flags(&[RecordedSession]) -> Vec<bool>`: records with an id get `false` and are kept out of the `--continue` grouping; the rest go through `group_for_resume` and are mapped back by index.
- `src/session.rs`:
  - `struct Session` gained `conversation_id: Option<String>`.
  - `create_inner` / `create_resuming` gained `conversation: Option<&str>` (`create()` passes `None`). Command building:
    - conversation given and `profile.resumes_by_id()` -> append `fill_id(resume_id_args, c)`, keep `c`;
    - otherwise, if continuing (`resume && !resume_args.is_empty()`) append `resume_args` and record no id;
    - otherwise, if `resumes_by_id()` mint `conversation_id::new()` and append `fill_id(session_id_args, id)`.
  - `conversation_of(id) -> Option<String>` (pub).
  - `persist_last_sessions_with(&HashSet<u32>, &HashMap<u32, PathBuf>)` (pub) writes the three new fields; `persist_last_sessions()` now delegates with empty sets.
- `src/daemon.rs::restore_last_sessions`: uses `continue_flags` instead of `group_for_resume` and passes `record.conversation_id.as_deref()` to `create_resuming`. Doc comment updated.
- `src/main.rs::offer_to_resume_last_sessions`: did use `group_for_resume`; switched to `continue_flags`. The display marker now shows "will continue" when `resume || rec.conversation_id.is_some()` (otherwise records with ids would wrongly say "start fresh").
- All `create_resuming` call sites updated (3 existing tests in session.rs; none in tests/). `RecordedSession` literals updated in session.rs, daemon.rs test helper `rec`, last_sessions.rs test helper `rec`.

## Tests added

- conversation_id: `looks_like_uuid_v4_and_differs` (also checks hyphen positions 8/13/18/23).
- last_sessions: `old_file_without_new_fields_still_loads`, `new_fields_round_trip`, `continue_flags_skips_records_with_ids` (the id record has the LARGEST last_active, so plain `group_for_resume` would give the wrong answer — the test actually discriminates).
- session: `new_session_gets_a_conversation_id_and_passes_it`, `resuming_with_a_conversation_uses_resume_id_args_and_keeps_the_id` (also asserts no `--session-id` / `--continue`), `resuming_without_a_conversation_continues_and_has_no_id` (extra: pins the "never --continue together with --session-id" rule), `profile_without_id_args_has_no_conversation` (also: an id passed to a profile without id args is ignored), `persisted_record_carries_conversation_and_flags`.
- daemon: `restore_prefers_conversation_id_over_continue_grouping`, `restore_without_conversation_id_falls_back_to_continue_grouping`.
- Probe profile `echo-id`: `command = ["/bin/sh", "-c", "echo \"ARGS $*\"; sleep 30", "sh"]` via `sys::testing::toml_with_sh`, `is_agent = false`, `resume_args = ["--continue"]`, `session_id_args = ["--session-id","{id}"]`, `resume_id_args = ["--resume","{id}"]`. Reused existing `wait_for_screen` helper in session.rs; added small `wait_for_args` in daemon tests.

## TDD evidence

RED (`cargo test --lib conversation_id`, tests written, no implementation):
```
error: could not compile `dct` (lib test) due to 40 previous errors
   6 error[E0061]: this method takes 6 arguments but 7 arguments were supplied
   1 error[E0425]: cannot find function `continue_flags` in this scope
   2 error[E0425]: cannot find function `new` in this scope
   3 error[E0560]: struct `last_sessions::RecordedSession` has no field named `conversation_id` (also handoff, interrupted)
   9 error[E0599]: no method named `conversation_of` found for struct `session::SessionManager`
   1 error[E0599]: no method named `persist_last_sessions_with` found ...
```

GREEN (focused):
```
cargo test --lib -- conversation_id last_sessions:: session::tests::new_session session::tests::resuming session::tests::profile_without session::tests::persisted session::tests::create_resuming daemon::tests::restore
test result: ok. 24 passed; 0 failed
```

Full: `cargo test` -> lib `1821 passed; 0 failed; 1 ignored`, every integration binary `ok`, no FAILED.
Clippy: `cargo +1.99.0 clippy --all-targets -- -D warnings` -> clean (`Finished`).

## Deviations from the plan

1. Plan: `if resume { extend resume_args }` then `if resumes_by_id() && !resume { new id }`. I used `continuing = resume && !profile.resume_args.is_empty()` for both. Reason: a profile that declares id args but no `resume_args` would otherwise, on a `resume = true` restore, get neither `--continue` nor a new id. With the change it gets a fresh id. Identical behaviour for every profile that has `resume_args` (all claude-based ones).
2. `create_resuming` / `create_inner` now have 8 params incl. self, so `#[allow(clippy::too_many_arguments)]` was added to both.
3. main.rs marker also treats `conversation_id.is_some()` as "continue" (plan only said switch to `continue_flags`; without this the prompt would mislabel id-carrying records as "start fresh").
4. Formatting: the touched files were NOT rustfmt-clean at HEAD (session.rs/daemon.rs have pre-existing diffs). I applied rustfmt output only to hunks overlapping my own changes; pre-existing unformatted code untouched. conversation_id.rs and last_sessions.rs are fully rustfmt-clean.

## Self-review

- No branching on profile names; everything driven by `resumes_by_id()` / `resume_args`.
- Tests never touch `~/.dct`: last-sessions path comes from `last_sessions_path_for_socket(tmp/daemon.sock)`.
- No real daemon restarted, nothing installed.
- `persist_last_sessions_with` takes the same lock order as before (snapshot Arcs, then lock each), so no new deadlock risk; it is pub so Task 6 can call it.
- Old `last-sessions.toml` files without the new fields load fine (tested).

## Concerns

- The "continue" path for old records still gives that one session no id this time round (by design, claude rejects `--continue` + `--session-id`); it gets an id only after the next fresh session. Matches the spec.
- `interrupted` / `handoff` are only written, nothing reads them yet (Task 6+).
- `create_resuming`'s doc still mentions `group_for_resume` as the decider in one historical sentence; harmless but slightly stale.

---

# Fix round 1

Commit: see `git log` on feat/graceful-restart. Subject: "fix(restore): record a conversation id only after a submitted turn, restart sessions whose conversation is gone with a warning, and keep id records in --continue grouping when their profile no longer resumes by id"

## 1a. Prevention: only persist an id after a submitted turn
- New `Session.submitted: bool`. Set in `send_input`'s empty-text branch (the Enter path; verified the TUI grid/attach, web `Key Enter` -> `key_to_input` -> `""` and bridge all submit via `send_input(id, "")`). Starts `true` for a session resumed by id (its conversation already exists), `false` otherwise.
- `persist_last_sessions_with` writes `conversation_id` only when `submitted`; else `None`, so restore starts it fresh.
- Not chosen: `checkpoints.len() > 1` (only agent sessions take checkpoints, and they dedupe identical SHAs). Pastes/typed text containing newlines in the non-empty branch do NOT count — claude treats pasted text as text, not a submit; worst case is a missed id, never a wrong one.
- Tests: `unsubmitted_session_records_no_conversation_id` (typing without Enter -> None; after Enter -> Some), `a_session_resumed_by_id_records_its_id_without_new_input`. Existing `persisted_record_carries_conversation_and_flags` now submits an Enter first.

## 1b. Fallback: resumed-by-id session that exits right away
- New `SessionResumeSkipReason::ConversationGone` (proto.rs) with i18n: zh 「原来的对话找不到了，已经在原处重新开了一个」, en "its earlier conversation could not be found, so a fresh one was started in its place", rendered inside the existing "没能接回 {dir} 的「{profile}」会话：…。" sentence. Added to the i18n all-warnings test (no newline, no Han in English).
- daemon.rs: `restore_last_sessions` split into `restore_sessions` -> `(skips, Vec<ResumedById>)` (`restore_last_sessions` kept as a `#[cfg(test)]` wrapper so existing tests are unchanged). New `watch_resumed_by_id(mgr: Arc<_>, all, pending, window) -> Option<JoinHandle>`: background thread "resume-watch", polls every 100ms up to `window` (production `RESUME_WATCH_WINDOW = 20s`, injectable as a param). If a session `exited_on_its_own`, it `discard`s the dead slot, recreates fresh in the same dir/profile (new id via `session_id_args`, recorded tag kept), and `push_resume_skip`s `ConversationGone` (or `StartFailed` if recreation fails). Started after `set_resume_skips` in `run_with_manager` so its warnings append rather than get overwritten; daemon startup does not wait for it.
- session.rs additions: `Session.stopped_by_request` (set by `stop()`/`kill()`), `exited_on_its_own(id) -> Option<bool>` (process gone or Stopped, and not stopped on request; read-only, tick still reaps), `discard(id)` (kill + remove from the map + persist), `push_resume_skip(w)`.
- Detection is "process exited", not the `Failed` screen state: a Failed session may still be alive and usable, and claude exits on "no conversation found".
- Tests: `a_session_whose_conversation_is_gone_is_restarted_fresh_and_reported` (probe exits immediately on `--resume`; after the watcher: exactly one session, tag kept, new id != old, screen shows `--session-id <new>`, one ConversationGone warning), `a_session_resumed_by_id_that_stays_up_is_left_alone`, `a_resumed_session_stopped_on_purpose_is_not_restarted`.

## 2. continue_flags respects the profile
- `last_sessions::will_resume_by_id(rec, profiles)` = has id AND its profile is known and `resumes_by_id()`. `continue_flags(sessions, profiles)` now filters on that. `restore_sessions` and `main.rs::offer_to_resume_last_sessions` both pass the profile list; the main.rs "will continue" label uses `resume || will_resume_by_id(rec, &all)`. The by-id watch list uses the same predicate.
- Test: `an_id_whose_profile_no_longer_resumes_by_id_joins_continue_grouping` (also: unknown profile -> not by id).
- Stale `create_resuming` doc sentence now names `continue_flags`.

## Evidence
- Mutation (RED) check — reverting each fix individually: persist ignoring `submitted`, watcher disabled, `continue_flags` filtering only on `conversation_id.is_none()`:
  `test result: FAILED. 0 passed; 3 failed` (unsubmitted_..., a_session_whose_conversation_is_gone_..., an_id_whose_profile_...). Ignoring `stopped_by_request`: `a_resumed_session_stopped_on_purpose_is_not_restarted` FAILED.
- GREEN: focused set `48 passed; 0 failed`; full `cargo test`: lib `1827 passed; 0 failed; 1 ignored`, all other binaries ok; after the one clippy fix, `session::` + `daemon::tests` re-run `149 passed; 0 failed`.
- `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean (one `redundant_pattern_matching` fixed).
- rustfmt applied only to hunks I touched; remaining rustfmt diffs in these files are pre-existing.

## Concerns
- New `SessionResumeSkipReason` variant changes the wire format of a Response. I did NOT bump `PROTOCOL_VERSION` here because the plan bumps 24 -> 25 later on this same branch (Drain requests) and both changes ship together; if this branch were split, this variant alone would need that bump.
- The watcher calls `create_resuming` from a background thread while the daemon is serving requests; this is the same path as a normal `create` so locking is unchanged, but the replacement session gets a new session number (the dead one is removed).
- A resumed session that dies after the 20s window is not replaced (by design: the user was using it).

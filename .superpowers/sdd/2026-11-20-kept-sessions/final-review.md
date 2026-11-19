# Final whole-branch review: kept sessions (23318f1..b73285f)

Reviewer scope: whole branch, extra attention on `src/daemon.rs` (Task 3 had no separate review).
Ran: `cargo test --lib keep` → 113 passed, 0 failed (includes `daemon::keep_tests::*` and `keep::watch_tests::*`).

## Verdict: needs fixes (small)

One change was already ruled on (the backoff off-by-one, finding 1). Finding 2 is a cheap should-fix. Everything else is minor.

## Findings

### 1. should-fix (ruled): the first retry after a quick death waits 10 s, but the spec says 5 s
- `src/keep.rs:184-187`
  ```rust
  fn fail(&mut self, now: Instant) {
      self.failures += 1;
      self.next_try = Some(now + backoff(self.failures));   // backoff(1) = 10 s
  }
  ```
  The code bumps `failures` first and then computes the wait, so quick deaths in a row wait 10, 20, 40 … 300 s. The spec (§3: 「第 1 次 5 秒，之后每次翻倍，最长 5 分钟」) and plan Review Focus #5 both say 5, 10, 20 … 300. The tests lock in the current behavior.
- **Exact change:** compute the wait before bumping the count:
  ```rust
  fn fail(&mut self, now: Instant) {
      self.next_try = Some(now + backoff(self.failures));   // 0→5 s, 1→10 s, 2→20 s … 6→cap 300 s
      self.failures += 1;
  }
  ```
  Leave `backoff()` as it is. Change its doc comment (keep.rs:140) to 「已经连续失败 `failures` 次时，这次要等多久」. The healthy path at keep.rs:268 (`backoff(w.failures)` with failures = 0, which is 5 s) stays as it is.
- Tests to update:
  - keep.rs:663: `[10,20,40,80,160,300,300,300]` → `[5,10,20,40,80,160,300,300]`
  - keep.rs:688: `[10,20,40]` → `[5,10,20]`
  - keep.rs:907/908/911 (`a_failed_open_warns_once_and_backs_off`): opens at 0, 5, 15, then success at 35. The asserts become `next_open_from(1) == 5`, `next_open_from(6) == 15`, `next_open_from(16) == 35`.
  - The `CRASH_WARN_AFTER` tests count failures, not seconds, so they should still pass. Re-run them anyway.
- Docs: README.md:516 should say "5 seconds, then 10, 20 …". README.zh-CN.md:429 should say 「5 秒、10 秒、20 秒……」.
- Side effect to note: a session that was healthy for a long time and then starts crash-looping now waits 5 (a normal reopen, not counted as a failure), then 5, 10, 20 … instead of 5, 10, 20 …. That fits the spec's reading of "a failure = it died within 60 s of a reopen". The big user-visible gain: a session you stop by hand within its first minute comes back in 5–10 s instead of 10–15 s.

### 2. should-fix: warnings pile up in `resume_skips` and are never cleared
- `src/daemon.rs` (`ManagerRoster::warn`, and `keep_round`'s `mgr.push_resume_skip`) → `src/session.rs:1340` `push_resume_skip` always appends.
- `resume_skips` is never cleared while the daemon runs. It is attached to every `Request::Profiles` (daemon.rs:1960), so the profile picker shows the whole list every time. The watcher only avoids saying the same thing twice while a problem lasts. When a problem goes away and comes back, it appends again. That happens with `NameClash` when the other session comes and goes, `CrashLooping` after each healthy minute, `StartFailed`/`DirGone`/`ProfileGone` after each recovery, and a config entry broken, fixed, then broken again. So the picker keeps showing warnings for problems already fixed, the same sentence can repeat, and the list grows without limit over a long daemon life. The README's "dct shows a warning once, not over and over" is only half true.
- Fix (the cheapest): add `SessionManager::push_resume_skip_once(w)` that skips a warning already in the list (`WarningCode: PartialEq` already exists, since `said.contains` uses it). Use it from `ManagerRoster::warn` and `keep_round`. That limits the growth and drops the repeats. Clearing a warning once its problem is fixed is out of scope for v1.

### 3. minor: right after a graceful restart, a kept session can be opened twice
- `src/daemon.rs:193-199` versus `src/keep.rs` `step_one`, the never-seen path.
- `set_restoring(false)` happens before `watch_resumed_by_id` starts, so the keep watcher is already running during resume-watch's 20 s window. Suppose the restored kept session dies at once (its conversation file is gone) **before** the watcher's first round has seen it alive (so `seen == false`). If that round also falls in the gap between the dead slot leaving the list (pruned or `discard`ed) and resume-watch's `create_resuming` putting the replacement in, the watcher opens a new one right away. Resume-watch then opens its own, and you get two sessions both named 「家教」, so `dct send` reports ambiguity. If the watcher had already seen it (`seen == true`), it waits 5 s and then finds the replacement, so that case is fine.
- The gap is small: the slot is inserted right after the PTY spawns, before the checkpoint (session.rs:~1560). So this is unlikely but possible.
- Fix options:
  - (a) A `resume_watching` flag on `SessionManager`, set while `watch_resumed_by_id` runs, and `ManagerRoster::paused()` also returns it. The cost is that a kept session missing from the restore waits up to 20 s more.
  - (b) In `step_one`, treat a first-ever miss like a death that was just seen: set `next_try = now + RESTART_BASE` and do not open in that round. The cost is that a cold start opens 5 s later.
  - Both are cheap. (a) is the more precise one.
- Related, and OK as it is: when a kept session's by-id resume fails, resume-watch replaces it with a **new number** (on purpose, see its comment). So in that case "same number" does not hold. Worth one line in the README, or just accept it.

### 4. minor: README says "back within about 5 seconds"
- README.md:514, README.zh-CN.md:428. It takes 0–5 s for the next round to notice, plus a 5 s wait, so 5–10 s in practice. Before fix 1, a session stopped within its first minute takes 10–15 s. The Task 3 real-machine run saw about 7 s and about 15 s. Suggest "usually within about 10 seconds" / 「一般十秒内就回来」.

### 5. minor: a relative `dir` is taken relative to the daemon's working folder
- `src/keep.rs` `expand`: `dir = "tutor"` (no `~` or `/`) becomes `PathBuf::from("tutor")`. `is_dir()` and `restore` then use the daemon's current folder. That gives a confusing `DirGone` warning, or opens in the wrong folder.
- Fix: treat a non-absolute path after `~` expansion as `KeepEntryInvalid { MissingField("dir") }` (or add a new problem variant), or document "use a full path or `~/…`".

### 6. minor: the daemon has two pause checks, so one can be removed without any test failing
- `keep_round` returns early on `is_restoring() || is_draining()` (daemon.rs:1689), and `KeepWatch::step` checks `paused()` again. Removing `ManagerRoster::paused`'s body would not fail `daemon::keep_tests`. The unit test `nothing_is_opened_while_paused` covers `step`. Both checks are correct, and the early return in `keep_round` is needed for the warning ordering. No change needed; this note is only so nobody "simplifies" one away without knowing.

## Focus items verified (no finding)

1. **Locks and deadlocks.** The keep thread never holds a lock across a `SessionManager` call. `list()` takes `sessions` then each session lock, the same as everywhere else. `discard` releases `sessions` before the session lock and `persist_last_sessions`. The secrets guard in `open` (daemon.rs:1649) is a temporary inside the `let`, dropped before `restore`. No lock is held around `restore`. No new lock order is introduced.
2. **Thread never dies.** Each round runs under `catch_unwind`. Poisoned mutexes go through `recover`. If the spawn fails, it logs and returns `None`.
3. **Paused while restoring or draining.** Checked twice (see 6). `set_draining(true)` stays set from the start of the drain until the process exits (`finish` → `kill_all_now` → exit). Only `cancel` and `abandon` clear it, and both resume normal running. So no reopen happens after `kill_all_now`.
4. **Config warnings once.** `said` is updated only after the restoring/draining early return, so warnings from the restore phase are not wiped by `set_resume_skips` (tested in `nothing_is_opened_while_restoring`). A broken, fixed, then broken entry warns again (tested). See finding 2 for the list growing.
5. **Folder comparison.** `same_dir` canonicalizes both sides and falls back to the raw path. That handles macOS `/private/var` and `spawnable()`'s stored form.
6. **Stopped slot with `want_id`.** It is discarded before `restore`. A live session holding that number is left alone, and `restore` then hands out a new number. Liveness of the reused number after `recently_ended` is tested.
7. **Graceful restart.** `persist_last_sessions` writes the session's `name_slot` (= the trimmed keep name, set via `restore_tag`). The restore thread passes `record.tag` back as `restore_tag`, and `create_inner` trims it again, so the restored tag equals `k.name`. Profile and folder match (folders canonicalized). The watcher's first round after restore finds it, no duplicate (apart from the race in finding 3). If resume-watch replaces a failed resume, it keeps the tag, so the watcher picks up the replacement.
8. **Auto-naming.** `restore_tag` sets `name_attempted = true` (session.rs:1512-1519), so `tick()` never calls `request_name` for a kept session. `name_slot` is written nowhere else.
9. **Renaming the kept session (focus 7).** dct has no rename request: no `Request` changes a tag, and `name_slot` is written only by `request_name`. So "the user renames it in the board, then a second one opens" cannot happen today. If a rename feature is added later, the watcher would see the renamed session as gone and open a second one. Note it then; nothing to document now.
10. **Protocol 26 → 27.** `PROTOCOL_VERSION` and all eleven pinned shape tests in `src/proto.rs` were updated. Nothing in `tests/`, `container/` (`server.mjs` only checks error names, not the version), `crates/dct-link` (has its own number), or `src/web` pins 26. The new `WarningCode` variants have i18n in zh and en and are in the exhaustive render test.
11. **README.** Matches the behavior except for finding 1's numbers and finding 4's "about 5 seconds". The other bullets are accurate: open at start, same number and name, conversation resumed, deleting the entry leaves the running session alone, changes picked up within 5 s, agent folder must be a git repo, folder not created, and each problem reported once.

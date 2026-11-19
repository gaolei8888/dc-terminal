# Task 3 report: wire the keep watcher into the daemon + docs

Status: DONE
Branch: feat/kept-sessions (worktree /Users/lei/work/dc/dc-terminal/.worktrees/keep)

## Commits

- `99d0b3a feat(daemon): keep [[keep]] sessions running`
- `b73285f docs: kept sessions`

No AI attribution lines. `cargo fmt` not run.

## Changes

### src/daemon.rs

- `KEEP_TICK = 5 s`.
- `KeepClock { tick, now }`, so the clock can be swapped in tests. `KeepClock::real()` uses `KEEP_TICK` and `Instant::now`. The test clock moves forward one full round (5 s) every time it is read, with a 20 ms real sleep. The real thread then runs through the real 10 s backoff in about 2 real ticks, and the "lived < 60 s" timing still holds. Shortening only the tick would have changed those windows.
- `ManagerRoster<'a> { mgr, secrets, profiles }` implements `keep::Roster`. A new one is built every round with that round's `all_profiles`.
  - `paused`: `is_restoring() || is_draining()`.
  - `find` / `name_taken_elsewhere`: go through `mgr.list()` and skip `Stopped`. A match needs the same tag, the same profile, and `same_dir`. `same_dir` canonicalizes both paths and falls back to the path as written if that fails.
  - `conversation_of`: calls `mgr.conversation_of`.
  - `open`: if `want_id` is in the roster as `Stopped`, it calls `mgr.discard(id)` first. A live session holding that number is left alone, and `restore` hands out a new number. The secret is looked up under a short `recover(secrets.lock()).get(profile)` lock, the same way the restore thread's `secret_of` does it. Then it calls `mgr.restore(dir, profile, secret, profiles, false, Some(name), conversation, want_id)`. On error it logs with `eprintln` and returns `e.to_string()`.
  - `warn`: calls `mgr.push_resume_skip`.
- `keep_round(...)`: does nothing at all while restoring or draining, not even config warnings. When the restore thread finishes, `set_resume_skips` overwrites the whole warning list, so a warning pushed during the restore would be lost, and because `said` would already list it, it would never be pushed again. Otherwise the round:
  1. runs `keep::load(config_path_for_socket(socket), home)`,
  2. pushes only the config warnings that were not in the previous round's set (`said`). Once a fixed entry drops out of `said`, breaking it again warns again,
  3. runs `all_profiles(profiles_dir)` and ignores its warnings, which are already reported through `Request::Profiles`,
  4. calls `KeepWatch::step`.
- `spawn_keep_watcher(socket, mgr, secrets, clock, stop) -> Option<JoinHandle>`: a named thread `keep`. The first round runs right away, then one round per tick. Each round is wrapped in `catch_unwind`, so a panic is logged and the next round still runs. `stop` exists only so tests can end the thread. Production never sets it, and the thread does not keep the process alive.
- It is started in `run_with_hooks` right after the restore thread is set up, so `set_restoring(true)` is already in place. It starts with `KeepClock::real()`.
- **`~` in keep entries**: `crate::sys::home()`, the user's real home folder (from `HOME`, or `USERPROFILE` on Windows). The game-skill installer uses the same function. If no home folder can be found, it falls back to the folder two levels above the socket (`~/.dct/daemon.sock` → `~`). It is not `socket.parent()`: that is `~/.dct`, which is where config and profiles live, not the user's home.
- **recently_ended on a reused number**: checked, nothing to clear. `get_arc` (used by every request through `with_session`) and `liveness` both look in the roster first and only fall back to `recently_ended` when the number is not in the roster. So a live session on a reused number never gets `SessionEnded`. The test asserts `liveness(id) == Running` and `screen(id).is_ok()` after the reopen. The old entry stays in `recently_ended` until it expires. When the reused session ends through `prune`, `record` replaces that entry.

### README.md / README.zh-CN.md

A new subsection "Kept sessions" / 「常驻会话」 under "Sessions outlive the terminal" / 「会话活得比终端久」. It covers:

- the three-line `[[keep]]` example,
- that it opens when the background service starts,
- that it reopens with the same number, the same name, and the same conversation, usually within about 5 s,
- the growing wait between tries: 10 s, 20 s, 40 s … up to 5 min, which goes back to the start after one minute up,
- that deleting the entry stops the reopening but does not stop the running session,
- that config changes take effect within about 5 s,
- that an agent's folder must be a git repo and is not created for you,
- that warnings are shown once.

## Tests (src/daemon.rs `mod keep_tests`)

They use a temp socket dir, a profile `keepme` on disk (`sh -c 'sleep 1000'`, `is_agent = false`, so no git repo needed), and a `config.toml` with a `[[keep]]` entry.

1. `a_kept_session_is_opened_and_comes_back_with_the_same_number_after_stop`: opens with the right tag, profile and dir. After `stop(id)` the session leaves the roster, then comes back with the **same id**. Its `liveness` is Running, `screen` works, and there is exactly one.
2. `nothing_is_opened_while_draining`: nothing opens while draining. It opens once draining is turned off.
3. `nothing_is_opened_while_restoring`: nothing opens and no config warning is pushed while restoring. After restoring ends it opens and the warning arrives once.
4. `a_broken_entry_is_reported_once_not_every_round`: a bad 2nd entry does not stop the good one, is warned once across many rounds, and is warned again after being fixed and broken again.
5. `the_daemon_starts_the_watcher_on_its_own`: `run_with_manager` on a temp socket opens the kept session (first round, no wait).

Mutation checks (each one reverted afterwards):

| Change | Result |
|---|---|
| Don't start the thread in `run_with_hooks` | test 5 fails |
| `if true` instead of `!said.contains` | test 4 fails |
| Pass `None` instead of `want_id` | test 1 fails |
| Remove the `keep_round` pause guard | test 3 fails |
| Also remove draining from `Roster::paused` | tests 2 and 3 fail |
| Remove only one of the two pause checks | still passes, by design: each check backs up the other |
| Remove the `Stopped` filter in `live()` | **still passes**, see concerns |

Runs:

- `cargo test --lib keep_tests`, 3 times in a row: `ok. 5 passed` each time (about 1.0 s each).
- `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean.
- `cargo test --no-fail-fast`:
  - lib: 2121 passed, 5 failed. All 5 failures are in `session::tests`, a different set on each run. Run alone, `a_dead_model_leaves_the_first_line_as_the_name` passed on a later try. These are the known flaky tests, and none of them touch daemon.rs.
  - Every integration test binary: ok.

## Manual check (isolated)

Setup:

- HOME: `/private/tmp/claude-502/kp`
- `config.toml`:
  ```toml
  [[keep]]
  name = "kept"
  profile = "shell"
  dir = "~/w"
  ```
- daemon started with `env -u DCT_SESSION_ID HOME=$H ./target/debug/dct daemon` (built from the worktree), pid 84080. The real `~/.dct` was never touched.

Before any stop:

```
$ dct ps
1  shell  —  /private/tmp/claude-502/kp/w  lei@LeiMacBook-Air w %
```

Raw `List` over the socket (dct ps does not print names):

```
1 'kept' shell Unknown /private/tmp/claude-502/kp/w
```

Stop within the first minute (03:34:13):

```
Stopped session 1
ps after stop:
No sessions
back after ~15s at 03:34:28:
1  shell  —  /private/tmp/claude-502/kp/w  ...
1 'kept' shell Unknown /private/tmp/claude-502/kp/w
```

Same number and same name. It took about 15 s because it was opened less than 60 s earlier, so it counts as a quick death: 10 s backoff plus up to 5 s until the next round.

Stop after it had been up 70 s (03:35:42):

```
Stopped session 1
back after ~7s at 03:35:48:
1  shell  —  /private/tmp/claude-502/kp/w  ...
```

This is the 5 s path.

Emptied `config.toml`, waited 6 s, then `dct stop 1`:

```
1  shell  —  /private/tmp/claude-502/kp/w  ...
Stopped session 1
20s later:
No sessions
```

The running session was left alone and was not reopened once stopped.

The daemon was killed afterwards (`daemon gone`). Because this daemon used the default socket path inside the temp HOME, it also wrote the dco rules file there. That is harmless and stayed inside the temp HOME.

## Concerns

1. **The `Stopped` filter in `find` / `name_taken_elsewhere` has no test.** `stop` and `kill` prune at once, and `tick` prunes right after marking a session Stopped. So a Stopped slot can't be held in place long enough for a daemon-level test to see it, and the mutation survives. The filter follows the brief and does no harm.
2. **The first reopen after a manual stop within a session's first minute takes 10–15 s, not about 5 s.** This is how Task 2's `KeepWatch` is designed (any death within 60 s of our own open counts as a quick death: `fail()` → `backoff(1)` = 10 s). The spec's real-machine check says it should come back in 「5 秒左右」, and the brief's README wording says "5 s → 5 min". The README now says "usually about 5 seconds" and lists the growing waits as 10, 20, 40 s … 5 min, which matches what the code does. If the user expects the first retry at 5 s, `fail`/`backoff` in keep.rs would need to use `failures - 1`. That changes Task 2 tests, so it was not touched here.
3. **The keep thread runs in every daemon, including tests that call `run_with_manager`.** When there is no `[[keep]]`, it only reads `config.toml` every 5 s. No existing test writes `[[keep]]`, so existing tests are unaffected.
4. **The old `recently_ended` entry for a reused number stays until it expires** (10 min). No request sees it while the session is live. If the reused session were later removed with `discard` instead of being pruned, `liveness` would report the old way it ended. This is a narrow, harmless edge case.

## Final fix round (findings 1–5 of final-review.md)

Commits:

- `5e46c79 fix(keep): first retry after a quick death waits 5 s, then 10, 20 ... 300`
- `fe96502 fix(keep): list each kept-session warning once; pause while resumed sessions are watched`
- `dfb8e96 fix(keep): reject a [[keep]] dir that is not a full path or ~/...`
- `7b0c27a docs: kept sessions retry at 5 s, are back in 5-10 s, and need a full folder path`

### 1. Backoff 5, 10, 20 … 300 s

- `Watched::fail` now sets `next_try` from the current `failures`, then increments it.
- The `backoff` doc comment now reads 「已经连续失败 `failures` 次时，这次要等多久」.
- The tests were changed first and failed (3 of them), then the code fix made them pass:
  - waits `[5,10,20,40,80,160,300,300]`
  - waits `[5,10,20]`
  - failed opens retry at 5, 15, then 35
- The `CRASH_WARN_AFTER` tests pass unchanged.

### 2. Warnings pile up

- New `SessionManager::push_resume_skip_once`: it does not add a warning that is already in the list. `ManagerRoster::warn` and `keep_round` now use it. The restore thread still uses the plain `push_resume_skip`.
- New tests:
  - `session::tests::push_resume_skip_once_does_not_repeat_an_equal_warning`
  - `daemon::keep_tests::a_warning_that_comes_back_is_not_listed_twice` (break / fix / break / fix leaves exactly 1 entry)
- `a_broken_entry_is_reported_once_not_every_round` no longer expects a second entry after re-breaking the config.
- Mutation check: using a plain push in `keep_round` makes the new test fail.
- Gap: a plain push in `ManagerRoster::warn` is not caught at daemon level. Making a watcher warning come back for real needs a healthy minute or a name clash coming and going.

### 3. Race with the resume watch after a graceful restart

- New `SessionManager::{set_resume_watching, is_resume_watching}` (an `AtomicBool`).
- The restore thread raises the flag **before** `set_restoring(false)`.
- `watch_resumed_by_id` holds a drop guard (`Lower`) that lowers the flag on every way out: nothing to watch (`None`), the thread finishing, the spawn failing (the closure that owns the guard is dropped), or a panic.
- `ManagerRoster::paused()` also returns true while the flag is up. `keep_round`'s own early return is unchanged, so config warnings may still go out during the resume watch. That is safe: `set_resume_skips` has already run by then.
- New tests:
  - `daemon::keep_tests::nothing_is_opened_while_the_resume_watch_runs`
  - `daemon::tests::the_resume_watch_flag_is_lowered_when_the_watch_ends`: the `None` path, plus up during a real watch and down after `join`
  - `session::tests::the_resume_watch_flag_starts_off_and_follows_set`
- Mutation checks, each one failing the matching test:
  - drop the flag from `paused()`
  - never lower the flag
  - `mem::forget` the guard in the thread

### 4. README

- EN: "Usually it is back within 5–10 seconds."
- ZH: 「一般 5 到 10 秒就回来了。」
- The growing-wait line now says 5, 10, 20 … up to 5 min in both READMEs.

### 5. Relative `dir`

- New `KeepEntryProblem::DirNotAbsolute`.
  - zh: 「文件夹要写完整路径或 ~/ 开头」
  - en: "the folder must be a full path or start with ~/"
  - It was added to the exhaustive i18n render list.
- `parse` expands `~` first, then rejects any `dir` that is not `has_root()`.
- I used `has_root` instead of `is_absolute` on purpose. On Windows, `/w` has no drive letter, so it is not "absolute", but it still points at the root of the current drive, not the daemon's working folder. `has_root` keeps those entries (and the existing `/w/...` tests) working on Windows CI, and still rejects `tutor`, `./x`, `~tutor` and Windows `C:tutor`.
- Test: `keep::tests::a_relative_dir_is_rejected_and_the_rest_survive`.
- Both READMEs explain it (comment line in the example plus one bullet).
- The protocol stays at 27: the variant was added inside the same unreleased protocol bump.

### Results

- `cargo +1.99.0 clippy --all-targets -- -D warnings`: clean.
- `cargo test --lib -- keep resume_watch push_resume_skip_once`: 3 runs, 119 passed each time.
- `cargo test --no-fail-fast`:
  - lib: 2131 passed, 1 failed.
  - Every integration binary: ok.
- The one failure was `session::tests::whitespace_only_input_is_asked_about_exactly_once_not_forever`, with `OperationFailed(Checkpoint)`, which is a git snapshot failing. It fails or passes at random even when run alone (fail, pass, fail across 3 tries). It never touches the code changed here; it is the same family of pre-existing flaky `session::tests` failures.

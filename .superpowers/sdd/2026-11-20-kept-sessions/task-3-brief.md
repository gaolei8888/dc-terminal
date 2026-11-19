### Task 3: Wire into the daemon + docs

**Files:**
- Modify: `src/daemon.rs` (Roster impl over `SessionManager` + secrets; `keep` thread), `README.md`, `README.zh-CN.md`
- Test: daemon-level test (in `src/daemon.rs` tests or `tests/`), using a temp socket dir

**Interfaces — Consumes:** `keep::{load, KeepWatch, Roster}`.

- `Roster` impl: `paused` = `mgr.is_restoring() || mgr.is_draining()`; `find`/`name_taken_elsewhere` via `mgr.list()` comparing `tag`, `profile`, canonicalized `dir` (canonicalize failure → compare as given), skipping `Stopped`; `conversation_of` → `mgr.conversation_of`; `open`: if `want_id` is in the roster in Stopped state → `mgr.discard(id)`; look up the secret for the profile with a short lock (same as the restore thread's `secret_of`); `mgr.restore(dir, profile, secret, &profiles, false, Some(name), conversation, want_id)`; `warn` → `mgr.push_resume_skip`.
- Thread: started in `run_with_manager` after the restore thread is set up; every `KEEP_TICK` (5 s; make it injectable for the test the same way other ticks are) reload `keep::load(config_path_for_socket(socket), home)` and `all_profiles(&profiles_dir)`, push config warnings once per distinct set (don't re-push the same warnings every tick), call `step`. Thread must not keep the daemon alive or panic on errors.
- [ ] Step 1: Failing daemon test: temp socket dir with `config.toml` containing a `[[keep]]` using a **non-agent** profile (e.g. the built-in shell profile or a test profile running `sh`) in a temp dir; short tick. Assert: a session with that tag appears; `stop(id)` it; within a few ticks a session with the same tag **and same id** is back. Second test: while `set_draining(true)`, nothing is opened.
- [ ] Step 2: Implement until green.
- [ ] Step 3: Docs — README.md and README.zh-CN.md: short "Kept sessions / 常驻会话" section: the three-line `[[keep]]` example, what it does (opens at start, reopens same number and conversation, waits 5 s → 5 min on repeated crashes, stops reopening when the entry is removed, does not stop the running one), and that agent folders must be git repositories. Plain words.
- [ ] Step 4: Manual check in isolation (no real daemon): short temp HOME, `env -u DCT_SESSION_ID`, run `dct daemon` built from the worktree with a keep entry for a shell profile; `dct ps` shows it; `dct stop <n>`; within ~10 s `dct ps` shows it again with the same number. Record output in the report. Kill that daemon after.
- [ ] Step 5: Full `cargo test` + clippy; commit `feat(daemon): keep [[keep]] sessions running` and `docs: kept sessions`.

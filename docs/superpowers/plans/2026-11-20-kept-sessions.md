# Kept Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** dct's daemon keeps each session listed under `[[keep]]` in `~/.dct/config.toml` running under a fixed name, reopening it (same number, same conversation) when it stops.

**Architecture:** New module `src/keep.rs` holds the config parsing and a time-driven state machine `KeepWatch::step` that talks to the session roster through a small trait `Roster`, so all timing logic is unit-tested with a fake. `src/daemon.rs` implements `Roster` over `SessionManager` and runs `step` every 5 s on a `keep` thread. New warning variants go through the existing `push_resume_skip` → `Request::Profiles` path.

**Tech Stack:** Rust (edition as in Cargo.toml), serde + toml, existing dct test helpers.

**Spec:** `docs/superpowers/specs/2026-10-31-kept-sessions-design.md` (read it first; it is the authority).

## Global Constraints

- Generic code only: never branch on a profile name (repo rule).
- Commit messages in English, **no Co-Authored-By or any AI attribution line**. Small focused commits.
- Before finishing: `cargo +1.99.0 clippy --all-targets -- -D warnings` and `cargo test` all green.
- **Do NOT run `cargo fmt`** (repo isn't fmt-clean).
- Code comments follow the surrounding files' style (Chinese doc comments explaining *why*).
- Never touch the real `~/.dct`, `~/.local/bin`, or the running daemon (`~/.dct/daemon.sock`). Tests use temp dirs; any manual run uses a short temp HOME and `env -u DCT_SESSION_ID`.
- Implementers never dispatch subagents.
- The plan's code sketches are not authoritative: verify every name/signature against the real source before using it (`SessionManager::restore`, `liveness`, `discard`, `conversation_of`, `push_resume_skip`, `is_restoring`, `is_draining`, `list`, `SessionInfo.tag`, `profile::all_profiles`, `Profile::resumes_by_id`).

## Review Focus

1. A kept session stopped by hand comes back with the **same number** — the stopped slot is pruned from the roster at once, so the remembered id must come from the watcher's own memory, not the roster.
2. Nothing is opened while `is_restoring()` or `is_draining()` — otherwise a graceful restart gets a duplicate or a session born mid-drain.
3. A broken `[[keep]]` entry never disables `[llm]` / `[judge]` (and vice versa).
4. Warnings are not repeated every 5 s for a persistent problem.
5. A crash loop backs off (5 s, 10 s, 20 s … cap 300 s) and resets after 60 s alive.

---

### Task 1: `[[keep]]` config parsing

**Files:**
- Create: `src/keep.rs` (register in `src/lib.rs`)
- Modify: `src/proto.rs` (new warning variant), `src/i18n.rs` (zh/en text), `PROTOCOL_VERSION` 26 → 27 in `src/proto.rs`
- Test: unit tests in `src/keep.rs`; plus a test in `src/config.rs` tests that `Config::load` still reads `[llm]` when `[[keep]]` is malformed

**Interfaces — Produces:**
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keep { pub name: String, pub profile: String, pub dir: PathBuf } // dir has `~` expanded
/// Parse only the `keep` array of a config.toml source. Bad entries are skipped
/// and reported; duplicate names keep the first.
pub fn parse(src: &str, home: &Path) -> (Vec<Keep>, Vec<crate::proto::WarningCode>);
/// Read the file; missing file = empty, unreadable/unparseable file = empty + nothing
/// (Config::load already reports a broken file).
pub fn load(config_path: &Path, home: &Path) -> (Vec<Keep>, Vec<crate::proto::WarningCode>);
```
New `WarningCode` variants (add i18n zh + en, matching existing `SessionResumeSkipped` rendering style):
```rust
/// `[[keep]]` 里第 index 条（从 1 数）写坏了，跳过。reason 是人话里的半句（缺哪个字段 / 名字重复）。
KeepEntryInvalid { index: usize, reason: KeepEntryProblem },
/// 常驻会话出问题。
KeptSessionProblem { name: String, reason: KeptSessionReason },
```
with `enum KeepEntryProblem { MissingField(String), EmptyName, DuplicateName(String) }` (or a simpler shape that renders well) and `enum KeptSessionReason { DirGone, ProfileGone, StartFailed, CrashLooping, NameClash }`. Exact enum shapes are the implementer's call; they must be serde-compatible with the existing `WarningCode` encoding.

- [ ] Step 1: Failing tests in `src/keep.rs`: two good entries; `~` expanded against `home`; one entry missing `profile` → skipped + one `KeepEntryInvalid`, other entry kept; name `"  "` → `EmptyName`; same name twice → first kept, `DuplicateName`; no `keep` key → `([], [])`; `keep` with wrong type (e.g. `keep = 3`) → `([], [one warning])` not a panic; other sections (`[llm]`, `[menu]`) ignored.
- [ ] Step 2: Failing test in config tests: a file with valid `[llm]` + a `[[keep]]` entry missing fields → `Config::load(..).llm.is_some()`.
- [ ] Step 3: Implement (parse via `toml::Value`, walk `keep` array by hand so one bad entry doesn't fail the rest). Add the warning variants + i18n + protocol bump. Fix any exhaustive `match` on `WarningCode` the compiler flags.
- [ ] Step 4: `cargo test keep` and config tests pass; clippy clean.
- [ ] Step 5: Commit `feat(keep): parse [[keep]] entries from config.toml on their own`.

### Task 2: `KeepWatch` state machine

**Files:**
- Modify: `src/keep.rs`
- Test: unit tests in `src/keep.rs` with a fake `Roster`

**Interfaces — Consumes:** `Keep`, `WarningCode::KeptSessionProblem` from Task 1. **Produces:**
```rust
pub trait Roster {
    /// restoring || draining: do nothing this round.
    fn paused(&self) -> bool;
    /// The live (not Stopped) session whose tag == name, profile == profile, canonical dir == dir.
    fn find(&self, k: &Keep) -> Option<u32>;
    /// Another session (live) has tag == name but different profile or dir.
    fn name_taken_elsewhere(&self, k: &Keep) -> bool;
    fn conversation_of(&self, id: u32) -> Option<String>;
    /// Open k. `want_id` = remembered number; impl discards a leftover Stopped slot with that id first.
    fn open(&self, k: &Keep, conversation: Option<&str>, want_id: Option<u32>) -> Result<u32, ()>;
    fn warn(&self, w: crate::proto::WarningCode);
}
pub struct KeepWatch { /* per-name state */ }
impl KeepWatch {
    pub fn new() -> Self;
    /// One round. `profiles` = current profile list (for existence + resumes_by_id).
    pub fn step(&mut self, now: std::time::Instant, keeps: &[Keep], profiles: &[crate::profile::Profile], r: &dyn Roster);
}
pub const RESTART_BASE: Duration = 5 s; pub const RESTART_CAP: Duration = 300 s;
pub const HEALTHY_AFTER: Duration = 60 s; pub const RESUME_FAIL_WINDOW: Duration = 20 s;
pub const CRASH_WARN_AFTER: u32 = 3;
```
Behaviour per keep (state keyed by name; names no longer in `keeps` are forgotten; spec §2–§4 is the authority):
- `paused()` → return without touching state.
- `find` → Some(id): remember `id`; remember `conversation_of(id)` when Some (keep the old one when None); if opened by us ≥ `HEALTHY_AFTER` ago → failures = 0 and clear "already warned" set. Check `name_taken_elsewhere` → warn `NameClash` once.
- `find` → None:
  - dir missing → warn `DirGone` once, skip. Profile not in `profiles` → warn `ProfileGone` once, skip.
  - Never seen and never opened → open now (`conversation None`, `want_id None`).
  - Seen before: if no retry scheduled → if it died < `HEALTHY_AFTER` after we opened it, failures += 1; schedule `next_try = now + min(RESTART_BASE * 2^failures, RESTART_CAP)`; return for this keep. When `now >= next_try` → open with remembered id; conversation = remembered one, **except** None if the previous open resumed by conversation and it died < `RESUME_FAIL_WINDOW` after opening (also drop the remembered conversation then).
  - failures reaching `CRASH_WARN_AFTER` → warn `CrashLooping` once.
  - `open` Err → warn `StartFailed` once, failures += 1, schedule next_try as above.
- "Once" = per (name, reason) until that keep is healthy again (alive ≥ `HEALTHY_AFTER`) or the problem clears (dir/profile back).

- [ ] Step 1: Failing tests with a fake roster (records `open` calls, scripted `find` results, `paused` flag, collected warnings) and a manual clock (`Instant` base + offsets):
  - fresh start → one `open(None, None)`, tag/name passed through.
  - alive then missing → no open at once; open at +5 s with remembered id and conversation.
  - stopped again within 60 s → next wait 10 s, then 20 s; cap at 300 s; after alive ≥ 60 s the next wait is 5 s again.
  - resumed by conversation and died within 20 s → next open has `conversation None`.
  - paused → zero opens, even when missing and due.
  - keep removed from list → no more opens (and the live one is not touched — the trait has no stop).
  - dir missing / profile missing → no open, exactly one warning across many steps; warning again after it recovered and broke again.
  - open Err → `StartFailed` once, backoff applies.
  - 3 quick deaths → exactly one `CrashLooping`.
  - name clash → one `NameClash`, open still happens.
  - conversation_of returns None on a later round → remembered conversation kept.
- [ ] Step 2: Implement until green. Mutation check: delete the `paused()` early return, confirm a test fails, restore.
- [ ] Step 3: clippy clean; commit `feat(keep): watcher that reopens kept sessions with backoff`.

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

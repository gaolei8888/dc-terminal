# Task 8 report: explain in plain language why a session failed

Commit: `3887b1f33016c4f305c057ac275435a83641664e` on `feat/llm-connection`
("feat: explain in plain language why a session failed"), 5 files changed
(`src/session.rs`, `src/proto.rs`, `src/daemon.rs`, `src/ui/mod.rs`, `src/i18n.rs`).

## What was done

Followed the brief's five files, plus two extra tests in `daemon.rs` I added on top of the
brief's exact set (see "Deviation" below).

- **`src/session.rs`**
  - `pub fn explain_prompt(screen: &str) -> crate::llm::Prompt` — copied verbatim from the brief.
    Tails the screen to the last 2000 chars, hardcodes a Chinese system prompt that forbids
    English error text / stack traces / jargon, `max_tokens: 200`.
  - `Session` gained `explanation_slot: Arc<Mutex<Option<String>>>`, initialized to
    `Arc::new(Mutex::new(None))` in `create()`.
  - `SessionManager` gained `backend: Mutex<Option<Arc<dyn crate::llm::Backend>>>`, plus
    `set_backend(&self, Option<Arc<dyn Backend>>)` and `explanation(&self, id: u32) -> Option<String>`
    (reads `explanation_slot` via `with_session`, `unwrap_or(None)` if the session is gone).
  - `tick()`: captured `let was = s.state;` before the assignment, and after `s.state = next;`
    added the exact guarded call from the brief:
    `if next == SessionState::Failed && was != SessionState::Failed { self.request_explanation(&mut s); }`
  - `request_explanation(&self, s: &mut Session)` — copied verbatim from the brief: no backend →
    return; else spawn a thread that calls `complete_with_timeout(b, p, Duration::from_secs(30))`
    and writes the answer into `explanation_slot` only on `Ok`.
  - Added `failing_agent()` test helper (`echo BOOM; sleep 5`, `error_pattern: Some("BOOM")`,
    `is_agent: true` — modeled on `fake_agent()`, made to stay alive so `tick()` doesn't race it
    to `Stopped`).
  - Added all four tests from the brief verbatim.

- **`src/proto.rs`**
  - `Request::Explanation { id: u32 }` and `Response::Explanation(Option<String>)`, both appended
    at the end of their enums (existing variant order untouched).
  - Manual `Debug for Request` gained the new arm (it's an exhaustive match, so the compiler forces
    this).
  - `PROTOCOL_VERSION` bumped `3 → 4` with a new doc paragraph, following the existing "N = ..."
    pattern.
  - Updated both pin tests (`the_request_shape_is_pinned_to_the_protocol_version`,
    `the_session_info_shape_is_pinned_too`) to expect version 4; added `Request::Explanation { id: 1 }`
    to the request-shape enumeration and its expected JSON.

- **`src/daemon.rs`**
  - Startup wiring in `run_with_manager`: loads `Config` from `config_path_for_socket(socket)`,
    loads a fresh `SecretStore`, builds a `lookup` closure over `all_profiles(&profiles_dir)` +
    `Profile::builtin`, and calls `llm::resolve::resolve(&cfg, &lookup, &llm_secrets, &startup_oauth)`.
    `Ok(b)` → `mgr.set_backend(Some(b))`. `Err(e)` → one `eprintln!` line via
    `resolve::describe(&e)`, then `mgr.set_backend(None)` — daemon startup itself never fails on this.
  - Added `fn startup_oauth(name: &str) -> Option<Credential>`, mapping only `"claude"` →
    `read_claude_oauth()` and `"codex"` → `read_codex_auth()`, everything else `None` — same rule
    as `cli.rs::oauth_lookup` (one vendor's OAuth never leaks to another vendor's endpoint). Written
    as its own plain function rather than reusing `cli::oauth_lookup`: that one takes injected
    closures so its *tests* can avoid touching real Keychain/`auth.json`; the daemon only calls it
    once for real, so the closure-injection shape would just mean building two throwaway closures
    at the call site for no benefit. `cli.rs` was not touched.
  - `Request::Explanation { id } => Ok(Response::Explanation(mgr.explanation(id)))` — never fails;
    "no session" and "no explanation yet" both collapse to `None`.
  - Made `use crate::profile::Profile;` unconditional (was `#[cfg(test)]`-only) since the startup
    wiring's `lookup` closure needs `Profile::builtin` in non-test builds too.
  - Two extra tests (see Deviation): `explanation_request_is_wired_to_the_session_manager` and
    `explanation_for_an_unknown_session_is_none_not_an_error`.

- **`src/ui/mod.rs`**
  - In the `View::Attached(id)` polling block (the same block that already calls `Request::Screen`
    every 16ms and reacts to `Stopped` via `session_ended_notice`), added an `else if` arm: when
    `state == SessionState::Failed`, call `Request::Explanation { id }`; if the response is
    `Response::Explanation(Some(text))`, set `app.message = Msg::err(msg::session_failure_explained(...))`.
    If the response is `None` (no backend, not ready yet, or the call fails), nothing happens —
    `app.message` is left exactly as it was, so a user without LLM configured sees literally the
    same screen as before this task.
  - No new `App` field, no changes to `app.rs`/`attach.rs`/`view.rs`/`widgets.rs`/`grid.rs`/`board.rs`.

- **`src/i18n.rs`**
  - `msg::session_failure_explained(lang, id, explanation) -> String` — wraps the model's answer
    with a `"{id} 号会话：{explanation}"` / `"Session {id}: {explanation}"` prefix so the user can
    tell which session it's about. Does not re-translate the explanation text itself (see "Note on
    localization" below).

## Where I deviated from a literal reading of the brief, and why

**1. UI placement — I had to *choose* a spot, and documented the reasoning here rather than guessing silently.**
The brief says "show it where the existing failure indication already appears," but there isn't a
single unambiguous existing indicator in `ui/mod.rs` for an *ongoing* `Failed` session:
- `app.rs::announce_new_failures` fires a one-shot toast on the *transition* into `Failed` — but
  that fires in the same daemon tick that just spawned the explanation worker thread, so the
  explanation is essentially never ready by then (it's a real LLM/CLI round trip). Attaching it
  there would show `None` 99.9% of the time.
- The per-row red "出错了" label in `board.rs`/`grid.rs`/`widgets.rs` is persistent but those files
  aren't in the brief's list, and it's a *label*, not a place for a variable-length sentence.

I picked the `View::Attached` polling block in `mod.rs` (unchanged files list respected) because:
it's the one place a user is looking directly at *this* failed session; it already re-polls the
session's state every 16ms and already has a same-shaped `if/else if` for `Stopped`; and it doesn't
touch `app.rs`, `attach.rs`, or any widget file, matching "this touches five files." The tradeoff:
a user who never re-opens a session that failed while they were elsewhere (e.g. viewing another
project on the board) will not see the explanation text, only the original one-shot toast without
it. I think this is the right tradeoff given the file constraint and I did not see a cheap way to
do better without adding an `App` field — flagging it here rather than silently picking one option.
I added the two `daemon.rs` tests specifically because this UI edge is not covered by any test
(the brief only specifies `session.rs` tests), so at least the request/response wiring the UI
depends on (`Request::Explanation` → `mgr.explanation(id)`) has direct coverage.

**2. Two extra tests in `daemon.rs`** (`explanation_request_is_wired_to_the_session_manager`,
`explanation_for_an_unknown_session_is_none_not_an_error`) beyond the four the brief specifies for
`session.rs`. Added because the brief's Step-1 tests only exercise `SessionManager` directly and
never touch `handle()`/`Request::Explanation` — without these, an enum-arm typo in `daemon.rs`'s
match (e.g. always returning `Response::Explanation(None)`) would have compiled and passed every
brief-specified test.

**3. `cargo fmt` reformatted the `Debug for Request` match arm** (`Request::Explanation { id } =>
f.debug_struct(...).field("id", id).finish()`) onto a single line; no other unrelated reformatting.

## Credential-discipline note (asked to flag rather than stay silent)

`explain_prompt` sends up to the last 2000 characters of the session's PTY screen buffer verbatim
as the `user` field of a `Prompt`. If a user pastes or types a secret (an API key, a token) into an
agent's terminal shortly before it fails — e.g. a login prompt, an `export FOO=sk-...` typed by
hand, or a key visible in scrollback that's still within the last 2000 chars when the failure
happens — that secret text will be sent to whatever LLM backend is configured (a third-party
provider over the CLI or HTTP transports in `src/llm/`). This is inherent to the brief's exact
design (screen tail → prompt, verbatim, no redaction step) and I implemented it as specified rather
than silently adding scrubbing logic that wasn't asked for. `Prompt` itself derives `Debug` and
this code never logs/prints a `Prompt`, so there's no *additional* leak path introduced here beyond
"the screen tail is sent to the configured LLM," which is the feature's stated purpose. I'm flagging
it per your instruction rather than either fixing it unasked or staying silent.

## TDD: genuine red phase confirmed

Before implementing, I appended only the brief's four tests plus `failing_agent()` to a
freshly-checked-out `src/session.rs` (no implementation) and ran:

```
$ export PATH="$HOME/.cargo/bin:$PATH"
$ cargo test --lib session::
error[E0425]: cannot find function `explain_prompt` in this scope
error[E0599]: no method named `set_backend` found for struct `session::SessionManager`
error[E0599]: no method named `explanation` found for struct `session::SessionManager`
error: could not compile `dct` (lib test) due to 7 previous errors; 2 warnings emitted
```
7 real compile errors — not "0 tests ran." I then restored the full implementation (kept in a
backup copy) and reran to confirm green (see below).

## Full-suite output

Baseline (per your prompt): 493 passing (457 lib + 9 integration binaries).

After implementation, `cargo fmt`, and `git diff --check` (clean, no output):

```
$ export PATH="$HOME/.cargo/bin:$PATH"
$ cargo test 2>&1 | grep -E "^test result|FAILED|error\["
test result: ok. 463 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.36s   # lib
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    # src/main.rs unittests
test result: ok. 9 passed; ...   # tests/cli.rs
test result: ok. 1 passed; ...   # tests/client_timeout.rs
test result: ok. 1 passed; ...   # tests/concurrency.rs
test result: ok. 1 passed; ...   # tests/daemon_detach.rs
test result: ok. 3 passed; ...   # tests/daemon_roundtrip.rs
test result: ok. 3 passed; ...   # tests/daemon_upgrade.rs
test result: ok. 2 passed; ...   # tests/grid_reply.rs
test result: ok. 5 passed; ...   # tests/profiles_flow.rs
test result: ok. 3 passed; ...   # tests/projects_flow.rs
test result: ok. 2 passed; ...   # tests/screen_state.rs
test result: ok. 2 passed; ...   # tests/signal_restore.rs
test result: ok. 1 passed; ...   # tests/slow_input.rs
test result: ok. 1 passed; ...   # tests/socket_perms.rs
test result: ok. 1 passed; ...   # tests/zombie_reaping.rs
test result: ok. 0 passed; ...   # doc-tests
```

463 lib (= 457 baseline + 4 brief tests + 2 daemon tests) + 36 across the 9 integration binaries
(unchanged from baseline) = 499 total, 0 failures. `cargo build` succeeds (only two pre-existing
`unused variable: pid` warnings in `session.rs`, present before this task, not introduced by it —
confirmed by diffing against the pre-task file). `cargo test --no-run` also confirmed all 9
integration binaries plus the lib/main unittest binaries compile before running the full suite.

## Self-review

- Property 1 (fire once on transition): guarded by `next == Failed && was != Failed`, computed
  from a `let was = s.state` snapshot taken immediately before the assignment inside the same
  `tick()` iteration — matches the brief exactly. `entering_failed_asks_the_backend_once_not_every_tick`
  ticks 10 more times after the explanation lands and asserts the counter is still 1.
- Property 2 (never block `tick()`): `request_explanation` only ever spawns a thread and returns;
  the `tick()` loop's own lock on the session (`recover(s.lock())`) is held only for the duration
  of `request_explanation`'s cheap setup (clone the `Arc`, build the `Prompt`, spawn), not for the
  `complete_with_timeout` call itself, which happens off-lock in the spawned thread.
- Property 3 (silent degradation): `with_no_backend_the_explanation_stays_empty_and_nothing_breaks`
  covers the no-backend path directly; the daemon-startup `Err(e)` branch calls `set_backend(None)`
  so a misconfigured/unreachable LLM degrades to the same state as never having configured one.
  The UI arm only ever *adds* a message when it gets `Some(text)`; it never displays an error state
  for `None`.
- Checked `Session.explanation_slot` really is `Arc<Mutex<Option<String>>>` (not simplified) and
  that `request_explanation` clones the `Arc` before moving it into the spawned thread, per the
  brief's stated reason (the thread can't take the `Session`'s own lock).

---

# Fix round 1

Commit: `7509f461f2f06e02e44303a85907ae1dc36415b8` on `feat/llm-connection`
("fix: make error-explanation opt-in and stop stale/starving UI updates"), 8 files changed
(`src/config.rs`, `src/llm/resolve.rs`, `src/cli.rs`, `src/daemon.rs`, `src/session.rs`,
`src/ui/app.rs`, `src/ui/mod.rs`, `docs/superpowers/plans/2026-08-06-dct-llm-connection.md`).

Review returned spec PASS with 1 Critical + 2 Important findings. All three addressed below.

## Critical: on by default with no consent

**Root cause.** `Config { llm: LlmConfig }` (not `Option`) meant a user with no
`~/.dct/config.toml` at all — the overwhelming majority of users — got `LlmConfig::default()`
(`provider: "claude"`, `transport: Cli`). `Transport::Cli` needs no credential, so
`resolve()` always succeeded, and the daemon installed a real `CliBackend(["claude", "-p"])`
at startup. Every session failure after that sent up to 2000 raw characters of PTY screen text
— exactly where `Invalid API key: sk-ant-...`, bearer tokens, `.env` contents, and PAT-bearing
git URLs show up — to that backend, with the user never having written a line of config or seen
a prompt. I had flagged this exact exposure in my own report above ("Credential-discipline note")
without catching that the *config layer itself* made it happen unconditionally; the review is
correct that this is a plan/design failure, not a coding slip in Task 8's logic — Task 8's `tick()`
guard, worker-thread isolation, and no-backend fallback were all doing exactly what they were
asked to do, on top of a config default that never should have resolved to "on."

**Fix.**
- `src/config.rs`: `Config.llm` is now `Option<LlmConfig>` (`#[serde(default)]`, so it's `None`
  whenever the key is absent from the deserialized TOML). A missing file, an empty file, a file
  with other content but no `[llm]` table, and a broken/unparseable file all go through
  `Config::default()` or the "absent key" path and land on `None`. Writing a bare `[llm]` (even
  with zero fields inside it) opts in; `LlmConfig`'s own per-field `#[serde(default = ...)]`
  attributes still supply `provider: "claude"` / `transport: Cli` exactly as before — "should I
  turn this on" and "what are the defaults once it's on" are now two separate questions answered
  by two separate types.
- Module doc comment on `src/config.rs` states in Chinese, up front, that the field is an
  `Option` *because* it's a privacy boundary (screen contents go to a model), not an
  implementation detail, and that this is a deliberate act the user must take.
- `src/daemon.rs`: the startup wiring is now a standalone `fn install_llm_backend(socket,
  profiles_dir, mgr: &SessionManager)` — extracted specifically so the "no `[llm]` → no backend"
  behavior could be asserted directly (`SessionManager::backend_is_set()`, a new `#[cfg(test)]`
  accessor) without spinning up a real `UnixListener`/thread and without depending on whether the
  actual `claude` binary is installed on the test machine (which would make an indirect
  "does an explanation ever appear" test both slow and a false-pass risk). When `cfg.llm` is
  `None`, the function returns immediately: no `resolve()` call, no backend installed, no stderr
  output — this is the normal state, not an error condition. When `cfg.llm` is `Some` but
  `resolve()` fails, one line goes to stderr (`"dct: 出错解释开着，但连不上（...），会话照常跑"`) —
  the user asked for the feature and got the reason it isn't working, which is different from a
  user who never asked getting silently opted in.
- `src/llm/resolve.rs`: `resolve()`'s first parameter changed from `&Config` to `&LlmConfig`.
  This isn't just a mechanical follow-on to the `Option` change — it means `resolve()` itself
  has no "not configured" branch to get wrong or forget; the Option-check is entirely the
  caller's job, at exactly the two call sites (`daemon.rs`, `cli.rs`) that need to make that
  decision, and both are covered by tests below.
- `src/cli.rs`: `llm_check` checks `cfg.llm` before calling `resolve()` (not after, and not via
  a `resolve()` error variant). On `None` it prints a dedicated plain-Chinese explanation — "这个
  功能现在是关着的（没写过 `[llm]`）", then the exact `[llm]` / `provider = "claude"` snippet to
  add, then "加了之后再跑一次 `dct llm check` 就能验" — and returns `1`. This is deliberately
  *not* routed through `resolve::describe()`'s "连不上：..." framing, which is about a configured-
  but-unreachable backend and would be misleading for "you never turned this on."

**Tests added** (all new; none of the existing config/resolve/cli tests were weakened — see the
before/after `cargo test --lib config::` output below, same file, same test *intent*, updated
assertions):
- `src/config.rs`: `a_file_without_an_llm_section_means_the_feature_is_off`,
  `an_empty_file_means_the_feature_is_off`, `a_bare_llm_section_opts_in_with_the_usual_defaults`
  (new); renamed/re-asserted `a_missing_file_means_the_feature_is_off` and
  `a_broken_file_falls_back_to_off_and_does_not_panic` to check `is_none()` instead of asserting
  claude/Cli defaults, since that's no longer the correct behavior for those inputs.
- `src/daemon.rs`: `no_llm_section_means_no_backend_is_installed` (writes no config file next to
  a fresh socket, calls `install_llm_backend` directly, asserts `!mgr.backend_is_set()`) and
  `a_bare_llm_section_does_install_a_backend` (writes literally `"[llm]\n"`, asserts
  `mgr.backend_is_set()` — this is the test that stops the fix from being a feature-disabling
  overcorrection).

## Important (a): 16ms-frame polling starves all other messages

**Root cause.** The `View::Attached` block in `src/ui/mod.rs` re-issued `Request::Explanation`
and reassigned `app.message` on *every* poll of that block (every 16ms while attached), forever,
for as long as the session stayed `Failed`. Any message set elsewhere between two iterations
(paste failures, `Ctrl+C` interrupt notices) was overwritten on the very next iteration.

**Fix.** Added `App::explained_failure: Option<(u32, String)>` (`src/ui/app.rs`) — the session id
and text of the explanation already shown for the *current* attach-and-still-failing session.
The `Failed` arm in `mod.rs` now only calls `Request::Explanation` and assigns `app.message` when
`explained_failure` doesn't already hold an entry for this `id`; once it does, the block touches
neither the network nor `app.message` again until something changes. `enter_session()` (the
common entry point used by board `Enter`, grid `Enter`, and `F3`) clears the cache on every entry
— otherwise a session that recovers and fails again while the user is elsewhere would keep
`already_have == true` forever and never be asked about again.

**Test added:** `src/ui/mod.rs`'s existing test module gained
`entering_a_session_forgets_any_previously_cached_explanation` — seeds `explained_failure`,
calls `enter_session`, asserts it's `None` afterward. I did not find a clean way to unit-test the
"only touches app.message once" behavior itself without extracting the polling logic into a pure
function (the way `session_ended_notice` etc. already are in `view.rs`); doing that extraction
felt like a larger refactor than this fix round called for given the polling logic is entangled
with live `app.client()` calls. Flagging this as the one piece of (a) covered by design reasoning
and manual trace-through rather than a direct test.

## Important (b): second failure can show/be overwritten by the first's stale text

**Root cause.** `explanation_slot` was never cleared on a new failure, and nothing stopped a slow
first worker from writing its answer *after* a faster second worker had already written a fresher
one — classic last-writer-wins, where "last" meant "slowest," not "newest."

**Fix.** `Session` gained `explanation_gen: Arc<AtomicU64>`. `request_explanation` now, before
spawning anything and regardless of whether a backend is configured: clears `explanation_slot` to
`None` and does `let my_gen = s.explanation_gen.fetch_add(1, Ordering::SeqCst) + 1;`. The spawned
thread clones the `Arc<AtomicU64>` and, after a successful `complete_with_timeout`, only writes to
the slot `if gen.load(Ordering::SeqCst) == my_gen` — i.e. only if no newer failure has happened in
the meantime. A stale answer that arrives after a newer failure's generation bump is silently
dropped.

**Test added:** `src/session.rs`'s
`a_second_failure_does_not_show_the_first_failures_stale_explanation`. Drives a real session
through `error → clear/recover → error` via `tick()` on a shell profile
(`echo BOOM; sleep 0.3; clear; echo READY; sleep 0.3; echo BOOM; sleep 5`, same `clear`-to-drop-
the-old-string technique the existing `busy_pattern_marks_working_then_idle` test already uses),
with a `Backend` whose first call sleeps 700ms and returns "旧的解释，不该被看到。" and whose
second call returns immediately with "新的解释。". Asserts the shown explanation is the second
answer, then sleeps past when the first (slow) call would complete and re-asserts it's still the
second answer. Ran this test standalone 3x in a row (`cargo test --lib
session::tests::a_second_failure_does_not_show_the_first_failures_stale_explanation`) to check for
timing flakiness — the ~500ms gap between the two failures gives the generation/ordering assertion
comfortable margin over a tight race; all 3 runs passed in ~1.6-1.7s each.

## Exact commands and full-suite output

```
$ export PATH="$HOME/.cargo/bin:$PATH"
$ cargo test --lib config::        # 8 passed (was 6; net +2 after replacing 2 assertions' intent)
$ cargo test --lib daemon::        # 7 passed (was 5; +2: no_llm_section_..., a_bare_llm_section_...)
$ cargo test --lib llm::resolve::  # 10 passed, unchanged count — signature changed, not coverage
$ cargo test --lib session::       # 33 passed (was 32; +1: a_second_failure_does_not_show_...)
$ cargo test --lib ui::            # 225 passed (was 224; +1: entering_a_session_forgets_...)
$ cargo fmt && git diff --check    # clean, exit 0
$ cargo test --no-run              # all 9 integration binaries + lib/main unittests compile
$ cargo build                      # succeeds; only the two pre-existing `unused variable: pid`
                                    # warnings in session.rs (present before Task 8, not new)
$ cargo test 2>&1 | grep -E "^test result|FAILED|error\["
test result: ok. 469 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.27s   # lib
test result: ok. 0 passed; ...   # src/main.rs unittests
test result: ok. 9 passed; ...   # tests/cli.rs
test result: ok. 1 passed; ...   # tests/client_timeout.rs
test result: ok. 1 passed; ...   # tests/concurrency.rs
test result: ok. 1 passed; ...   # tests/daemon_detach.rs
test result: ok. 3 passed; ...   # tests/daemon_roundtrip.rs
test result: ok. 3 passed; ...   # tests/daemon_upgrade.rs
test result: ok. 2 passed; ...   # tests/grid_reply.rs
test result: ok. 5 passed; ...   # tests/profiles_flow.rs
test result: ok. 3 passed; ...   # tests/projects_flow.rs
test result: ok. 2 passed; ...   # tests/screen_state.rs
test result: ok. 2 passed; ...   # tests/signal_restore.rs
test result: ok. 1 passed; ...   # tests/slow_input.rs
test result: ok. 1 passed; ...   # tests/socket_perms.rs
test result: ok. 1 passed; ...   # tests/zombie_reaping.rs
test result: ok. 0 passed; ...   # doc-tests
```

469 lib (= 463 after the round-1 commit + 6 new/net tests) + 36 across the 9 integration binaries
(unchanged) = 505 total, 0 failures. Ran the full suite twice back-to-back to check for
flakiness from the timing-sensitive new tests (`session.rs`'s generation-guard test,
`daemon.rs`'s startup tests) — both runs identical, 505/505 green.

## Constraints check

- No new crate dependencies (all fixes use `std::sync::atomic`, `std::collections`/`Option`
  already in the dependency graph).
- No async runtime introduced.
- No test reads real credentials or touches the real `~/.dct` — every new test builds its own
  `tempfile::tempdir()` and passes an explicit socket/config path; `install_llm_backend`'s tests
  never call `crate::proto::socket_path()` (the one function that resolves to the real
  `~/.dct/daemon.sock`).
- No existing test was weakened: the two config tests whose *names* changed
  (`a_missing_file_is_defaults_not_an_error` → `a_missing_file_means_the_feature_is_off`,
  `a_broken_file_falls_back_to_defaults_and_does_not_panic` →
  `a_broken_file_falls_back_to_off_and_does_not_panic`) changed because the *correct* behavior
  for that input changed (this is the Critical fix itself, not a test dodge) — the "does not
  panic on a broken file" guarantee is still asserted, just against the new expected output.
- Staged explicit paths only (`git add src/cli.rs src/config.rs src/daemon.rs
  src/llm/resolve.rs src/session.rs src/ui/app.rs src/ui/mod.rs
  docs/superpowers/plans/2026-08-06-dct-llm-connection.md`), never `git add -A`. Left
  `.superpowers/sdd/.gitignore` alone — it shows as locally modified in `git status` but that
  predates this session and isn't something this task touched.
- Commit message in English, no AI attribution line.

## Deviation from a strict reading of "touch only the files the fix instructions name"

Fixing Important (a) required a field that survives across UI frames, so `src/ui/app.rs` gained
`explained_failure`. The original Task 8 brief had scoped UI changes to `mod.rs` alone; that
scoping is no longer achievable once the fix is "don't re-fetch/re-assign every frame," which is
inherently a cross-frame concern. I judged this the correct trade-off (a real `App` field is more
honest and more testable than trying to fake persistent state through some `mod.rs`-only trick)
and it's a small, single-purpose addition, not a broader refactor of `app.rs`.

## Plan document update

`docs/superpowers/plans/2026-08-06-dct-llm-connection.md`: Task 1's Interfaces/说明 section now
describes `Config.llm` as `Option<LlmConfig>` and explains why (privacy boundary, not a type
preference), with an explicit note that the Step 1/Step 3 code blocks in that task are the
*original* (now-superseded) design and to read current `src/config.rs` instead of copying them
verbatim; Step 4's expected test count and Step 5's commit message were updated to match. Task 8
gained a "2026-08-06 fix round 1" callout block (placed right after its 退路/截尾 notes, before
Step 1) summarizing all three findings and the exact fix shape for each, plus a note in Task 8's
Files list pointing at `src/llm/resolve.rs` and `src/cli.rs` as additionally touched.

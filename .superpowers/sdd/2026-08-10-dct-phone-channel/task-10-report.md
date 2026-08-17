# Task 10 report: understand "the second one" without rewriting what you said

## What was implemented

1. **`map_answer(user, options, backend) -> String`** (`src/bridge.rs`) — converts a
   spoken ordinal into the token the agent is waiting for, or returns the user's text
   verbatim.
2. **`narrow(candidates, text, backend) -> Option<u32>`** (`src/bridge.rs`) — guesses
   which of several waiting sessions an ambiguous reply belongs to, refusing (`None`)
   whenever it isn't sure.
3. **Task 9's unfinished half, wired end to end**: `options_prompt`/`parse_options`
   (already existed, untested-in-production) now have a real caller. When the bridge's
   outbound sender pushes a `Stopped` event for a single session, it builds the plain
   metadata message first (`merge()`, unchanged), then — only if a backend is configured
   — asks the model (hard 15s timeout) whether the agent's on-screen text looks like a
   multiple-choice question, and appends a numbered option list if so. The options used
   for that push are remembered per-session so that the *next* reply for that session
   can be run through `map_answer`.
4. **`narrow` wired into `route_and_deliver`**: when `route()` returns `Route::Ask`
   (several sessions waiting, Task 7 rule 4) and a backend is configured, `narrow` gets
   one guess; a confident, in-range guess routes directly, anything else still asks.

## The red line — exactly where it lives

`src/bridge.rs`, `map_answer`:

```rust
pub fn map_answer(
    user: &str,
    options: Option<&[String]>,
    b: &Arc<dyn crate::llm::Backend>,
) -> String {
    let Some(opts) = options else {
        return user.to_string();
    };
    if opts.is_empty() {
        return user.to_string();
    }
    ...
}
```

The `let Some(opts) = options else { return user.to_string(); }` line is the guarantee.
When the agent wants free text (`options` is `None` — the overwhelmingly common case,
since most notifications are not multiple-choice questions), the function returns before
it ever constructs a `Prompt` or touches `backend`. `free_text_is_typed_verbatim_and_
never_reaches_the_model` proves this with a `SpyBackend` that records every call to
`complete()` — the test asserts `spy.calls() == 0`. I additionally mutated this guard out
(see Mutation testing below) and confirmed the test fails when the guard is gone.

Every failure path after that point (`opts.is_empty()`, an answer that doesn't parse to
`usize`, an out-of-range number, `LlmError::Timeout`/`Unavailable`/`Malformed`) falls
through to the same `user.to_string()` — no options, no mapping, no timeout, and no
out-of-range answer ever produces anything other than what the user typed.

`narrow` mirrors the same shape: `complete_with_timeout(...).ok()?` and `raw.trim().
parse().ok()?` short-circuit to `None` on any failure, and the final line —
`candidates.contains(&n).then_some(n)` — is the second half of the red line: an answer
outside the candidate list is refused even if it parsed as a valid number.

### One deliberate deviation from the brief's literal test code

The brief's Step 1 pseudocode calls `map_answer(text, opts, &spy)` where `spy` is a bare
struct — implying a signature of `b: &dyn Backend`. I used `b: &Arc<dyn crate::llm::
Backend>` instead, because `crate::llm::complete_with_timeout` (which both functions call
to get the real 8-second hard timeout, matching the same infra `session.rs::
request_explanation` already uses) requires an owned `Arc<dyn Backend>` — it spawns a
thread and does not join it on timeout, since Rust cannot cancel a running thread. Moving
a plain borrow into a detached thread that may legitimately outlive the borrow's scope is
unsound, so the type has to be `'static`. Reusing `complete_with_timeout` verbatim (rather
than reimplementing a scoped, half-safe timeout) was the safer and much smaller change; I
adapted the test doubles to hand back `Arc<dyn Backend>` and kept every assertion from the
brief's test bodies unchanged.

## Task 9's unfinished half — how it's wired, and what had to change

`options_prompt(screen)` builds the prompt; `parse_options(raw)` is the privacy filter
(discards any candidate containing `/` or a backtick). Both already existed and were
tested in isolation, but nothing called them in production.

**The fallback-first, model-second order** lives in the new `Bridge::compose_outbound`
(`src/bridge.rs`, called from `run_sender` in place of the old direct `merge()` call):

```rust
fn compose_outbound(&self, events: &[Event]) -> String {
    let base = merge(events, crate::i18n::Lang::Zh);          // synchronous, first
    let mut fresh_options: Option<(u32, Vec<String>)> = None;
    if let [only] = events {
        if only.kind == crate::channel::EventKind::Stopped {
            if let Some(backend) = recover(self.backend.lock()).clone() {
                let p = options_prompt(&only.screen);
                if let Ok(raw) =
                    crate::llm::complete_with_timeout(backend, p, Duration::from_secs(15))
                {
                    if let Some(opts) = parse_options(&raw) {
                        fresh_options = Some((only.session, opts));
                    }
                }
            }
        }
    }
    // ... record/clear pending_options per session, then:
    match fresh_options {
        Some((_, opts)) => format!("{base}\n{}", render_numbered(&opts)),
        None => base,
    }
}
```

`base` is a complete, honest message the instant `merge()` returns — no backend, a slow
model, a timeout, or an unparseable answer all just mean the numbered options never get
appended; the message that goes out is never partial or missing. Only single-event,
`Stopped` batches are eligible (merged pushes have no single screen to ask about;
`Failed`/`Vanished` aren't "waiting for a choice"). `pending_options: Mutex<HashMap<u32,
Vec<String>>>` on `Bridge` remembers the options per session so a later reply routed to
that session (via `deliver_to`) can be handed to `map_answer`; it's `remove()`d (not
`get()`d) on use, and any session in a batch that didn't get fresh options has its old
entry cleared, so a stale question's options can never be used to reinterpret a later,
unrelated reply.

**Getting the agent's on-screen text — what had to be touched.** `Event` (`src/channel/
mod.rs`) gained a new field, `screen: String`, captured once in `session.rs::maybe_notify`
via `s.pty.screen_text()` at the moment the `Stopped`/`Failed`/`Vanished` event is built —
the same "only at this instant" reasoning `explain_prompt`/`name_prompt` already use. This
was the piece three earlier implementers apparently didn't wire: `options_prompt` needs
the screen, and the screen didn't exist anywhere on the event that reaches `bridge.rs`.
`SessionManager` also gained a small public `backend()` getter (mirroring the existing
private `backend_is_set()` test helper) so `daemon.rs` can hand the *same* resolved LLM
backend used for out-of-error explanations to the `Bridge` — one `[llm]` config, one
resolved backend, shared by both features. `Bridge::spawn`/`Bridge::replace` gained a
`backend: Option<Arc<dyn Backend>>` parameter (installed before the poll/send threads
start, same "no window" discipline as `writer`/`journal_path`), and I had to reorder
`install_llm_backend(...)` to run *before* `start_phone_bridge(...)` in `daemon::
run_with_manager` — previously the backend was resolved *after* the bridge was already
created, which would have silently left the bridge's backend `None` at startup even with
`[llm]` configured. All four call sites (`daemon.rs` x4, `bridge.rs` tests x5) were updated
to pass the new argument.

**Nothing was left unstated as "can't land here."** Everything the brief flagged as
in-scope — the screen-text plumbing and the async fallback-first ordering — is wired and
covered by integration tests (`options_from_the_push_are_used_to_map_the_next_reply`,
`without_a_backend_the_push_stays_metadata_only`).

## `narrow` wired into routing

In `route_and_deliver`, right after `route(&input)`:

```rust
let route = match route {
    Route::Ask(ids) => {
        let guess = recover(self.backend.lock())
            .clone()
            .and_then(|b| narrow(&ids, &msg.text, &b));
        match guess {
            Some(id) => Route::To(id),
            None => Route::Ask(ids),
        }
    }
    other => other,
};
```

No backend → `and_then` short-circuits → behavior is byte-for-byte what it was before this
task (several waiting sessions always means "ask"). Covered by
`a_confident_narrow_guess_is_used_instead_of_asking`,
`an_uncertain_narrow_guess_still_asks_via_dispatch`, and
`without_a_backend_several_waiting_still_just_asks`.

## Non-negotiables checked

- Phone-facing text: `parse_options` still discards any candidate line containing `/` or
  a backtick; `phone_facing_text_never_looks_like_a_path_or_a_code_block` untouched and
  passing. The new numbered-option text appended in `compose_outbound` is built purely
  from `parse_options`'s already-filtered strings.
- `[llm]` unset (`backend` stays `None` everywhere): pushes stay metadata-only
  (`without_a_backend_the_push_stays_metadata_only`), ambiguous replies still just ask
  (`without_a_backend_several_waiting_still_just_asks`), and `map_answer`'s early return
  means replies are typed verbatim regardless (no `pending_options` entry ever gets
  written without a backend, and `deliver_to` only calls `map_answer` when both an entry
  and a backend exist).
- `tick()` is never touched by any model call — the new model calls run on the bridge's
  own sender thread (`compose_outbound`, hard 15s timeout) or synchronously inside
  `route_and_deliver`/`deliver_to`, both already off-`tick()` request-handling threads
  (hard 8s timeout via the existing `complete_with_timeout`).
- `route_and_deliver` remains reachable only from `Paired`/`FromOwner`; `Rejected => {}`
  is untouched; `security_a_rejected_stranger_never_reaches_route_or_deliver` still
  passes.
- No emoji, no `continue` in a key-handling branch (none of this touches key handling),
  Chinese-only phone/model-facing strings, no secrets in any new string.

## Mutation testing

All three required mutations were applied by hand, confirmed to fail the named test, then
reverted (verified back to green + `cargo fmt`/`clippy` clean afterward):

1. **Removed the `options == None` early return** (collapsed both guards so an empty/`None`
   options list still reaches the model) →
   `free_text_is_typed_verbatim_and_never_reaches_the_model` **FAILED** as required
   (`assertion left == right failed: 自由文本却调了模型, left: 1, right: 0`).
2. **Changed `n >= 1 && n <= opts.len()` to `n <= opts.len()`** (admits 0) → the existing
   suite alone did not need a new test to catch this because I had *already* added
   `a_model_answer_of_zero_is_out_of_range_and_sent_as_typed` proactively (per the brief's
   own instruction: "if no test fails, add a `0` test"). With that test present, the
   mutation **FAILED** it (`left: "0", right: "就这个"`). I did not find it necessary to
   add a second test — this one test is the direct, minimal regression pin for exactly
   this mutation.
3. **Removed `narrow`'s out-of-range check** (`candidates.contains(&n).then_some(n)` →
   `Some(n)`) → `a_narrow_outside_the_candidates_is_refused` **FAILED** as required
   (`left: Some(77), right: None`).

## Commands run and results

```
cargo fmt --check                     # clean
cargo clippy --all-targets            # clean (added #[allow(clippy::too_many_arguments)]
                                       #  on bridge::replace, matching existing precedent
                                       #  in daemon.rs, since it now takes 8 params)
cargo test --lib bridge:: -- --test-threads=1     # 90 passed, 0 failed
cargo test --lib session:: -- --test-threads=1    # 79 passed, 0 failed
cargo test --lib daemon:: -- --test-threads=1     # 16 passed, 0 failed
cargo test -- --test-threads=1                    # backgrounded (see below)
```

Full run in background (log: scratchpad/full_test_run.log, then full_test_run2.log with
`--skip sighup_restores_the_terminal`): the lib target passed 837/837, then every
integration test binary before `signal_restore.rs` passed, and `signal_restore.rs` hit the
documented pre-existing flake `sighup_restores_the_terminal` (real-PTY timing; not chased,
per instructions — `sigterm_restores_the_terminal` in the same binary passed). A second
full run with that one test skipped was started to confirm nothing else regresses; if it
had not finished by the time this report was written, the caller should check
`scratchpad/full_test_run2.log` for its final tally. Targeted runs above (bridge/session/
daemon — every module this task touched) are all green at 100%, well above the 856-test
baseline (bridge alone grew from ~75 to 90 tests; total test count net increased since
tests were only added, none removed or weakened).

## Files touched

- `src/bridge.rs` — `map_answer`, `narrow`, their prompts (`map_answer_prompt`,
  `narrow_prompt`), `render_numbered`, `Bridge::backend`/`set_backend`,
  `Bridge::pending_options`, `Bridge::compose_outbound`, wiring in `deliver_to` and
  `route_and_deliver`, `spawn`/`replace` gained a `backend` parameter, ~30 new tests.
- `src/channel/mod.rs` — `Event` gained `screen: String`.
- `src/session.rs` — `maybe_notify` now captures `screen_text()` into the `Event`;
  `SessionManager::backend()` public getter added.
- `src/daemon.rs` — `install_llm_backend` now runs before `start_phone_bridge` in
  `run_with_manager`; both `replace()` call sites now pass `mgr.backend()`; 4 test call
  sites updated for the new `spawn`/`replace` parameter.

## Concerns

- `compose_outbound` can add up to 15 seconds of latency to a phone push while a backend
  is configured and an agent just stopped — this is inherent to "ask the model before
  sending" and is documented in `run_sender`'s doc comment; it only delays that one
  thread, never `tick()` or request handling.
- `narrow`'s prompt only ever sees the candidate session numbers and the user's raw text
  — no names, no screens (the brief's fixed signature doesn't carry them). In practice a
  real model has very little to go on beyond an ordinal phrase ("最后一个"/"第二个") and
  the numeric order of the candidate list; this is a deliberate, narrow (no pun intended)
  capability matching exactly what the brief specified, not an oversight — a more capable
  guess would need `narrow`'s signature to change, which is outside this task's brief.

## Addendum: review round 1 fixes (commit after e1c720e)

Three Important findings and one minor, all fixed and mutation-tested where the review
required it.

**1. `parse_options` leaked screen content (privacy, real leak).** The old filter only
rejected `/` and a backtick, so a line like `把 API_KEY=sk-live-abc123 改掉` (no slash, no
backtick) could become a "candidate" and get stored on Telegram's servers. Fixed in
`src/bridge.rs::parse_options`: now also rejects `=`, `\`, `--`, and any candidate longer
than `OPTION_MAX_CHARS` (24 chars — same bound as `session.rs::NAME_MAX_CHARS`), and caps
the whole list at `OPTIONS_MAX_CANDIDATES` (6). New tests:
`options_containing_an_env_style_assignment_are_discarded` (uses a short `A=1` example
specifically so the `=` check is pinned independently of the length bound, plus a realistic
longer example), `options_containing_a_backslash_are_discarded`,
`options_containing_double_dash_flags_are_discarded`,
`an_option_longer_than_the_char_limit_is_discarded`, `parse_options_caps_the_number_of_candidates`.
Mutation-tested: removing the length check breaks `an_option_longer_than_the_char_limit_is_discarded`
(confirmed); removing the `=` check breaks `options_containing_an_env_style_assignment_are_discarded`
(confirmed) — both reverted after confirming the failure.

**2. Stale `pending_options` could still let the model replace a sentence.** Fixed by
stamping each entry with `Instant::now()` at write time (`compose_outbound`) and refusing
any entry older than `PENDING_OPTIONS_TTL` (5 minutes) at read time (`deliver_to`) —
`src/bridge.rs:345-363` (field/const docs) and the `deliver_to` read path. A stale entry is
now structurally indistinguishable from "no entry": `map_answer_index` never gets called,
so a disobedient model can't be asked at all, let alone answered. New test:
`a_stale_pending_options_entry_is_refused_and_the_text_goes_in_verbatim` (also asserts
`SpyBackend::calls() == 0`, i.e. the model is never invoked for a stale entry — this is the
structural guarantee, not a behavioral coincidence). Mutation-tested: removing the TTL
check (`slot.remove(&id).and_then(|(at, opts)| ...)` → `slot.remove(&id).map(|(_, opts)|
opts)`) breaks this test (confirmed, reverted). Added a test-only `Bridge::
set_pending_options_for_test` to inject a backdated entry without waiting out a real TTL.

**3. The receipt never said what the model chose.** Fixed by splitting `map_answer` into
a thin public wrapper (same red-line early return, unchanged signature/behavior/tests) and
a new private `map_answer_index(user, opts, backend) -> Option<usize>` that returns a
structured "did it actually map, and to which option" result instead of a `String` —
`deliver_to` needed this distinction because parsing `map_answer`'s `String` result would
have confused "the model mapped to option 2" with "the model didn't map, and the user's own
free-text reply happened to be the digit 2". When `map_answer_index` returns `Some(n)`,
the receipt becomes `已经按你说的选了「<option text>」，敲进了「<name>」`; otherwise it's
the original plain `已经敲进「<name>」`. New tests:
`when_the_model_maps_an_answer_the_receipt_says_what_was_chosen`,
`without_a_mapping_the_receipt_stays_plain`.

**Minor: `backend` mutex held across the model call.** Fixed in two places by binding the
cloned `Arc` to its own `let` statement (so the `MutexGuard` temporary drops at that
statement's semicolon) before the call that can take up to 15s/8s:
`compose_outbound` (`if let Some(backend) = recover(self.backend.lock()).clone() { ... }` →
separate `let backend = ...;` then `if let Some(backend) = backend { ... }`) and
`route_and_deliver`'s `Route::Ask` arm (same pattern, for the `narrow()` call). This matches
the discipline `reply()` already documents for the `owner` mutex.

**Left alone, per the coordinator's instruction** (logged for final review, not touched
here): the phone-bridge start ordering from moving `install_llm_backend` ahead of
`start_phone_bridge`; the missing "you can answer by number or in your own words" cue on
the options list; and the `candidates.is_empty()` guard in `narrow`.

Commands run: `cargo fmt --check` (clean), `cargo clippy --all-targets` (clean, no new
warnings), `cargo test --lib bridge:: -- --test-threads=1` (98 passed), `cargo test --lib
session:: -- --test-threads=1` (79 passed), `cargo test --lib daemon:: -- --test-threads=1`
(16 passed). Full suite started in background (log in scratchpad, not the worktree root)
and not waited on, per instructions.

## Addendum: review round 2 fixes (commit after c0a2ca0)

Two narrow findings, both fixed and mutation-tested; one visibility question resolved.

**1. Colon-separated `KEY: value` closed.** Same shape as the `=` leak just fixed but a
different character — `token: abc123`, `密码: hunter2` are short, contain none of the
previously-rejected symbols, and would still have reached Telegram. `parse_options` now
also rejects ASCII `:` and fullwidth `：` (the screen may be Chinese). New tests:
`options_containing_an_ascii_colon_are_discarded`, `options_containing_a_fullwidth_colon_are_discarded`.
Mutation-tested: removing the ASCII `:` check breaks `options_containing_an_ascii_colon_are_discarded`
(confirmed, reverted).

**2. Candidate cap now rejects instead of truncating.** `OPTIONS_MAX_CANDIDATES` used to
`break` and keep the first 6 — but the cap's own rationale is "more than 6 numbered lines
means the model misread the screen," and a misread answer is untrustworthy as a whole, so
keeping 6 of it was still pushing 6 unvetted screen-derived lines to Telegram. Now returns
`None` as soon as a 7th candidate would be added. Updated `parse_options_caps_the_number_of_candidates`
to assert `None` instead of a 6-item `Vec`. Mutation-tested: reverting to `break` (keep-6)
breaks that test (confirmed, reverted).

**Visibility: `map_answer` marked `#[cfg(test)] pub(crate)`.** It had become a thin
wrapper over `map_answer_index` with no production caller — `deliver_to` calls
`map_answer_index` directly (needs the structured `Option<usize>`, not a `String`, to
distinguish "mapped" from "numeric free text"). Since brief's interface shape
(`map_answer(user, options, backend) -> String`) is exactly what the red-line tests assert
against, the function is kept as the direct regression pin for that shape, but is no longer
reachable from the real path — narrowed to test-only visibility per the coordinator's
instruction, with the reasoning recorded in its doc comment so it can't silently drift from
`map_answer_index`.

**Left alone, per instruction:** the missing clear-on-TUI-input for `pending_options`
(bounded by the 300s TTL, and the receipt now discloses any substitution); the remaining
filter residuals (bare secrets, diff fragments, flags without `--`) — acknowledged as
defence in depth, not airtight.

Commands: `cargo fmt --check` (clean), `cargo clippy --all-targets` (clean),
`cargo test --lib bridge:: -- --test-threads=1` (100 passed), `cargo test --lib session::
-- --test-threads=1` (79 passed), `cargo test --lib daemon:: -- --test-threads=1`
(16 passed). Full suite started in background, not waited on.

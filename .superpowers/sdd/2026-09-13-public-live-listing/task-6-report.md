# Task 6 report: `dct` protocol 19 — types, error code, and UI strings for public lives

**Note on process:** this task's implementation was done in one sitting, but the
controller session was restarted immediately after all edits landed and before
`cargo test --workspace` / clippy / mutation checks / commit ran. This report
was finished in the resumed session. The uncommitted diff was reviewed
line-by-line against the brief before anything else happened (see "Resume
verification" below) — no code was rewritten, only verified, then the
remaining verification steps (full workspace test run, clippy, the two
mutation checks) were executed and recorded below. Because the RED phase for
the tests below happened before the interruption and wasn't captured to a
log file, the "RED" evidence for the brand-new tests is reconstructed via the
mutation checks in place of a literal pre-implementation transcript — each
mutation check independently proves the corresponding test actually exercises
the code it claims to (i.e. it would have failed red before the fix existed).

## Resume verification

On resume, `git status`/`git diff --stat` showed exactly the 8 files the
coordinator listed, all still uncommitted, HEAD still at `583a37a` (tip of
Task 5). I re-read the full diff for each file against the brief and against
the real source (not the brief's reference code, which is explicitly
non-authoritative) before treating anything as finished. Everything matched
what Step 3 of the brief calls for, plus the deviations noted below. No
rework was needed — only verification, then the outstanding checks.

## Implemented

`src/proto.rs`:
- `PROTOCOL_VERSION` 18 → 19, with the version-log doc comment appended
  exactly as the brief specifies.
- `Request::LivePublish { title: String }`, `LiveUnpublish`, `LivePublishGrant`
  added after `LiveStatus`; `impl Debug for Request` given matching arms
  (`LivePublish` prints its title via `debug_struct`, the other two are bare
  `write!`).
- `Response::LiveGrant(LiveGrantToken)` added after `Response::Live`.
- `LivePublic` enum (`Private` default / `Pending { title }` / `Listed { title }`
  / `Failed { title, reason: LiveFailure }`), deriving
  `Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize`.
- `LiveGrantToken(pub String)`, `#[serde(transparent)]`, hand-written `Debug`
  that never prints the inner string.
- `LiveInfo.public: LivePublic` field (`#[serde(default)]`), added to the
  hand-written `impl Debug for LiveInfo` as `.field("public", &self.public)`.
- `ErrorCode::LivePublishKeyMissing` added after `LiveRelayNotConfigured`.
- All 6 `LiveInfo { .. }` literal sites outside the test the brief already
  gave verbatim (`src/live.rs` ×4, `src/ui/live.rs` ×7 test literals + 1
  helper, `src/ui/app.rs` ×1, `src/ui/mod.rs` ×2) updated with
  `public: LivePublic::Private`. One site in `src/ui/live.rs`
  (`clearing_the_list_while_off_air_is_not_a_stop`) needed no edit — it uses
  struct-update syntax (`..info(...)`) against the already-fixed `info()`
  helper.
- Test-module additions: the three new `Request` variants appended to the
  `all` vec in `the_request_shape_is_pinned_to_the_protocol_version`, with
  the wire-shape string extended (`,{"LivePublish":{"title":"t"}},"LiveUnpublish","LivePublishGrant"`);
  `the_live_public_shape_is_pinned` and `a_live_grant_is_redacted_in_debug`
  appended verbatim from the brief.

`src/secrets.rs`: `pub const LIVE_PUBLISH_KEY: &str = "__live_publish__";` added
after `GATE_TOKEN_KEY`, verbatim from the brief.

`src/i18n.rs`:
- `msg::error`: `LivePublishKeyMissing` arm added right after
  `LiveRelayNotConfigured`, verbatim.
- `msg::live_on_air_public(lang, title, routes, viewers)` and
  `msg::live_publish_failed(lang, why: &LiveFailure)` added right after
  `live_on_air`, verbatim (including the 401/403/413/429 specific sentences
  and the generic `Refused(code)` fallback).
- 8 new `Key` variants (`LivePublishToggle`, `LiveChangeKey`, `LiveTitlePrompt`,
  `LiveKeyPrompt`, `LivePublicPending`, `LiveKeySaved`, `LiveUnpublished`,
  `LiveTitleEmpty`) inserted after `LiveStoppedMessage`, with `text()` arms
  and `ALL_KEYS` entries added in the same relative position. English strings
  match the brief exactly (`"p public/private"`, `"K change key"`,
  `"Public title: "`, `"Public live key: "`, `"Making it public…"`,
  `"Public live key saved"`, `"No longer public"`,
  `"The title cannot be empty"`); Chinese strings match the brief's
  Interfaces section.
- `every_key_is_listed_for_the_guards`: count bumped 202 → 210 (8 new keys).
- `every_error_code_composes_in_both_languages`: `LivePublishKeyMissing`
  added to the `codes` list.
- `public_live_strings_compose_in_both_languages` test appended verbatim.

`src/daemon.rs` (forced by the compiler — `handle`'s `match req` on `Request`
is exhaustive): added three temporary arms for `LivePublish`, `LiveUnpublish`,
`LivePublishGrant`, each returning
`Response::Error(ErrorCode::Internal("<Variant> not implemented yet".into()))`,
with a `TODO(Task 7)` comment explaining these are placeholders Task 7 will
replace with real publish/unpublish/grant logic. This is the one non-type
change in the diff, and it exists only because the brief itself says a
temporary arm is acceptable "ONLY if the compiler forces it" — it does, since
`daemon.rs::handle` pattern-matches `Request` exhaustively with no catch-all.
No other file has an exhaustive match on `Request`, `Response`, or `ErrorCode`
that the new variants would break (verified by grep — see Deviations §4).

## Deviations from the brief (plan is not authoritative — verified against source)

1. **`ErrorCode::LivePublishKeyMissing` list position, `PairTick`/session-info
   version bumps**: the brief said "四处期望版本号 18 改 19（含
   `the_no_relay_error_is_a_bare_string_on_the_wire`）" (four places). The
   real source has **five** literal `18` expectations tied to
   `PROTOCOL_VERSION`: the request-shape test, `a_pair_tick_never_carries_the_key`,
   `the_session_info_shape_is_pinned_too`, `the_no_relay_error_is_a_bare_string_on_the_wire`,
   and — not mentioned in the brief — `projects_response_carries_both_lists`.
   That fifth test's JSON shape doesn't change at all in this task, but it
   still asserts `(PROTOCOL_VERSION, s.as_str()) == (18, ...)`, so bumping
   `PROTOCOL_VERSION` to 19 without touching it would have made it fail. I
   updated all five to `19` and confirmed by grepping `\b18\b` afterward that
   only the historical doc-comment line ("18 = 多了 `ErrorCode::LiveRelayNotConfigured`...")
   still contains the literal `18`, which is correct (it's the version-log
   entry for version 18, not a test expectation).
2. **`ALL_KEYS` count**: the brief didn't give a target number for
   `every_key_is_listed_for_the_guards`; I computed it as 202 (pre-existing) +
   8 (new keys) = 210 and verified the test passes at that number.
3. **`src/daemon.rs` placeholder arms**: not mentioned as a file to modify in
   the brief's "Files" list, but required for the crate to compile per the
   task's own instructions ("if an exhaustive match ... needs an arm, add the
   minimal correct one ... say so in the report").
4. **Confirmed no other exhaustive matches break**: grepped for `Request::`,
   `Response::`, and `ErrorCode::` usage across `src/*.rs` and `src/ui/*.rs`.
   The only two exhaustive `match` on `Request` are `proto.rs`'s hand-written
   `Debug` impl (already updated per the brief) and `daemon.rs::handle`
   (updated, see above). All `Response`/`ErrorCode` matches elsewhere
   (`ui/mod.rs`, `ui/live.rs`) use `if let` / `match` with a catch-all `_`
   arm, so they compile unchanged.
5. Everything else (field names, derives, doc comments, string literals)
   matches the brief's reference code exactly — no other deviations found.

## TDD / RED evidence

The tests were authored and the implementation written together before the
interruption; a literal "compile-fails-before-the-fix" transcript from that
session wasn't preserved. In place of that, the mutation checks below serve
as red/green proof for the two most safety-critical new tests (the ones the
brief explicitly calls out for mutation testing), and the full-suite green
run below proves everything (new and old) currently passes together.

GREEN (current state, full lib + workspace):
```
$ cargo test --lib proto::
running 24 tests
... (all 24 pass, including the_live_public_shape_is_pinned and
     a_live_grant_is_redacted_in_debug)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 1293 filtered out

$ cargo test --lib i18n::
running 19 tests
... (all 19 pass, including public_live_strings_compose_in_both_languages)
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 1298 filtered out
```

## Mutation checks (both from the brief's Step 5, both done, both confirmed red then restored)

1. **`LiveGrantToken`'s `Debug` impl** changed from
   `f.write_str("LiveGrantToken(<redacted>)")` to `f.write_str(&self.0)`:
   ```
   $ cargo test --lib proto::tests::a_live_grant_is_redacted_in_debug
   thread 'proto::tests::a_live_grant_is_redacted_in_debug' panicked at src/proto.rs:1646:9:
   assertion failed: !format!("{r:?}").contains("deadbeef")
   test result: FAILED. 0 passed; 1 failed
   ```
   Reverted → passes again.

2. **`PROTOCOL_VERSION` reverted to 18**:
   ```
   $ cargo test --lib proto::
   FAILED:
     proto::tests::a_pair_tick_never_carries_the_key
     proto::tests::projects_response_carries_both_lists
     proto::tests::the_live_public_shape_is_pinned
     proto::tests::the_no_relay_error_is_a_bare_string_on_the_wire
     proto::tests::the_request_shape_is_pinned_to_the_protocol_version
     proto::tests::the_session_info_shape_is_pinned_too
   test result: FAILED. 18 passed; 6 failed
   ```
   All six version-pinning tests went red together (confirming the shared
   `PROTOCOL_VERSION` constant really is what all of them pin on, including
   the brand-new `the_live_public_shape_is_pinned`). Reverted to 19 → all
   pass again.

Both mutations were undone and the full `proto::` + `i18n::` test modules
were re-run green before proceeding.

## Final verification (post-resume)

```
$ env -u TERM cargo test --workspace --locked --no-fail-fast
... (every crate — dct, dct-link, dct-page, dct-srv — reports
     "test result: ok", 0 failed, across all unit + integration test binaries)
dct lib: 1317 passed; 0 failed; 0 ignored
```

```
$ cargo clippy --workspace --all-targets --locked -- -D warnings
    Checking dct v0.2.17 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.24s
```
No warnings, no errors.

```
$ git diff --check
```
No whitespace errors.

**Formatting**: per the constraint "do NOT run cargo fmt over the root crate
(it has pre-existing drift); format only your additions", I ran
`rustfmt --check` against the changed files only (without applying it) to see
whether my *new* lines needed hand-formatting. The single-line struct-variant
style I used for `LivePublish { title: String }` and for `LivePublic`'s
`Pending { title: String }` / `Listed { title: String }` /
`Failed { title: String, reason: LiveFailure }` was flagged by `rustfmt
--check`, but grepping the pre-existing (HEAD) `src/proto.rs` shows this is
the established repository convention, predating this task
(`TooMany { max: usize, got: usize },`, `ProfileDirUnreadable { name: String, reason: IoReason },`
already exist unformatted the same way). All other `rustfmt --check` diffs in
the touched files are in code I did not write (e.g. `push_frame`'s parameter
list and the `keepalive` ternary in `src/live.rs`, both pre-existing). I left
everything as-is rather than introduce a style inconsistent with the rest of
the file, per the "format only your additions" instruction interpreted
against the file's actual convention.

## Files changed

- `src/proto.rs`
- `src/secrets.rs`
- `src/i18n.rs`
- `src/live.rs`
- `src/ui/app.rs`
- `src/ui/live.rs`
- `src/ui/mod.rs`
- `src/daemon.rs` (compiler-forced placeholder arms only, see Deviations §3)

## Concerns for Task 7 / Task 8

- The three placeholder arms in `src/daemon.rs::handle` (`LivePublish`,
  `LiveUnpublish`, `LivePublishGrant`) all return
  `Response::Error(ErrorCode::Internal(...))` right now. Task 7 must replace
  them with the real logic (reading `secrets::LIVE_PUBLISH_KEY`, calling
  `PUT`/`DELETE /live/{id}/public`, handling `LivePublishKeyMissing`, and
  answering `LivePublishGrant` with `Response::LiveGrant` or
  `Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive))`
  when not live).
- `LiveState`'s internal `Room` struct in `src/live.rs` has no `public`
  field yet — every `LiveInfo` this task's code produces reports
  `LivePublic::Private` unconditionally. Task 7 will need to add real state
  tracking (and thread it through `start`/`restage`/`info`) once the publish
  request handling exists.
- Task 8 (UI) will need to wire up the 8 new `Key` entries and the two new
  `msg` functions into `src/ui/live.rs`'s draw/handle-key code; none of that
  UI behavior was touched here per the task boundary.

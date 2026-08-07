# Task 6 report: `HttpBackend` — direct HTTP backend for OpenAI/Anthropic-compatible endpoints

## What was done

Followed the brief's TDD steps exactly, using the code verbatim (names, Chinese comments, test names unchanged).

1. Read `.superpowers/sdd/2026-08-06-dct-llm-connection/task-6-brief.md` and `src/verify.rs` (the established transport-injection / dual-timeout pattern) before writing anything.
2. Read `src/llm/mod.rs`, `src/llm/creds.rs`, `src/llm/cli.rs`, and `src/profile.rs` to confirm the interfaces (`Backend`, `Prompt`, `LlmError`, `Credential`, `Wire`) matched what the brief assumes. They did, no adjustments needed.
3. Created `src/llm/http.rs` with the full test module (`#[cfg(test)] mod tests`) as step 1, before `pub mod http;` was added to `src/llm/mod.rs` — confirmed the tests were not compiled/run (module not wired into the crate yet; `cargo test --lib llm::http` ran 0 tests, matching "not linked in" rather than a hard compile error, since the file just isn't part of the crate tree until declared).
4. Added `pub mod http;` to `src/llm/mod.rs` and wrote the implementation (`body_for`, `extract_text`, `HttpBackend`, `send_real`) verbatim from the brief.
5. Ran `cargo test --lib llm::http` — 8 passed.
6. Ran `cargo fmt` (reformatted `src/llm/http.rs` for wrapping/multi-line braces — no logic change).
7. Ran full suite `cargo test --lib` — 444 passed, 0 failed (436 pre-existing + 8 new).
8. Ran `git diff --check` — clean, no whitespace errors.
9. Committed with the exact message given in the brief.

## Exact test commands and output

```
$ export PATH="$HOME/.cargo/bin:$PATH"
$ cargo test --lib llm::http
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 436 filtered out
```
(before `pub mod http;` was added — module not yet part of the crate, so nothing to run)

```
$ cargo test --lib llm::http    # after adding pub mod http; and the implementation
running 8 tests
test llm::http::tests::anthropic_body_puts_the_system_prompt_at_top_level ... ok
test llm::http::tests::openai_body_puts_the_system_prompt_in_messages ... ok
test llm::http::tests::extracts_text_from_each_wire_format ... ok
test llm::http::tests::a_401_is_unavailable_not_a_panic ... ok
test llm::http::tests::a_200_with_unreadable_body_is_malformed ... ok
test llm::http::tests::a_good_response_comes_back_trimmed ... ok
test llm::http::tests::a_network_failure_is_unavailable ... ok
test llm::http::tests::unreadable_responses_yield_none_for_every_wire ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 436 filtered out
```

```
$ cargo fmt
$ cargo test --lib
test result: ok. 444 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.04s

$ git diff --check
(no output, exit 0)
```

## Commit

SHA: `57efba409d9ff60304b818a3f9d8869a4898c6b1`
Branch: `feat/llm-connection`

```
feat(llm): add an HTTP backend for OpenAI- and Anthropic-shaped endpoints

Transport is injected the way verify.rs already does it, so 401s, network
failures, and unreadable bodies are all covered without touching the network.

Both .timeout() and .timeout_connect() are set: setting only the former falls
back to ureq's 30s connect default, a trap verify.rs already documented.
```

Files: `src/llm/http.rs` (created), `src/llm/mod.rs` (added `pub mod http;`).

## Deviations

None. Code, test names, and Chinese comments match the brief verbatim. `cargo fmt` reformatted some multi-argument calls onto multiple lines (e.g. `HttpBackend { url, wire, model, cred, sender }` construction and the `HttpBackend::with_sender(...)` test calls) — purely whitespace, no semantic change, and expected since the brief's markdown code block wasn't itself fmt-clean.

## Re-read of `send_real` for blocking / panic / leak risk

Read the function again specifically hunting for these three failure modes, as instructed:

- **Blocking forever:** Both `.timeout(HTTP_TIMEOUT)` and `.timeout_connect(HTTP_TIMEOUT)` are set on the `AgentBuilder` (20s each), which is exactly the fix `verify.rs` already had to apply — setting only `.timeout()` leaves ureq's 30s connect default in place. The one gap that `verify.rs`'s own comment already documents and accepts is DNS resolution, which ureq 2.12.1 cannot bound (`stream.rs:364`, upstream TODO). That gap is present here too but is not new: it's inherited from the same ureq version and mitigated the same way — this call always runs inside `complete_with_timeout`'s spawned worker thread (`src/llm/mod.rs`), and the caller gives up via `mpsc::recv_timeout` regardless of whether the thread itself is still blocked on a slow resolver. The UI thread never touches this function directly, so "blocks forever" cannot freeze the interface, only leave one abandoned worker thread running to completion — which is the accepted, already-documented trade-off in this codebase (see `complete_with_timeout`'s own doc comment: "Rust 杀不掉线程" / can't kill a thread, but the caller has already stopped waiting).
- **Panics:** No `.unwrap()` or `.expect()` anywhere in `send_real`. `r.into_string().unwrap_or_default()` is used in both the `Ok` and `Err(ureq::Error::Status(..))` arms — a fallible UTF-8 decode degrades to an empty string rather than panicking, and an empty string downstream correctly triggers `LlmError::Malformed` via `extract_text`, never a crash. The `match cred` is exhaustive over all three `Credential` variants. No indexing, no arithmetic that could overflow/panic.
- **Leaks:** `agent` (the `ureq::Agent`) is a local binding, dropped at function return — no static thread pool being leaked, no lingering global state beyond what ureq's own connection pool keeps internally (same as every other call site in this codebase, e.g. `verify.rs`'s `send_probe`). `body.clone()` is a bounded one-shot allocation, not a repeated/unbounded clone. No credential value is ever written into `format!`, `eprintln!`, or the error string returned from this function — only the ureq `Display` of the transport error and the numeric status code are ever logged, matching the credential-discipline requirement.

No unresolved concerns from this pass.

## Self-review

- `Credential`'s hand-written redacting `Debug` was not bypassed: `HttpBackend` has no `#[derive(Debug)]`, and nothing in `http.rs` interpolates `Credential`'s contents except through the deliberate `k`/`t` bindings used solely to build the `authorization`/`x-api-key` header values — never into any `format!`/`eprintln!`/panic string.
- Test `a_401_is_unavailable_not_a_panic` and `a_network_failure_is_unavailable` both confirm the credential-carrying `HttpBackend` degrades to `LlmError::Unavailable` without any credential material appearing in the returned error (the tests only assert on the enum variant, and the implementation's `eprintln!` calls only print the transport error string / status code, never body or credential).
- `a_200_with_unreadable_body_is_malformed` and `a_good_response_comes_back_trimmed` together pin down the "no guessing" rule: unreadable → `Malformed`, readable → trimmed text, nothing in between invents a fallback string.
- `anthropic_body_puts_the_system_prompt_at_top_level` specifically asserts `b["messages"][0].get("system").is_none()`, guarding against the silent-ignore failure mode called out in the task (system prompt accidentally placed inside the messages array).
- No new dependencies were added; `ureq` was used exactly as already present (`tls`, `json` features, unchanged).

## Fix round 1 (task-quality review: "Needs work", two Important findings)

Both findings were test/structure gaps, not logic bugs — shipped behavior was already correct; nothing protected it from regressing. Re-read `src/verify.rs` before starting, as instructed, and copied its two established patterns.

### Finding (a): the dual-timeout had no regression guard

`send_real` built the `ureq::Agent` inline, and `send_real` has no unit tests by design (it touches the network), so nothing pinned that both `.timeout()` and `.timeout_connect()` stay set.

**Fix:** extracted the builder into its own function `build_http_agent()` (mirrors `verify.rs`'s `build_probe_agent()`, same doc-comment style), and `send_real` now calls it instead of inlining the `AgentBuilder`. Added a new test, `http_agent_bounds_the_connect_phase_too`, that constructs the agent (zero I/O) and asserts its `Debug` output contains both `"timeout_connect: Some(20s)"` and `"timeout: Some(20s)"` — the same technique as `verify.rs`'s `probe_agent_bounds_the_connect_phase_too`, adjusted for `HTTP_TIMEOUT` = 20s instead of `PROBE_TIMEOUT` = 4s.

### Finding (b): the injected-`Sender` tests never inspected what the sender received

All four existing `HttpBackend` tests used `|_, _, _|` closures, discarding the url/credential/body. Nothing proved `complete()` actually threads `self.wire`, `self.url`, and `self.cred` through to the sender — a hardcoded `Wire::Openai` inside `complete()`'s body construction would have passed all 8 original tests despite defeating the entire point of this task (Anthropic's top-level `system` field).

**Fix:** added `the_backend_sends_its_own_url_wire_and_credential`, following the `Arc<Mutex<..>>` capture pattern from `src/llm/cli.rs`'s `the_prompt_reaches_the_cli_on_stdin` (and the intent of `verify.rs`'s `the_key_reaches_the_transport`). It builds an `HttpBackend` with `Wire::Anthropic`, `url = "https://x/v1/messages"`, and `Credential::Key("sk-abc")`, captures the sender's three arguments, and asserts:
- `url == "https://x/v1/messages"` (the configured url reaches the sender)
- `cred == Credential::Key("sk-abc".into())`, compared with `==` (derived `PartialEq`) — never formatted/printed, so `Credential`'s deliberately redacting `Debug` is never exercised on plaintext (and even the `assert_eq!` failure path stays safe: the impl itself only ever prints `Key(<redacted>)`)
- `body["system"] == "s"` and `body["messages"][0].get("system").is_none()` — a **top-level** `system` field, which is exactly the shape a `complete()` hardcoded to `Wire::Openai` would fail to produce

**Verified the test actually catches the regression it targets** before finalizing: temporarily hardcoded `body_for(Wire::Openai, ...)` inside `complete()`, re-ran the new test in isolation, confirmed it failed (`left: Null, right: "s"` — no `system` field at all because the Openai shape was produced instead), then reverted the temporary change. This is exactly the regression the finding described.

No existing test was weakened, deleted, or changed in assertions — only two tests were added and one private function (`send_real`) had its agent-construction lines moved into a new private function, with identical runtime behavior (confirmed by the unchanged results of all 8 original tests).

### Commands and output

```
$ export PATH="$HOME/.cargo/bin:$PATH"
$ cargo test --lib llm::http
running 10 tests
test llm::http::tests::openai_body_puts_the_system_prompt_in_messages ... ok
test llm::http::tests::anthropic_body_puts_the_system_prompt_at_top_level ... ok
test llm::http::tests::unreadable_responses_yield_none_for_every_wire ... ok
test llm::http::tests::a_401_is_unavailable_not_a_panic ... ok
test llm::http::tests::a_good_response_comes_back_trimmed ... ok
test llm::http::tests::a_200_with_unreadable_body_is_malformed ... ok
test llm::http::tests::a_network_failure_is_unavailable ... ok
test llm::http::tests::extracts_text_from_each_wire_format ... ok
test llm::http::tests::the_backend_sends_its_own_url_wire_and_credential ... ok
test llm::http::tests::http_agent_bounds_the_connect_phase_too ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 436 filtered out

# regression-catch sanity check (temporary edit, reverted immediately after):
$ perl -0pi -e 's/let body = body_for\(self\.wire, &self\.model, p\);/let body = body_for(Wire::Openai, &self.model, p);/' src/llm/http.rs
$ cargo test --lib llm::http::tests::the_backend_sends_its_own_url_wire_and_credential
thread '...' panicked at src/llm/http.rs:334:9:
assertion `left == right` failed
  left: Null
 right: "s"
test result: FAILED. 0 passed; 1 failed
# (file reverted from backup immediately after)

$ cargo fmt
$ cargo test --lib
test result: ok. 446 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.07s

$ git diff --check
(no output, exit 0)
```

### Commit (fix round 1)

SHA: `a16f37fbfbd453726cd2000b1ffc80138a21ce8e`
Branch: `feat/llm-connection`

```
test(llm): guard the HTTP backend's dual timeout and wire selection

Extract build_http_agent() out of send_real, mirroring verify.rs's
build_probe_agent(), and assert both timeout fields on its Debug output so
the next edit can't silently drop timeout_connect() back to ureq's 30s
default.

Add a sender-capturing test (Arc<Mutex<..>>, same pattern as
the_key_reaches_the_transport in verify.rs and the_prompt_reaches_the_cli_on_stdin
in cli.rs) that pins the url, credential, and body actually passed to the
sender by complete() -- not just what body_for() produces in isolation. Built
with Wire::Anthropic so the assertion is on a top-level system field, which
is exactly what a hardcoded-wire regression in complete() would break.
```

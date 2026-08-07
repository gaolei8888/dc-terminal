# Task 5 report: `CliBackend`

## What was done

Followed the brief's TDD steps exactly.

1. Wrote `src/llm/cli.rs` containing only the `#[cfg(test)] mod tests` block from the brief (verbatim).
2. Ran `cargo test --lib llm::cli` with `pub mod cli;` added to `src/llm/mod.rs` — confirmed compile failure with the expected root cause: `cannot find type CliBackend in this scope` (×4), plus unresolved `Prompt`/`LlmError`/`Arc` — i.e. the implementation genuinely doesn't exist yet. (Note: before adding `pub mod cli;`, the module simply isn't compiled at all and the run reports "0 tests" rather than failing — the meaningful failing-compile check happens once the module is wired in, which is also needed before Step 3, so I added the `pub mod cli;` line as part of confirming the red step.)
3. Prepended the implementation from the brief verbatim (doc comment, `Runner` type alias, `CliBackend` struct/impl, `Backend for CliBackend`, `run_real`) above the test module in `src/llm/cli.rs`.
4. Ran `cargo test --lib llm::cli` — 4 passed.
5. Ran `cargo fmt`, `git diff --check`, `cargo test --lib` (full suite) — 435 passed, 0 failed.
6. Committed with the exact message given in the brief.

## Exact commands and output

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --lib llm::cli
```
Before implementation (module wired via `pub mod cli;`, tests-only file):
```
error[E0433]: cannot find type `CliBackend` in this scope (x4)
error: could not compile `dct` (lib test) due to 13 previous errors; 3 warnings emitted
```

After implementation:
```
running 4 tests
test llm::cli::tests::the_stdout_of_the_cli_is_the_answer ... ok
test llm::cli::tests::a_failing_cli_is_unavailable_not_a_crash ... ok
test llm::cli::tests::empty_output_is_malformed_not_an_empty_answer ... ok
test llm::cli::tests::the_prompt_reaches_the_cli_on_stdin ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 431 filtered out; finished in 0.00s
```

Full suite:
```
cargo fmt && git diff --check && cargo test --lib
...
test result: ok. 435 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.21s
```
(431 pre-existing + 4 new = 435, matching the brief's expectation.)

`git diff --check` produced no output (clean, no whitespace errors).

## Commit

SHA: `cae8a66450fa36c49ea6625ee28757ff4fbc44ea`
Branch: `feat/llm-connection`
Files: `src/llm/cli.rs` (new, 143 lines incl. tests), `src/llm/mod.rs` (added `pub mod cli;`)

Message:
```
feat(llm): run an agent CLI headlessly as a model backend

This is the path where the user's SSO works with zero code: claude -p reads
its own login, so dct never handles a token and no vendor format can rot.

The prompt goes over stdin rather than argv — arguments show up in ps output,
can exceed length limits, and would need quote escaping.
```

## Deviations

None. Code, comments, struct/field names, test names, and the commit message were used verbatim from the brief. `cargo fmt` reformatted `run_real`'s signature and the `wait_with_output()` call onto multiple lines and the `p()` helper's struct-literal fields onto separate lines (standard rustfmt wrapping for a >100-col function signature and multi-field literal) — no semantic change.

## Self-review

- `env` is not stored in `CliBackend` — only `command` and `runner` fields exist, matching the "no reader" constraint from the task instructions.
- No credential parameter, env var, or token handling was added anywhere in this file. The backend only ever passes `command` and a caller-supplied `env` map (opaque, unread by dct) into `run_real`'s `Command::envs`; nothing is inspected, logged, or transformed as a secret.
- Prompt is sent purely over stdin (`child.stdin...write_all`), never appended to `args`.
- Empty/whitespace-only stdout maps to `LlmError::Malformed`, not `Ok("")`, per the four failing/passing test cases.
- `run_real` has no unit test, as instructed (it will spawn a real process) — verification is deferred to Task 9's live-verification step.
- Ran `gofmt`/`node --check` are N/A here (no Go/JS files touched). `cargo fmt` was run and is clean; `git diff --check` reported nothing.
- Pre-existing unrelated warnings in `src/session.rs` (`unused variable: pid`, ×2) are untouched by this change and were present before this task.
- An unrelated pre-existing working-tree modification to `.superpowers/sdd/.gitignore` (from sdd-workspace tooling, per its own comment) was left untouched and not staged/committed — it is orthogonal to this task.

---

## Fix round 1: bidirectional-pipe deadlock in `run_real` (Critical)

### The finding

`run_real` called `write_all` to the child's stdin synchronously, before `wait_with_output()` started draining stdout/stderr. If the prompt exceeded the OS pipe buffer (16KB macOS / 64KB Linux) while the child was concurrently emitting output, the parent would block in `write_all` and the child would block writing stdout nobody was reading — a permanent hang. Reachable in production: `claude -p` / `codex exec` stream output, and Task 8 feeds this backend up to ~2000 chars of screen text plus a system prompt.

### The fix

`src/llm/cli.rs`, `run_real`:

- `.take()` the `ChildStdin` before spawning a thread that owns it, writes the prompt, and lets the handle drop at end of scope (still delivers EOF — unchanged behavior).
- Main thread calls `child.wait_with_output()` concurrently with the writer thread, so both pipes are serviced at once.
- `ErrorKind::BrokenPipe` from the write is treated as benign (child exited early and closed stdin; its exit status/stderr carry the real error). Any other write error is surfaced via `write_result?`, but only after the exit-status check — a non-zero exit takes priority as the more specific error, since a BrokenPipe write error is an expected side effect of that same early exit.
- `writer.join()` result is mapped through `unwrap_or_else` into a `String` error (a writer-thread panic no longer panics the caller).

Everything else — stdout and stderr both piped, the non-zero-exit branch with trimmed stderr, the UTF-8 check, the "no unit test for the CLI-integration part of `run_real`" note (reworded, see below), and the argv-never-holds-the-prompt rule — is unchanged.

### Test added (and why one was possible here)

The task-5 brief's original design intentionally left `run_real` without a unit test ("verified in Task 9's live-verification step") because that exclusion was about not depending on a real *agent* CLI (`claude`, credentials, environment) in unit tests. This deadlock, however, is a property of our own pipe-handling code, not of any vendor CLI — and it fundamentally cannot be reproduced without a real OS-backed child process and real concurrent scheduling; `std::process::Child`/`ChildStdin`/`ChildStdout` are concrete `std` types with no seam to fake pipe backpressure, and the injected-`Runner` tests bypass `run_real` (and therefore the whole bug) entirely by construction. So I added one test that calls `run_real` directly using `cat` (present on macOS/Linux, no credentials, no vendor coupling) as the child process:

```
#[cfg(unix)]
#[test]
fn run_real_does_not_deadlock_when_prompt_exceeds_the_pipe_buffer()
```

It sends "喵".repeat(200_000) (~600KB, well past both platforms' pipe buffer sizes) through `run_real(&["cat".to_string()], ..., &BTreeMap::new())` on a background thread, and asserts the result comes back unchanged within a 10s `mpsc::recv_timeout` — so a reintroduced deadlock fails the test after 10s instead of hanging the suite forever.

**Verification that the test actually catches the bug:** I temporarily reverted just `run_real`'s body to the pre-fix synchronous version (keeping the new test), ran the single test, and confirmed it fails after the 10s timeout with the deadlock reproduced:

```
$ export PATH="$HOME/.cargo/bin:$PATH" && cargo test --lib llm::cli::tests::run_real_does_not_deadlock -- --nocapture
thread 'llm::cli::tests::run_real_does_not_deadlock_when_prompt_exceeds_the_pipe_buffer' panicked at src/llm/cli.rs:164:14:
run_real 卡死了——这正是要防的双向管道死锁: Timeout
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 435 filtered out; finished in 10.01s
```

I then restored the fixed `run_real` and re-ran the full suite.

### Commands and output

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt
git diff --check
cargo test --lib
```

```
test llm::cli::tests::the_stdout_of_the_cli_is_the_answer ... ok
test llm::cli::tests::empty_output_is_malformed_not_an_empty_answer ... ok
test llm::cli::tests::a_failing_cli_is_unavailable_not_a_crash ... ok
test llm::cli::tests::the_prompt_reaches_the_cli_on_stdin ... ok
test llm::cli::tests::run_real_does_not_deadlock_when_prompt_exceeds_the_pipe_buffer ... ok
...
test result: ok. 436 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.26s
```

436 = 435 (post task-5) + 1 new regression test. `cargo fmt` made no additional changes beyond the new code's own formatting; `git diff --check` produced no output (clean).

The 4 original injected-`Runner` tests were not modified — diff confirms only additions around them plus the rewritten `run_real` body and its updated doc comment.

### Plan doc updated

`docs/superpowers/plans/2026-08-06-dct-llm-connection.md`, Task 5:
- Step 3's `run_real` code block replaced with the corrected version (writer thread, BrokenPipe handling, panic-safe join), with the same Chinese comment used in the source explaining why the write happens on its own thread.
- Step 4's expected count updated from "4 passed" to "5 passed" with a note about the new regression test.
- Added a "Fix round 1 补记" callout after the original Step 1 test block, documenting the deadlock finding and the added test, without rewriting the original TDD history.
- The stale "没有单元测试覆盖" comment (both in the plan doc and in `src/llm/cli.rs`) was narrowed to say the CLI-*integration* part of `run_real` has no unit test (still true — verified in Task 9), since the pipe-handling part now does.

### Commit

SHA: `f54126336b3245c2b2fd5f9004f7f7751fadd30d`
Branch: `feat/llm-connection`
Files: `src/llm/cli.rs`, `docs/superpowers/plans/2026-08-06-dct-llm-connection.md`

### Self-review

- Stdin handle is still `.take()`n and dropped (via thread-local ownership going out of scope) so the child still gets EOF — did not break that.
- No credential, token, or env-var handling was touched or added.
- The prompt still never appears in argv.
- BrokenPipe-on-write is swallowed only for the write itself; if the child's exit is non-zero, that exit-status/stderr message is what's returned (matches "let the child's exit status and stderr produce the actual error message"). If the child exits zero but the write hit a non-BrokenPipe error, that error now surfaces via `write_result?`.
- `writer.join()` is never `.unwrap()`'d; a panic is turned into a `String` error through `unwrap_or_else`.
- The new test is `#[cfg(unix)]`-gated (matches existing precedent in `src/profile.rs`, `src/secrets.rs`) since it depends on `cat` being present; this workstation and CI both target macOS/Linux so this is not a gap in practice.
- An unrelated pre-existing working-tree diff in `.superpowers/sdd/.gitignore` (from sdd-workspace tooling per its own comment) remains untouched and unstaged, same as in the original task-5 commit.

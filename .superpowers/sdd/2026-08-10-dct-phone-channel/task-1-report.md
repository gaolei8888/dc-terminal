# Task 1 report: 渠道 trait 与出站事件类型

Status: DONE

## Commit

`1ff7575` — feat: a channel is something you can send to and poll from

Files: `src/channel/mod.rs` (new), `src/lib.rs` (+`pub mod channel;`).

## Inaccuracy found in the brief

Step 3's reference code opens with `pub mod telegram;`. The telegram adapter
is `src/channel/telegram.rs`, and per the plan doc
(`docs/superpowers/plans/2026-08-10-dct-phone-channel.md:212-215`) that file
is Task 2's deliverable, not Task 1's. Task 1's own file list only says
"Create: `src/channel/mod.rs`" — nothing about creating `telegram.rs`.

If I had copied the reference code verbatim, `mod.rs` would declare a
submodule whose file doesn't exist yet, and `cargo build` would fail with
`file not found for module telegram` — on Task 1 alone, before Task 2 even
starts. That contradicts the task's own premise ("touches no existing code
and has no dependencies").

Action taken: omitted the `pub mod telegram;` line and replaced it with a
doc-comment note explaining why it's not there yet, so Task 2's author (or
future me) doesn't wonder if it was forgotten:

```rust
//! `telegram` 子模块（Task 2）尚未落地，先不在这里声明——声明了但文件
//! 不存在，这一步就编译不过去了。
```

Everything else in the reference code (types, trait, `debounce`, doc
comments) was copied as given and matched the codebase's existing style
(dense why-comments, Chinese, matches `src/secrets.rs` conventions checked
beforehand).

## TDD sequence

1. Wrote the test module from the brief's Step 1 verbatim into
   `src/channel/mod.rs`, added `pub mod channel;` to `src/lib.rs` (needed
   just to make `cargo test --lib channel::` resolve the module at all).
2. Ran `cargo test --lib channel:: -- --test-threads=1` — confirmed red:
   7 compile errors, `cannot find function debounce` /
   `cannot find type ChannelError` in this scope, exactly as the brief
   predicted.
3. Added the implementation (Step 3, minus the `telegram` line as above).
4. Ran the same test command — 2 passed, 0 failed.
5. Mutation testing (see table below).
6. `cargo fmt` (reformatted the three multi-arg `debounce()` calls in the
   test module onto multiple lines — cosmetic only), `cargo clippy
   --all-targets` — clean, no warnings. `cargo build --lib` — clean, no
   dead-code warnings (everything here is `pub`, so nothing to flag yet
   even though most of it isn't wired up until later tasks).
7. Ran the full lib test suite (`cargo test --lib`, no filter) to make sure
   nothing pre-existing broke: 702 passed, 0 failed.
8. `git diff --check` — no whitespace errors.
9. Committed.

## Mutation table

| Mutation | Command | Result | Test that caught it |
|---|---|---|---|
| `debounce`: `now.saturating_sub(last) > window` → `>= window` | `cargo test --lib channel:: -- --test-threads=1` | FAILED (1 passed, 1 failed) — the boundary assertion `!debounce(Some(10s), 40s, 30s)` panicked | `debounce_suppresses_only_inside_the_window` |
| `worth_retrying`: `matches!(self, ChannelError::Unreachable)` → `!matches!(...)` | `cargo test --lib channel:: -- --test-threads=1` | FAILED (1 passed, 1 failed) — `ChannelError::Unreachable.worth_retrying()` panicked | `bad_token_is_not_retryable_but_unreachable_is` |

Both mutations were killed by the tests as written in the brief — no gaps
found, no new tests needed. Both mutations were reverted immediately after
observing the failure (verified via `git diff` / `grep` that the file
matched the pre-mutation state before re-running the green suite).

## Exact test commands and output tails

Red (before implementation):

```
$ cargo test --lib channel:: -- --test-threads=1
error[E0425]: cannot find function `debounce` in this scope
  --> src/channel/mod.rs:13:18
...
error[E0433]: cannot find type `ChannelError` in this scope
  --> src/channel/mod.rs:24:17
...
error: could not compile `dct` (lib test) due to 7 previous errors; 1 warning emitted
```

Green (after implementation):

```
$ cargo test --lib channel:: -- --test-threads=1
running 2 tests
test channel::tests::bad_token_is_not_retryable_but_unreachable_is ... ok
test channel::tests::debounce_suppresses_only_inside_the_window ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 700 filtered out; finished in 0.00s
```

Mutation 1 (`>` → `>=`):

```
$ cargo test --lib channel:: -- --test-threads=1
test channel::tests::bad_token_is_not_retryable_but_unreachable_is ... ok
test channel::tests::debounce_suppresses_only_inside_the_window ... FAILED

thread 'channel::tests::debounce_suppresses_only_inside_the_window' panicked at src/channel/mod.rs:102:9:
assertion failed: !debounce(Some(Duration::from_secs(10)), Duration::from_secs(40), w)

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 700 filtered out; finished in 0.00s
```

Mutation 2 (`matches!` → `!matches!`):

```
$ cargo test --lib channel:: -- --test-threads=1
test channel::tests::bad_token_is_not_retryable_but_unreachable_is ... FAILED
test channel::tests::debounce_suppresses_only_inside_the_window ... ok

thread 'channel::tests::bad_token_is_not_retryable_but_unreachable_is' panicked at src/channel/mod.rs:111:9:
assertion failed: ChannelError::Unreachable.worth_retrying()

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 700 filtered out; finished in 0.00s
```

Final full-suite run (post-revert, pre-commit):

```
$ cargo test --lib
test result: ok. 702 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.40s
```

`cargo clippy --all-targets`: clean, no output beyond `Finished`.
`git diff --check`: no output (no whitespace errors).

## Concerns

None. Task 1 is self-contained, builds and tests clean on its own, and
does not declare the `telegram` submodule that Task 2 will add — Task 2's
brief/implementer should add `pub mod telegram;` to `src/channel/mod.rs`
themselves when they create `src/channel/telegram.rs`.

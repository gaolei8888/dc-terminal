# Task 11 report — error-handling close-out

## Summary

All three code requirements in the brief (`backoff`, `QUEUE_CAP` drop-oldest,
`BadToken`/`Malformed` → `Broken` without retry) were **already fully
implemented and tested** by Task 5/6, with tests that are stricter than the
brief's own sketch. No production code in `src/bridge.rs` or
`src/ui/phone.rs` needed changes. The only change made in this task is to
`docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md`: the
未验证/风险 table was expanded with the eight Step-6 checks (all recorded
NOT RUN) and the list of guessed constants.

## Requirement-by-requirement mapping

### 1. `backoff(attempt) -> Duration`

The brief's proposed signature is `fn backoff(attempt: u32) -> Duration`
(stateless, indexed by attempt count). The actual code instead has:

```rust
// src/bridge.rs:1155
fn next_backoff(current: Duration) -> Duration {
    (current.saturating_mul(2)).min(MAX_BACKOFF)
}
```

a stateful-caller version: each of the three retry loops (poll/backlog-drain/
send, around lines 692–993) keeps its own `delay` variable seeded at
`INITIAL_BACKOFF = Duration::from_secs(1)` and calls `next_backoff(delay)` to
advance it. This is mathematically identical to `backoff(attempt) =
min(2^attempt, 300s)` but doesn't need an attempt counter threaded through
three different loops, and it composes directly with `sleep_or_stop` at each
call site. **Decision: keep the existing shape, do not add a duplicate
`backoff(attempt: u32)` function** — it would be dead code duplicating
`next_backoff`.

The existing test already pins the required properties, more strongly than
the brief's sketch:

```rust
// src/bridge.rs:2101 backoff_doubles_and_caps_at_five_minutes
let mut d = INITIAL_BACKOFF;               // 1s
assert_eq!(d, Duration::from_secs(1));
d = next_backoff(d); assert_eq!(d, Duration::from_secs(2));
d = next_backoff(d); assert_eq!(d, Duration::from_secs(4));
for _ in 0..20 { d = next_backoff(d); }
assert_eq!(d, MAX_BACKOFF);                // 300s, doesn't overshoot
```

This is equivalent to the brief's `backoff(0) < backoff(1) < backoff(2)` and
`backoff(99) == 300s` (20 further doublings from 4s vastly overshoots what
99 attempts would produce, so the cap assertion is at least as strong).
Ran unchanged: `cargo test --lib bridge::tests::backoff_doubles_and_caps_at_five_minutes -- --test-threads=1` → **ok**.

### 2. `QUEUE_CAP` bounded, drop-oldest

```rust
// src/bridge.rs:60
pub const QUEUE_CAP: usize = 32;
// src/bridge.rs:660
pub fn enqueue(&self, e: Event) {
    let mut q = recover(self.outbound.lock());
    if q.len() >= QUEUE_CAP { q.pop_front(); }
    q.push_back(e);
}
pub fn queued(&self) -> Vec<Event> { ... }
```

Both `enqueue` and `queued` are already `pub`. Existing tests
(`enqueue_keeps_everything_under_the_cap`, and
`enqueue_drops_the_oldest_when_the_queue_is_full` at line 1677) push
`QUEUE_CAP` events then one more, and assert `sessions.len() == QUEUE_CAP`,
`sessions[0] == 1` (oldest/session-0 gone), `sessions.last() ==
QUEUE_CAP as u32` (newest kept) — this is strictly more than the brief's
`assert_eq!(b.queued(), QUEUE_CAP)`. Ran unchanged, both pass.

### 3. `BadToken`/`Malformed` never retried

```rust
// src/channel/mod.rs:35
pub fn worth_retrying(self) -> bool {
    matches!(self, ChannelError::Unreachable)
}
```

All three retry loops in `bridge.rs` match `Err(e) if e.worth_retrying()`
to keep backing off; any other error (i.e. `BadToken` or `Malformed`) falls
through to:

```rust
// src/bridge.rs:715
recover(self.phone.lock()).state = PhoneState::Broken(broken_message(e));
```

```rust
// src/bridge.rs:1169
ChannelError::BadToken => "手机通知的令牌不能用了，去设置页重新粘贴一遍".to_string(),
ChannelError::Malformed => "手机通知收到了读不懂的数据，去设置页重新连一下".to_string(),
```

The brief's suggested message was "令牌被撤销了，按 Enter 重填"; the
actual message ("手机通知的令牌不能用了，去设置页重新粘贴一遍") differs
in wording but satisfies the same constraints: plain Chinese, no jargon, no
raw error, gives the concrete next step (go to settings, re-paste).
`src/ui/phone.rs` doesn't render this string directly for `Broken` (by
documented design the inner string is never read for UI display — see the
comment at `src/ui/phone.rs:25`); instead it renders a fixed i18n pair:
`PhoneBrokenLine` = "手机通知这会儿连不上" / `PhoneNextStepBroken` = "按
Enter 重新粘贴一遍令牌" (`src/i18n.rs:379,394`). That also gives a concrete
next step. Existing tests confirm this holds: `broken_message_is_prose_not_a_debug_dump`,
`run_populates_bot_then_pairs_then_stops_on_bad_token`, and two more BadToken
tests around lines 2248–2360 (stop-on-BadToken, no further polling after
Broken). All pass unchanged. Verified this was Task 5's finding and still
holds — no regression.

## Mutations

Both run manually (edit → test → revert), not left in the tree:

1. **Remove the `.min(MAX_BACKOFF)` cap** from `next_backoff`:
   `cargo test --lib bridge::tests::backoff_doubles_and_caps_at_five_minutes`
   → **FAILED** as required (`left: 4194304s, right: 300s`). Reverted.

2. **Make the queue unbounded** (drop the `if q.len() >= QUEUE_CAP { pop_front() }`
   guard in `enqueue`):
   `cargo test --lib bridge::tests::enqueue_` → `enqueue_drops_the_oldest_when_the_queue_is_full`
   **FAILED** as required (`left: 33, right: 32`). Reverted.

Both mutations were caught by the pre-existing tests exactly as the brief
predicted; no new test needed to pin either property, and no code changes
were left in place (`git diff` on `src/bridge.rs` is empty after this task).

## Spec table changes

Edited `docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md`,
appending two new sections after the existing 未验证/风险 table:

- **Task 11 Step 6 端到端实测清单——全部未跑**: all eight checks from the
  brief, each with what to do / what to expect / why it matters / status
  (all marked **未跑**). Check 7 (stranger message → nothing happens) is
  flagged as the most important — the only end-to-end confirmation of the
  `Rejected => {}` security boundary. Check 8 (kill+restart daemon, reply to
  stale message → "session is gone", nothing typed anywhere) is flagged as
  second most important.
- **从未在真实使用下验证过的常量**: `DEBOUNCE_WINDOW` (30s), `QUEUE_CAP`
  (32), `PENDING_OPTIONS_TTL` (300s), `MSG_MAP_CAP` (256), `OPTION_MAX_CHARS`
  (24), `OPTIONS_MAX_CANDIDATES` (6), and the 15s/8s model timeouts — each
  with a one-line note on what's unvalidated about it.

## Step 6 — explicitly not run

Step 6 (the live end-to-end test against a real Telegram bot) was **not
run**. The user has no bot token available in this environment and asked to
supply one later; there is no interactive terminal here either. Per the
project rule (没跑过的一律记成没跑过), this is recorded as **NOT RUN** in
the spec table above, with concrete instructions for each of the eight
checks so the user can execute them by hand once a token is available. The
green unit-test suite is not treated as a substitute — none of it has ever
called the real Telegram `getUpdates`/`sendMessage` endpoints.

## Verification run

- `cargo fmt --check`: clean.
- `cargo clippy --all-targets`: clean, no warnings.
- `cargo test --lib bridge:: -- --test-threads=1`: **100 passed, 0 failed**.
- Full suite (`cargo test -- --test-threads=1`) started in background,
  logging to `/tmp/dct-task11-logs/full_suite.log`; not waited on per
  instructions. Baseline was 885 passing at 52e154f; no test was removed or
  weakened by this task (no production code changed), so it is expected to
  match baseline.

## Files touched

- `docs/superpowers/specs/2026-08-10-dct-phone-channel-design.md` (spec
  table expansion — the only content change)
- `src/bridge.rs`, `src/ui/phone.rs`: read and verified, **no changes**
  (temporary mutation edits were made and reverted during Step 5, not
  committed)

# Task 8 report: `dct procedure approve | ticket`

**Status:** DONE

**Commit:** 3e9b896 — "feat(procedure): approve a procedure's tiers with Touch ID and sign tickets for it"

**Files:**
- Created `src/procedures.rs`
- Modified `src/lib.rs` (`pub mod procedures;`)
- Modified `src/main.rs` (`Some("procedure") => std::process::exit(dct::procedures::run_cli(&args[1..])),`)

## What was implemented

Followed the brief's reference code closely, with one required correction against
the real `dct-brain` interface (the brief itself flagged this risk):

- **Bug fixed vs. the brief's reference code:** `dct_brain::approval::verify_approval`
  takes 4 arguments — `(sa, trusted, procedure: &str, steps_sha256: &str)` — and
  returns `VerifyError::WrongProcedure` on a procedure-name mismatch. The brief's
  pasted `ticket()` body called it with only 3 args
  (`verify_approval(&sa, &keys.trusted()?, &steps_sha)`), which would not compile.
  Fixed to `verify_approval(&sa, &keys.trusted()?, full_name, &steps_sha)`.

Everything else (`Dcv` trait, `RealDcv`, `Shown::parse`, `Approvals`, `approve()`,
`ticket()`, `run_cli()`, and the test module) matches the brief's reference code and
compiled/passed unmodified once the above fix was applied. Verified against the real
source (not just the brief) for: `SignError::Chip(i32)` (propagated via its `Display`
impl through `.map_err(|e| anyhow!("{e}"))`, no special-casing needed), `Tier`/`SignerRole`
serde renames used by `parse_tier`, `KeyStore::{at, default_dir, trusted, signer}`,
`SecureEnclave` trait shape, and `Ticket`/`Subject`/`params_sha256` field shapes.

The `(4, Tier::SelfOnly)` override test (`a_self_only_procedure_gets_an_automatic_ticket_without_touch_id`)
passed as written — no deviation needed there.

## Step 6 (manual end-to-end): NOT DONE

Per the brief and the earlier tasks' notes, dcv v2's `show`/`approve` isn't implemented
yet — there is no real `dcv` binary to shell out to. `RealDcv` (which calls the `dcv`
CLI) is implemented but untested beyond compiling; all functional tests use `FakeDcv`.
This step is explicitly deferred until dcv v2 lands, as instructed.

## Verification

- `cargo test --lib procedures::` — 10/10 pass.
- `cargo test --workspace` — full green: dct lib 1382 passed, dct integration tests
  all passed, dct-brain 43 passed, dct-link 13 passed, dct-page 10 passed, dct-srv
  72+3 passed. 0 failed anywhere.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean, exit 0, no warnings.
- `cargo check --target x86_64-pc-windows-msvc --all-targets` — exit 0. One pre-existing
  unrelated warning in `src/student_projects.rs:669` (unused variable `path` in
  `sync_dir`), not touched by this task.
- `git diff --check` — clean.
- `devices/esp32-screen/` left untouched/unstaged, as instructed.

## Concerns / carried-forward notes for delivery (from the brief's "交付时要告诉用户的")

1. Automatic key protection is one notch weaker than the design intends (no Apple
   dev cert yet): same-account agents can sign `self`/`physical` tier tickets;
   outward/money tickets are unaffected (still require Touch ID at the point of signing).
2. End-to-end with real dcv v2 is unverified — see Step 6 above.
3. The judge module is still not built (blocked on DC gateway key); tiering here is
   rule-based only (`tier_for_step`).
4. dco-side work (verify tickets with `sign::verify`, track nonces, apply `tier::raise`
   as a runtime floor, and switch its own `Tier` enum to dct-brain's, which adds
   `physical`) is out of scope for this task and needs to be communicated to the
   dc-octo session.

## Fix round 1

Code review on commit 3e9b896 came back "spec OK, quality needs fixes." Implemented
all six rulings in `src/procedures.rs` (commit 3827849):

1. **Money cannot be lowered.** `approve()` now compares each override against the
   *proposed* tier (`tier_for_step`) before applying it: if the proposed tier is
   `Money` and the override is lower, it refuses with `"第N步涉及付钱，不能改低"`
   (no dcv call, no signature happens first). Other lowering (e.g. Content → SelfOnly)
   still works. Rewrote `approval_reason()` to take both the proposed and final tier
   vectors plus the override list: every overridden step is now named as
   `"第N步「arg摘要」原为X，改为Y"`, using the accurate tier word for X and Y
   (`tier_zh` covers all five tiers). Non-overridden steps at Physical/Content/Money
   are still named as `"第N步「arg」是X"`. The old blanket "其余只影响自己" catch-all
   was replaced with `"其余只读或只影响自己"`, which is now an accurate claim — Physical/
   Content/Money steps are always itemized above it (overridden or not), so anything
   left in the catch-all really is Read or SelfOnly. Added `arg_summary()` to truncate
   long args in the prompt.

2. **Time arithmetic.** `ticket()` now rejects `start_in > 2_592_000` (30 days) and
   `window > 86_400` (24h) up front with plain-Chinese errors, before touching dcv.
   `earliest`/`expires` are computed with `checked_add`, erroring in Chinese on
   overflow instead of wrapping. The ticket's Touch ID reason now states the validity
   period: `"从现在起 N 秒内有效"` when `start_in == 0`, or `"N 秒后开始，此后 M 秒内有效"`
   otherwise.

3. **`Shown::parse` hardening.** Step `n` is now converted with `try_into` and errors
   in Chinese ("步骤序号 {n} 超出范围") instead of silently truncating via `as u32`.
   A missing `arg` is now a hard error (was previously defaulted to `""`). After
   building all steps, a pass over their `n` values rejects duplicates. The "override
   names a step that doesn't exist" check was already present in the brief's code
   (`.ok_or_else` in the override loop) and is now covered by an explicit test.

4. **CLI.** `approve` now re-reads the procedure via `Shown::parse`/`RealDcv::show`
   after a successful approval so it can print `"第{n}步：…"` keyed by the step's real
   `n`, not its array index. The parameter-mismatch error in `ticket()` now goes
   through a new `join_names()` helper producing a plain `、`-joined list instead of
   `BTreeSet`'s `{:?}` debug syntax. A repeated `--param` is now a hard error
   ("参数 {k} 给了不止一次") instead of silently overwriting. `dcv.show`/`dcv.approve`
   calls in `approve()`/`ticket()`, and the filesystem calls in `Approvals::save`/`load`,
   are now wrapped with `.with_context(...)` giving a Chinese description of what
   failed and which path/procedure was involved.

5. **Approvals directory permissions.** `Approvals::save` now calls
   `crate::sys::fs::restrict_dir_to_owner` on the approvals directory (0700 on Unix)
   after `create_dir_all`, and writes the approval file through
   `crate::sys::fs::create_private` (0600) instead of `std::fs::write`, reusing the
   same helpers the keystore already uses for its own files — no new permission logic.

6. **New tests** (11 added, 21 total in the module, all passing):
   `a_positive_start_in_pushes_earliest_and_expires_forward`,
   `a_procedure_changed_after_approval_gets_no_ticket`,
   `an_approval_for_one_procedure_does_not_authorize_another_with_the_same_steps`,
   `an_override_of_a_nonexistent_step_is_an_error`,
   `lowering_a_money_step_is_refused`, `raising_a_money_step_still_works`,
   `the_prompt_names_overridden_steps`, `window_and_start_in_over_their_caps_are_rejected`,
   plus three direct `Shown::parse` hardening tests
   (`shown_parse_rejects_a_step_number_outside_u32`,
   `shown_parse_rejects_a_missing_arg`, `shown_parse_rejects_duplicate_step_numbers`).
   The existing wrong-params test also now asserts the error string contains no `{`
   (i.e. no Rust debug-set syntax).

Replaying old approvals was explicitly parked by the controller and not touched.

### Verification (fix round 1)

- `cargo test --lib procedures::` — 21/21 pass.
- `cargo test --workspace` — full green: dct lib 1393 passed (was 1382; +11 new tests),
  all dct integration test binaries pass, dct-brain 43, dct-link 13, dct-page 10,
  dct-srv 72+3. 0 failed anywhere.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean, exit 0.
- `git diff --check` — clean.
- `devices/esp32-screen/` left untouched/unstaged.

### Concerns (fix round 1)

- The money-lowering guard compares against `tier_for_step`'s *proposed* tier, not
  against any previously-saved approval's tier — there's no "ratchet" across repeated
  `approve` calls for the same procedure today (a re-approval starts fresh from the
  proposed tiers each time). This matches the brief's model (approvals aren't
  versioned/diffed against each other) and the controller's ruling only speaks to a
  single approve() call's overrides, but it's worth flagging: if dcv's proposed tier
  for a step can itself change between approvals (unlikely, since `tier_for_step` is
  a pure function of action/arg, and any change to arg text changes `steps_sha256`
  and voids the old approval anyway), this guard alone doesn't chase it.
- Chinese error/prompt strings were written to match the ruling's example wording as
  closely as possible; exact phrasing beyond the mandated examples (e.g. the ticket
  validity sentence, context-wrap prefixes) is my own wording, not dictated by the
  brief or the ruling — flagging in case the controller wants a specific phrasing.

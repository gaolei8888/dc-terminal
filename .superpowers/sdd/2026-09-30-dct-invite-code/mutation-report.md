# Mutation pass and acceptance — dct invite code (2026-09-30)

Executor: executing-plans, inline, worktree ../dc-terminal-invite, branch feat/dct-invite-code.

## Mutants

Command: `python3 mutants.py` (plan Task 10 Step 1 script, run from the worktree root; saved in the session scratchpad instead of /tmp).

```
KILLED M1 T 不含 group
KILLED M2 rec 不含 kx_pub
KILLED M6 拒绝采样 < 改 <=
KILLED M8 常数时间改 ==
KILLED M9 算不出 SPAKE2 不作废
KILLED M10 InviteFinish 不查 peer
KILLED M11 deadline > 改 >=
KILLED M12 expires >= 改 >
KILLED M13 不查 endpoint==from
KILLED M15 不查已在组里
KILLED M16 不编号
KILLED M17 B 不查 Open 的端点
KILLED M18 B 不验 cA
KILLED M19 B 不查组
KILLED M20 B 不查我的钥匙
KILLED M21 --name 先落盘
KILLED M22 广播也发给 B
KILLED M23 不限 3 台
KILLED M24 >16 改 >=16
KILLED M25 连按 a 不拦
KILLED M26 别处的码也报
KILLED M27 CLI 不按 id 认结果
KILLED M28 Debug 露码
KILLED M29 journal 记码
KILLED M30 InFlight 期间探问仍答 Open
```

25/25 KILLED, 0 SURVIVED, 0 SKIP, 0 COMPILE-ERROR. Worktree clean afterwards. Tests added for survivors: none.

## Acceptance (Step 3)

- `cargo test --workspace --no-fail-fast -- --test-threads=4`: 1954 passed, 1 failed — session::tests::recovering_from_a_failure_after_real_input_still_does_not_count (known flaky under load); rerun alone: ok, 1 passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: Finished, no warnings.
- `cargo check --workspace --all-targets --target x86_64-pc-windows-msvc`: Finished (target installed this session); only the pre-existing `unused variable: path` warning.
- `cargo test --test mesh_e2e -- --ignored --nocapture`: two_computers_join_and_leave_a_message_over_a_real_relay ... ok (2.84s).
- `cargo tree -p dct-mesh -e normal | grep -E "spake2|getrandom"`: spake2 v0.4.0, and getrandom v0.2.17 via rand_core (already at base through p256/x25519-dalek; not spake2's own feature — see ledger Task 1 ruling).
- `git log docs/dct-invite-code..HEAD`: 9 commits, Task 1–9.

## Flaky reruns during the run

- Task 2, final: session::tests::recovering_from_a_failure_after_real_input_still_does_not_count
- Task 3, Task 6: pty::tests::history_is_capped_at_the_configured_size, pty::tests::keeps_history_that_scrolled_off_the_screen
- Task 4: gate::tests::project_api_create_upload_save_and_no_overwrite (not on the plan's known list; src/gate.rs untouched; alone 3/3 pass)

All passed when rerun alone.

## Not done here

- Step 5 real-computer acceptance (Mac + Windows, same DC account): needs the user.
- dct invite Ctrl-C withdrawal: verified only through the code path/unit tests, not live.

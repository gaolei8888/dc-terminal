# dct voice relay v0 - report

Branch: feat/voice-relay (from main). Status: DONE, with concerns below.

## Commits
- da627f5 feat(voice): dct voice relays dictated speech ... (includes the spec file, src/voice/{mod,tests}.rs, main.rs/lib.rs wiring, SessionInfo.last_active_ms)
- (second) test(proto): pin SessionInfo.last_active_ms ... no protocol bump

## What was built
- `dct voice` (src/voice/mod.rs): pure functions (fix_aliases, is_bare_confirmation, is_cancel, waiting_for_approval, route, parse_hear, outgoing) + `Relay` loop over traits Hearer/Daemon/Clock/Out/Log (fakes in tests). cfg(unix) adapters: DcoClient `hear` via `DcoClient::call` (30 s timeout, longer than wait_ms 20 s), daemon via `Client` + `Request::List` / `Request::Input` (text, then "" for Enter, same two-step as bridge::submit), banner via `show_status_with("think", text)`, log ~/.dct/voice.log at 0600. Non-unix prints a Mac-only message, exit 1.
- Read-only `SessionInfo.last_active_ms` (#[serde(default)], refresh rules untouched). No PROTOCOL_VERSION bump (same exception as `tag`; proto test comment and expectation updated). Required adding `last_active_ms: 0` to ~35 test struct literals.

## Session state blocked
`SessionState::Asking` only (the existing "agent needs the user to answer" state). Checked twice: when deciding, and again after the 2 s window right before typing (the prompt can appear during the window). Other states are not blocked: Working, Idle, Failed, Unknown. Stopped sessions are not sent to (no_session). NOTE: nothing in session.rs currently *sets* Asking (the enum comment says a future Bridge will), and the claude/codex/qwen profiles run in yolo/skip-permission mode, so today the gate is armed but rarely fires. If a future classifier marks approval prompts as another state, `waiting_for_approval()` is the single place to change.

## Decisions beyond the spec (please review)
- Only `is_agent` sessions are ever targets. A plain shell session would execute the spoken text as a command.
- Confirmation words are checked on the corrected text and again on the text left after the routing prefix ("给X说好" is blocked). Extra confirm words: 好的 好吧 嗯嗯 是的 对 确定 批准 okay yeah sure; repeated single chars (嗯嗯嗯). Not blocked: "继续" alone.
- Stray "取消" with nothing pending is dropped (logged cancelled/nothing_pending), never sent.
- Rate limit action is logged as `blocked_confirm` with reason `rate_limited` (spec's action list has no entry for it). Utterances arriving in the cancel window are queued, not lost; after a send they hit the rate limit and are dropped with a printed notice.
- Start-up backlog: first `hear(0, 0)` is read and discarded (prints how many were skipped) so old speech is never replayed.
- Alias "whole word": CJK has no word boundaries. An ASCII-alphanumeric neighbour blocks replacement; a CJK neighbour blocks unless it is in a small closed set of function characters (before: 给对跟问叫让和同找请诉; after: 说啊呀吧呢你帮请来去能可把的在是). So "张瑜伽" is untouched, "给张瑜说" / "张瑜你好" are fixed.
- The spec's voice.toml example `章鱼 = [...]` is invalid TOML (bare keys must be ASCII). The loader quotes non-ASCII bare keys before parsing so the documented format works.
- Missing confidence in a dco utterance counts as 0 (blocked).
- dco's `hear` argument/field names (`since_seq`, `wait_ms`, `utterances[].seq/text/confidence`) are taken from the spec; not verified against a live dco.

## Mutation checks (each reverted afterwards; all five made tests fail, as required)
1. approval predicate -> false: 4 failed (session_waiting_for_approval_gets_nothing, approval_prompt_appearing_during_cancel_window_still_blocks, explicit_target_waiting_is_blocked_even_if_default_is_free, only_asking_state_counts...)
2. confidence gate off: 1 failed (low_confidence_is_blocked_at_the_edge)
3. is_bare_confirmation always false: 3 failed (bare_confirmation_words_are_detected, bare_confirmations_are_never_sent, confirmation_hidden_behind_a_routing_prefix...). (Disabling only the first of the two call sites passes, because the post-route check also catches it; that redundancy is intentional.)
4. CANCEL_WINDOW_MS = 0: 5 failed (cancel_inside_window..., cancel_right_at_window_end..., nothing_is_sent_before_the_window_ends, low_confidence_cancel_still_cancels, approval_prompt_appearing_during_cancel_window...)
5. default route = first session: 2 failed (default_route_is_most_recently_active, unknown_name_falls_back_to_default_and_keeps_text)

## Verification
- `cargo +1.99.0 test --workspace --locked` x2: 2450 passed, 0 failed both times (an earlier run failed only proto::tests::the_session_info_shape_is_pinned_too, which was the intended pin I then updated). No flaky tests seen.
- `cargo +1.99.0 clippy --workspace --all-targets --locked -- -D warnings`: clean.
- rustfmt not installed for 1.99.0, so not run. Non-unix build not compiled here (cfg split follows src/game/scene.rs).
- Real dco / daemon were not touched; no restart.

## Concerns
- The running daemon is old: it does not send `last_active_ms`, so every session reads 0 and the default route falls back to the highest session id until the daemon is restarted (controller's call; I did not restart). Acceptance step 1 needs the new daemon.
- Acceptance step 3 (say "同意" on a waiting session) is blocked by the bare-confirmation gate regardless; the state gate cannot be exercised on a real session until something sets Asking.
- dco `hear` / `show_status` field names unverified against the real dco.

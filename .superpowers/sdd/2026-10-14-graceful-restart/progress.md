# SDD ledger — plan: docs/superpowers/plans/2026-10-14-graceful-restart.md

Worktree: .worktrees/graceful-restart, branch feat/graceful-restart, base cd4cbc5 (main).
Spec: docs/superpowers/specs/2026-10-14-graceful-restart-design.md

## Preflight scan

| Pair / task | Produces → consumes | Finding |
|---|---|---|
| T1→T2 | fill_id, resumes_by_id, session_id_args/resume_id_args | consistent |
| T2→T6,T9 | RecordedSession.{conversation_id,interrupted,handoff}, persist_last_sessions_with, conversation_of | consistent |
| T2 internal | test "no-id record falls back: non-continue one gets new --session-id" vs impl (resume=false & resumes_by_id → new id) | consistent |
| T4→T6 | Drain/Action/Progress/NUDGE_TEXT/handoff_text; Member.resumable needs SessionManager::resumes_by_id(id) | T6 adds it; ok |
| T4→T8 | Progress (serde) | consistent |
| T5→T6,T9 | set_draining/is_draining, submit_unchecked | consistent |
| T6→T7 | T6 real DrainHooks calls client::spawn_swap, which T7 creates | ORDER CONFLICT |
| T6→T8 | DrainStatus{renew}, DrainCancel, DrainProgress | consistent |
| T7 internal | swap_daemon/wait_old_gone/spawn_swap + main __swap | consistent |
| T8 internal | parse_restart/progress_line/plan_for | consistent |
| T9 internal | note_for/step/Note; uses submit_unchecked | consistent |
| T3 | needs real qwen + network | may be BLOCKED by environment; then leave qwen undeclared |
| T10 | real-machine run with isolated socket | controller supervises |

Ruling: execute Task 7 before Task 6 — T6's real hook needs client::spawn_swap from T7; T7 is self-contained — cost if wrong: none, order only.
Ruling: plan reference code is non-authoritative (repo memory); implementers verify against source and report deviations — cost if wrong: n/a.

## Progress
Task 1: dispatched (BASE cd4cbc5, implementer sonnet)
Task 1: complete (commits cd4cbc5..e310b5b, review clean)
Task 1: minor (deferred): unverified_profiles test comment goes stale after Task 3
Task 2: dispatched (BASE e310b5b, implementer opus)
Task 2: implementer done ae1b857; review dispatched (opus)
Task 2: review approved; ⚠️ confirmed real gap: spec §1 resume-by-id failure fallback + user warning not covered by any task → fix round 1 (also: only persist id after first submitted turn; continue_flags filter by profile resumes_by_id)
Ruling: record conversation_id only after the session's first submitted turn — an untouched claude session has no transcript, so --resume <id> would fail; starting it fresh loses nothing — cost if wrong: a session whose only activity was typing without Enter restarts fresh (it had no conversation anyway).
Task 2: fix round 1 implemented (commits ae1b857..02aa15b); re-review dispatched
Task 3: dispatched (BASE 02aa15b, sonnet) in parallel with Task 2 re-review (read-only)
Task 2: fix round 1/5 (3 addressed, 0 open; commits ae1b857..02aa15b)
Task 2: complete (commits e310b5b..02aa15b, review clean)
Task 2: minor (deferred): resume watcher reacts only to process exit, not Failed screen state (deliberate)
Task 2: minor (deferred): user /exit within 20s of a by-id resume triggers a false "conversation gone" warning + fresh session
Task 2: minor (deferred): ConversationGone changes wire format; PROTOCOL_VERSION bump must land with Task 6 on this branch
Task 3: implementer done b2359f4 (qwen 0.24.6 verified headless -p via dc gateway; interactive untested); review dispatched (sonnet)
Task 3: controller removed stray test memory ~/.qwen/memories/user/secret-passphrase.md + its MEMORY.md line (created by the qwen test)
Task 4: dispatched (BASE b2359f4, sonnet) concurrently with Task 3 review (read-only)
Task 3: complete (commits 02aa15b..b2359f4, review clean)
Task 3: minor (deferred): profiles/dc.toml header comment still says resume args unverified — reword
Task 3: minor (deferred): Task 10 must cover interactive qwen resume-by-id (only -p tested); qwen global auto-memory can pollute recall controls
Task 4: implementer done 96a1d37; review dispatched
Ruling: handoff-writing sessions are not nudged at the wait deadline and are marked interrupted if unfinished at the hard deadline — interrupted only triggers "continue" for by-id-resumed sessions, which unresumable ones never are, so it is harmless; they still get read-handoff if the file exists — cost if wrong: none observable.
Task 5: dispatched (BASE 96a1d37, sonnet); flagged voice path bypasses bridge::submit — implementer to choose gating
Task 4: review approved; spec ❌ minor: initiator enters interrupted only if in members. Plus minors: AskHandoff sent to Stopped/Failed/missing sessions; same-step deadline marks just-asked idle session interrupted; untested handoff-writer-at-deadline path; "(Review Focus 5)" comment; lib.rs mod order.
Ruling: run Task 4 fix round 1 after Task 5 finishes (avoid two implementers committing concurrently in one worktree) — fold in: initiator always interrupted; no AskHandoff for Stopped/Failed/missing; tests for both + handoff-writer-at-deadline; comment fix — cost if wrong: short delay only.
Task 5: implementer done a9dc316 (new Request::Submit for voice; web routes left ungated as user's own view); review dispatched
Ruling: web phone page (web/routes.rs Input/Key) stays ungated — it is the user's own remote typing, spec says user's own typing is not blocked — cost if wrong: a remote page could type during drain, which only delays drain.
Ruling: carry into Task 6 — voice must fall back to Input+Input("") when daemon protocol < 25 (installed binary + old daemon would otherwise break voice until restart) — cost if wrong: voice broken between install and first restart.
Task 5: complete (commits 96a1d37..a9dc316, review clean)
Task 5: minor (deferred): bridge.rs test doc comment now above wrong fn (draining_cat_manager inserted); duplicated Profile literal in test helper
Task 5: minor (deferred): submit_unchecked test asserts only Ok; no test for voice RestartPending mapping nor daemon-level Request::Submit; zh sentence duplicated in bridge.rs + i18n (add equality test)
Task 5: minor (deferred): proto shape-pin test must flip to 25 in Task 6
Task 4: fix round 1 implemented e88dca6; re-review dispatched; Task 7 dispatched (BASE e88dca6, sonnet)
Task 4: fix round 1/5 (5 addressed, 0 open; commits a9dc316..e88dca6)
Task 4: complete (commits b2359f4..96a1d37 + e88dca6, review clean)
Task 4: minor (deferred): lib.rs mod order cosmetic
Task 7: implementer done 8600e1e; review dispatched; Task 6 dispatched (BASE 8600e1e, opus)
Task 7: complete (commits e88dca6..8600e1e, review clean)
Task 7: minor (deferred): main.rs __swap arm splits the daemon arm's comment; log when old daemon had to be force-killed
Task 6: implementer done 0ded584; review dispatched (opus)
Ruling: add `active: bool` to Response::DrainProgress (still protocol 25) so the CLI can tell "cancelled/not draining" from "draining, nobody left" — folded into Task 6 fix round — cost if wrong: one extra field.
Task 6: review approved; Important: DrainProgress can't distinguish cancelled vs draining-with-empty-wait → fix round 1 (active field, banner during finishing, drop redundant sleep)
Task 6: minor (deferred): UI poll_drain blocking socket call on render thread once/s (matches live::poll_status pattern)
Task 6: fix round 1 implemented 8497b23; re-review dispatched
Task 8: dispatched (BASE 8497b23, opus) concurrently with Task 6 re-review (read-only)
Task 6: fix round 1/5 (3 addressed, 0 open; commits 0ded584..8497b23)
Task 6: complete (commits 8600e1e..8497b23, review clean)
Task 8: implementer done a534af4; review dispatched (opus); Task 9 dispatched (BASE a534af4, sonnet)
Task 8: note: CLI waits 20s for new daemon vs swapper worst case ~22s → raise CLI wait in fix round
Task 8: note: 3 Windows-only clippy warnings in untouched files (llm/cli.rs, student_projects.rs, game/identify.rs) — check whether pre-existing on main at final review
Task 8: complete (commits 8497b23..a534af4, review clean)
Task 8: minor (deferred, fix in final wave): raise CLI new-daemon wait 20s → 30s (swapper worst case ~22s)
Task 8: minor (deferred, fix in final wave): Ctrl+C silently swallowed during swap wait — print cancel-too-late once
Task 8: minor (deferred): debug-dump eprintln "守护进程答非所问" (copies existing pattern)
Task 8: minor (deferred, fix in final wave): unreadable old pid ⇒ premature RestartDone; swap-wait polls every 50ms with 1-2 new connections → 250ms
Task 8: minor (deferred): Hello timeout on a current daemon prints "后台太旧" wording
Task 9: implementer done 6b6862f; review dispatched (sonnet)
Task 9: complete (commits a534af4..6b6862f, review clean)
Task 9: minor (deferred, fix in final wave): submit_unchecked retry after typed-but-Enter-failed can type the note twice — don't retry after a partial send
Task 9: minor (deferred): note may be delivered before watcher window closes (harmless; add comment); profiles without idle_pattern (opencode) always get the dropped-note warning
Task 10: dispatched (BASE 6b6862f, opus) — docs + isolated real-machine acceptance
Task 10: implementer done 7d2a50d (DONE_WITH_CONCERNS) — acceptance: 1,2,4,5 PASS; 3 codeword resume + continue PASS, codex "read handoff" FAIL intermittent (1/3 nothing, 1/3 Enter lost); 6 qwen resume PASS but unknown-state qwen holds the full wait and gets a nudge
Ruling: codex post-restart note — deliver only after the session has been Idle continuously for a settle window (≥3s) and ≥N s since spawn; reuse the no-retry-after-partial rule — why: codex shows its idle prompt from the first frame while still starting — cost if wrong: notes arrive a few seconds later
Ruling: sessions whose state is Unknown (profile can't tell busy/idle, e.g. qwen) are not waited on and not nudged; they are recorded interrupted=true so they are told to continue after restart — why: dct can't observe them, waiting 18 min buys nothing — cost if wrong: a qwen mid-edit is cut off, then resumed by id and told to check its work
Ruling: --now skips handoff requests and progress lines (spec §3: --now skips 1–4); handoff filenames use local time — why: spec text / user reads them — cost if wrong: none
Ruling: Task 10 task review runs in parallel with the final whole-branch review; its findings join the single final fix wave — why: docs-only diff, final reviewer covers the same range — cost if wrong: one extra fix item
Final: Task 10 review (sonnet) + whole-branch review (opus, BASE cd4cbc5 HEAD 7d2a50d) dispatched in parallel → final-review.md
Task 10: review Spec ✅ Quality needs fixes — Important: READMEs don't say the first restart from an old (protocol < 25) daemon is a plain restart; Minor: -y row missing from table; EN paraphrases the phone/voice refusal — all folded into the final fix wave
Final review: With fixes — 1 Critical, 8 Important, 7 Minor (final-review.md)
Ruling: fix now — C1 (UI returns to board on reconnect to a new daemon instance; restore saved session numbers if cheap), I2 codex note gate, I3, I5, I6, I7, I8, I9, minors 10, 11, 13, 15, 16, plus Task 10 items and the deferred final-wave minors — why: each can mislead or lose work on the user's first real restart — cost if wrong: a larger fix diff
Ruling: park I4 (/clear or /resume inside claude leaves a stale id) — why: needs a claude hook to learn the real id; effect is an older conversation resumed, not lost work — cost if wrong: an interrupted session may be resumed into the conversation from before /clear; documented as a known limit
Ruling: park minor 12 (stale DCT_SESSION_ID) — why: rare (editor launched from a session running dct restart) — cost if wrong: one unrelated session not waited for, then told to continue
Ruling: park minor 14 (half-typed line gets the message appended) — why: spec rule "user typing is never blocked"; clearing the line would destroy user text — cost if wrong: user sees the message appended to their draft
Note: rulings above use the summary numbering; in final-review.md the parked '/clear' item is I3, and fix-now items are C1, I1, I2, I4, I5, I6, I7, I8 + Minor list items except the stale-DCT_SESSION_ID and half-typed-line ones
Final fix wave: brief final-fix-brief.md, BASE 7d2a50d, dispatched (opus)
Final fix wave: done 5d14d0e 477befc (DONE_WITH_CONCERNS: Windows handoff names UTC; ~2s mesh loss window; agents that never leave canonical mode get no notes; codex re-run pending); scoped re-review + isolated codex handoff re-run dispatched
Final re-review: Needs fixes — C1 partial (N1: guard can't trip on first restart from old daemon), all others addressed; new N1 Important, N2–N8 Minor (final-rereview.md)
Ruling: residual pass (small, after the acceptance re-run finishes so the binary under test isn't rebuilt mid-run): fix N1 (trip on unknown→known instance change + tell user to close open dct windows before the first restart, in RestartOldDaemonPlain text and both READMEs), N3 (cap wait for never-ready idle unresumable sessions at 60 s), N4 (check "never submitted" before profile), N5 (cancel-note warning wording), N6 (raise HeldMessageDropped), N7 (misplaced doc comments), N8 (README: first restart restores automatically with new numbers; keep --continue for the newest-active id-less sibling) — why: N1 is the one path that can type into the wrong session on the user's very first restart — cost if wrong: one more small diff
Ruling: park N2 (drain lock held while typing can freeze the UI ~seconds during a drain) — why: only during drain, cosmetic stall — cost if wrong: UI stutters while dct types into sessions before a restart
Final acceptance re-run: note delivery PASS 5/5 when the handoff file existed; full codex flow FAIL 12/16 — codex v0.163 busy signal unreliable ("esc to interrupt" hidden for seconds or whole turns), so drain thinks the handoff is done ~3 s after asking; new daemon checks for the handoff file once at startup; --now PASS (final-acceptance.md)
Ruling: residual pass also fixes codex: (a) a handoff request counts as answered only when the handoff file exists (or the wait deadline passes), not on idle alone; (b) idle must hold ≥3 s before a busy session counts as finished; (c) after restart check for the handoff file when the note is due (retry until the 2-min note window ends), not once at startup; (d) re-measure codex v0.163 busy indicators (inline --no-alt-screen and default) and widen busy_pattern (e.g. "tab to queue") with a dated comment — why: codex is the only unresumable agent the user runs; without this its handoff is lost most of the time — cost if wrong: drains wait longer for codex
Residual pass dispatched (opus, BASE 477befc): residual-fix-brief.md
Residual pass: done 9e91a04 33092d2 ca2a8a7; real check 3/3 PASS; controller verified cargo test (1944 + suites) green after a stale-build rerun, clippy 1.99 clean. Branch complete.

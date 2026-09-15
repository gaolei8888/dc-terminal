# SDD ledger — plan: docs/superpowers/plans/2026-09-13-public-live-listing.md

Spec: docs/superpowers/specs/2026-09-13-public-live-listing-design.md
Worktree: .claude/worktrees/public-live on branch feat/public-live-listing (base 8e69a54)
Baseline: env -u TERM cargo test --workspace --locked --no-fail-fast → 1437 passed, 0 failed
Note: .superpowers/sdd/.gitignore is `*` in HEAD; existing SDD dirs are force-added. At finish: `git add -f` this dir (repo convention: keep brief/report/progress, do NOT delete).

## Pre-flight scan

| Pair / task | Produces → consumes | Finding |
|---|---|---|
| T1 → T3 | publish_grant, push_hash, MAX_PUBLIC_TITLE_CHARS, RESERVED_LIVE_ID | consistent |
| T1 → T7 | public_path, publish_grant, push_hash, MAX_PUBLIC_TITLE_CHARS | consistent |
| T2 → T3 | keys::PublishKeys{keys, blocked}, add(name, now), name_for, is_blocked | consistent (T3 reconcile reads pub field `keys`) |
| T2 → T4 | KeyFile::open/keys/reload_if_changed | consistent |
| T2 → T5 | tempfile dev-dep for dct-srv (used by T5 scaffold in tests/serves.rs) | consistent |
| T3 → T4 | Control, publish/unpublish/public_list/reconcile; frame(id, Option, lane); lanes → (Vec, Option<String>) | consistent; T3 adapts lib.rs callers temporarily |
| T4 ↔ existing test | T4 makes `GET /` the public page on Routes::LiveOnly and moves the phone page to `/phone` | CONFLICT: `the_default_relay_does_not_answer_the_unauthenticated_routes` (cffc17b) asserts `/` is 404 on the default relay. See Ruling 1 |
| T4 → T5 | public_page_route placeholder, replaced by dct_page::public_page() | consistent |
| T4 → T9 | PUT/DELETE /live/{id}/public (x-live-grant), GET /live/public JSON [{id,title,lanes,viewers}] | consistent |
| T4 → T7 | lanes JSON `public: {title} | null` | consistent with T7 PublicTitleBody |
| T6 → T7/T8 | LivePublic, LiveInfo.public, LivePublish/LiveUnpublish/LivePublishGrant, LiveGrantToken, LivePublishKeyMissing, LIVE_PUBLISH_KEY, i18n keys | consistent |
| T7 → T8 | LiveState publish/unpublish (daemon handlers) | consistent |
| T7 → T9 | Response::LiveGrant serialized transparent → `{"LiveGrant":"<hex>"}` | consistent with T9 fake driver |
| T9 → T10 | features.publicLive, live.public, livePublicError, live-public/live-private, live-start {public,title} | consistent |
| T11 | spec deviation edit (settings page → live panel) | consistent with plan preamble |
| T1 self | tests vs code | consistent (vectors computed independently in Python) |
| T2 self | tests vs code; main.rs uses `?` on String/io errors into Box<dyn Error> | consistent |
| T3 self | tests vs code | consistent |
| T4 self | tests vs code | consistent except Ruling 1 |
| T5 self | tests vs code | consistent |
| T6 self | tests vs code | consistent |
| T7 self | pusher tests use `live.start(vec![])` + SessionManager::new() like existing tests | consistent |
| T8 self | fake_live_daemon gains 2nd arg; existing callers updated | consistent |
| T9 self | depends on Store#student and audit record shape; plan says adapt | consistent (hedged) |
| T10 self | manual acceptance only (admin.html is minified single-line JS, no automated UI test) | consistent; see Ruling 2 |

Ruling 1: In Task 4, update `the_default_relay_does_not_answer_the_unauthenticated_routes` so it asserts `/link/*` and `/phone` are 404 on the default relay and `/` serves the public page (200) — spec makes `/` the public listing in the live route group; the test's intent (no unauthenticated pairing routes by default) is preserved — cost if wrong: the phone page under --with-link moves path (dev-only), low.
Ruling 2: Task 10 has no automated test (admin.html is a minified one-liner; admin-ui.test.cjs needs Playwright which is unavailable here); accept manual verification + Task 9 backend tests — cost if wrong: an admin UI regression is caught only by eye or at final review.

## Progress
Task 1: dispatched implementer (sonnet), BASE 8e69a54
Task 1: ⚠️ resolved — commit body has no AI attribution (checked git log -1 --format=%B 63d3c11)
Task 1: minor (deferred): publish_grant .expect relies on Hmac::new_from_slice accepting any key length; comment could cite the guarantee
Task 1: complete (commits 8e69a54..63d3c11, review clean)
Task 2: dispatched implementer (sonnet), BASE 63d3c11
Task 2: minor (deferred): keys.rs save() — mode(0o600) only applies when tmp file is created; pre-existing tmp keeps looser perms (file holds digests, not keys)
Task 2: minor (deferred): keys.rs save() — fixed tmp filename; concurrent management commands can clobber each other
Task 2: minor (deferred): parse_cli `key add --file X` (name omitted) creates a key named "--file" (plan-mandated reference code)
Task 2: minor (deferred): report said all reformatted hunks were new code; get_frame test helper signature was reformatted (cosmetic)
Task 2: complete (commits 63d3c11..13edaa9, review clean)
Task 3: dispatched implementer (sonnet), BASE 13edaa9
Task 3: review — Important (plan/spec-mandated): reconcile matches revoked keys by name; revoke+re-add same name within one reload keeps leaked key's rooms public
Task 3: Ruling: store the publishing key's digest (hash hex) in `Public` alongside key_name and have reconcile keep a publication only if that digest is still in the file (via a PublishKeys method); key_name stays for logs — spec's promise "rooms published by a revoked key become private" outranks the spec/plan field sketch `{title, key_name}` — cost if wrong: one extra String per public room, none functionally
Task 3: minor (deferred): no tests pinning probe-safe check order (wrong control + blocked/empty title/None keys must all be 401)
Task 3: minor (deferred): public_list viewers-desc sort untested (single-room tests)
Task 3: minor (deferred): viewer sum duplicated in public_list and viewers()
Task 3: minor (deferred): removed doc line explaining push_secret cannot read (authed)
Task 3: minor (deferred): implementer used shared git stash in worktree (process)
Task 3: ⚠️ resolved — 429 limit, HTTP routes, lanes `public` field, reload→reconcile wiring are Task 4 scope (not gaps in Task 3)
Task 3: fix round 1/5 (1 addressed, 0 open; commits cea5faa..285f05f)
Task 3: complete (commits 13edaa9..285f05f, review clean after 1 fix round)
Task 4: dispatched implementer (sonnet), BASE 285f05f — carries Ruling 1
Task 4: ⚠️ resolved — auth order lives in Task 3 code (reviewer read it, matches); deploy doc `/` wording is Task 11 scope
Task 4: review — Important (plan-mandated): reload task reconciles BEFORE swapping keys; a publish racing the reload validates against old keys after reconcile and stays public until next file change
Task 4: Ruling: swap the reload order — write the fresh keys under the write lock first (waits for in-flight publishes holding the read guard), then reconcile; fix the comment — spec promise "revocation/takedown effective within 10s" outranks plan's line order — cost if wrong: none (a publish arriving after the swap validates against fresh keys)
Task 4: review — Important: tokenless and empty-token GET /live/{id}/frame has no HTTP test (public 200 / private 401)
Task 4: minor (deferred): reload task wiring (reconcile + shared write) untested
Task 4: minor (deferred): PUT public route — note_start rate limit and 16KB body limit untested
Task 4: minor (deferred): LiveLanesResponse doc comment now sits on PublicTitle
Task 4: minor (deferred): new tests not rustfmt-shaped (long lines)
Task 4: minor (deferred): PUT spends a rate-limit slot before proof of control; manager self-heal after relay restart from one source can hit 10/min (429 is retried, not stuck) — weigh at final review
Task 4: fix round 1/5 (2 addressed, 0 open; commits abe125e..56c6e42)
Task 4: minor (deferred): reload-race fix verified by reasoning only (no deterministic concurrency test)
Task 4: complete (commits 285f05f..56c6e42, review clean after 1 fix round)
Task 5: dispatched implementer (sonnet), BASE 56c6e42
Task 5: review dispatch failed (API session limit, HTTP 429); retrying
Task 5: ⚠️ resolved — relay-side tokenless frame/lanes authorization is covered by Task 4 HTTP tests (a_published_rooms_frame_can_be_read_without_a_token_over_http etc.)
Task 5: minor (deferred): public.html has no [data-theme="dark"] override (no toggle on that page; cosmetic)
Task 5: minor (deferred): live.html render() concatenates endedPublic + " · " + link text
Task 5: complete (commits 56c6e42..583a37a, review clean)
Task 6: dispatched implementer (sonnet), BASE 583a37a
Task 6: implementer interrupted by controller session restart (uncommitted changes in 8 src files); resumed same agent
Task 6: ⚠️ resolved — old-JSON-without-`public` → Private guaranteed by #[serde(default)] + #[default] Private (reviewer verified)
Task 6: minor (deferred): no literal round-trip test of LiveInfo JSON missing `public`
Task 6: complete (commits 583a37a..f6b70ee, review clean)
Task 7: dispatched implementer (opus), BASE f6b70ee — replaces Task 6's TODO(Task 7) placeholder arms in daemon handle
Task 7: implementer stream stalled after committing 42e00dd and writing its report; controller verified suite (1492 passed, 0 failed) and clippy clean at HEAD
Task 7: ⚠️ resolved — commit 42e00dd has no AI attribution (checked)
Task 7: review — Important (pre-existing root cause, spec-promised): after a real relay restart the pusher never re-POSTs /live/start (`started` only resets when not live), so frames/lanes get 401 forever and publicity heal never triggers
Task 7: Ruling: close it inside Task 7 as fix round 1 — when a frame push or the lanes read gets HTTP 401 for the current room, clear `started` so the next loop re-registers the room with the same id and keys (which also re-applies publicity via the existing public_applied reset); keep start_backoff on repeated failures — spec L154/L158 name relay restart as a heal trigger and the whole live (not only publicity) silently dies otherwise; small, contained in pusher_loop — cost if wrong: an extra POST /live/start after a spurious 401 (idempotent for the same push secret)
Task 7: minor (deferred): stuck intent + manager publish then withdraw → seen_public shows Pending forever (public_stuck blocks the PUT)
Task 7: minor (deferred): extra idempotent DELETE after each restage once unpublished (clear public_applied only when want_public.is_some())
Task 7: minor (deferred → carried to Task 8): empty title after trim reaches the relay (413, stuck); Task 8 UI must refuse empty titles
Task 7: minor (deferred): publish key frozen at press time; changing key needs pressing p again (matches spec)
Task 7: minor (deferred → carried to Task 8): 429/5xx shown as Failed{Refused(code)} during backoff; UI must not word 429 as permanent
Task 7: minor (deferred): pusher_loop publicity block ~60 lines could be a helper; live.rs 2391 lines
Task 7: minor (deferred): timing-based backoff test margin; "heals once" check weak (20s cadence vs 2s window)
Task 7: fix round 1 — resumed implementer stalled again mid-fix (uncommitted src/live.rs partial); dispatched fresh implementer (opus) with brief, report, finding, ruling
Task 7: fix round 1 — fresh opus implementer also stalled (3rd stall; partial diff doesn't compile, 13 errors → model stream stall, not a hanging test). Dispatched fresh sonnet implementer to discard the partial diff and redo from 42e00dd
Task 7: fix round 1 — 4th attempt (sonnet) also stalled. Root cause of all 4 stalls: full `cargo test --workspace` now takes ~1554s (1492 pass) — almost all compile/link of ~25 integration test binaries after src/live.rs changes; a single Bash call >600s trips the subagent stream watchdog. Not a code hang (no binary >13s).
Ruling: implementers run only focused tests (`cargo test --lib live::`, `cargo test --lib daemon::`) and `cargo clippy --lib --tests` in bounded calls; the controller runs the full workspace suite + strict clippy in the background after each commit and feeds failures back — cost if wrong: an integration-test failure is found one step later
Task 7: fix round 1 — 5th attempt (sonnet, bounded commands) stalled with NO changes made → suspect subagent stream infrastructure, not command length; running a trivial haiku health-check agent
Task 7: health-check haiku agent returned in 5s (subagents work); lib test build 2s; stalled agent transcript shows last event = Read of src/live.rs then 10 min of no model output → stall correlates with large-file reads into context
Task 7: fix round 1 — 6th attempt (sonnet) dispatched with exact line map, offset/limit-only reads, no brief/report reads, bounded commands
Task 7: fix round 1 — 6th attempt succeeded: c76c6eb (focused live:: 80 pass ×3, daemon:: 42 pass, lib clippy clean, mutation red→green); controller full suite running in background; scoped re-review dispatched
Task 7: controller full suite at c76c6eb — 1494 passed, 0 failed; strict clippy clean
Task 7: fix round 1/5 (1 addressed, 0 open; commits 42e00dd..c76c6eb)
Task 7: complete (commits f6b70ee..c76c6eb, review clean after 1 fix round; 6 dispatch attempts for the fix due to subagent stalls)
Ruling: all remaining dispatches use scoped reads (line maps, offset/limit ≤120, no full reads of big files), bounded commands (no workspace test/clippy), and the controller runs the full suite after each commit — five stalls correlated with large reads and >600s calls — cost if wrong: slightly more controller work per task
Task 8: dispatched implementer (sonnet), BASE c76c6eb — carries Task 7 minors: UI must refuse empty titles; don't word 429 as permanent
Task 8: implementer stalled (no commit, clean tree) right after `sed -n '190,404p' src/ui/live.rs` returned 13KB; same signature as Task 7's stalls (large source dump → no model output for 10+ min)
Ruling: tighten dispatch rule to ≤60 lines of source per tool call, counting sed/cat/head/grep -A output as well as Read — every stall followed a ≥13KB source dump — cost if wrong: implementers take more, smaller reads
Task 8: re-dispatched implementer (sonnet) with ≤60-line rule
Task 8: 2nd implementer stalled (clean tree) after a 445-byte result at ~50 tool calls (364KB transcript) → stalls are not dump-size; long agent sessions stall
Ruling: split Task 8 into three short sequential dispatches, each committing its own step: 8a plumbing (LiveInput, View::Live{state,input}, App.last_public_title, all constructions/destructures compile, ui tests green); 8b key handling + input line drawing + the brief's panel tests; 8c banner text/style, bar_live_style, help/escape hints, guard-test cases, banner test — review covers 8a..8c as one Task 8 diff — cost if wrong: three commits instead of one for Task 8
Task 8a: dispatched (sonnet), BASE c76c6eb
Task 8a: implementer stalled after a 182-byte result (~25 tool calls); partial uncommitted edits: src/ui/view.rs (+30), src/ui/app.rs (+2). Subagent stalls now hit even tiny, short dispatches (9 stalls total since Task 7) → infrastructure, not task shape. Asking user how to proceed.
User decision: keep dispatching subagents (retry). Task 8a: re-dispatched (sonnet) to finish partial diff, ≤20 tool calls
Task 8a: committed f9beb2b (ui:: 531 pass, lib clippy clean, Debug-redaction mutation red→green); temporary #[allow(dead_code)] on LiveInput / last_public_title / View::Live.input until 8b wires them
Ruling: split the rest of Task 8 into 8b (key handling p/K + input editing + publish/unpublish + panel tests), 8c (draw input line + key-never-drawn test + help/escape hints), 8d (banner text/style, bar_live_style, solid-bar guard cases, banner test) — shorter dispatches complete; 8a took 25 calls / 2.5 min — cost if wrong: more commits
Task 8b: dispatched (sonnet), BASE f9beb2b
Task 8b: committed 64948c6 (ui:: 533 pass, lib clippy clean, 2 mutations red→green); dispatching 8c
Task 8c: committed 2a145d6 (ui:: 536, i18n:: 19, lib clippy clean, 2 mutations red→green; EN help texts no longer repeat the key letter)
Task 8d: committed c14d9ea (ui:: 537, lib clippy clean, 2 mutations red→green)
Task 8: controller full suite at c14d9ea — passed 1502 failed 0; clippy exit in output above
Task 8: minor (deferred): SetSecret RPC failure closes the key input instead of keeping it open for retry (matches brief)
Task 8: complete (commits c76c6eb..c14d9ea, 4 step commits, review clean; full suite 1502 pass, strict clippy clean)
Task 9: dispatched implementer (sonnet), BASE c14d9ea, scoped-read rules (≤60 lines/≤8KB per call)
Task 9: review — Important: relay fetch() calls have no timeout and run inside the global store.mutate queue (slow relay stalls all admin/student mutations)
Task 9: review — Important: heal timer has no reentrancy guard (overlapping heal cycles)
Task 9: review — Important (plan-mandated `.catch(() => null)`): healPublic clears livePublic on any daemon RPC failure, not only confirmed not-live
Ruling: fix all three in Task 9 — (a) AbortSignal.timeout(10000) on every relay fetch; (b) a `this.healing` guard so a heal run never overlaps; (c) in healPublic distinguish "RPC failed" (keep the record, try next tick) from "daemon confirmed not live" (clear); and do the daemon RPC + relay HTTP outside store.mutate, taking the mutation queue only to write the record — spec's "daemon says not live → clear" presumes a confirmed answer, and liveStatus's own doc says unknown ≠ not live — cost if wrong: a stale record for one extra tick
Task 9: minor (deferred): stored livePublic.title not truncated to 60 while relay copy is
Task 9: minor (deferred): live-stop leaves livePublicError on the row
Task 9: minor (deferred): room id parsed by pathname.split('/').pop() (brittle to trailing slash)
Task 9: fix round 1/5 (3 addressed, 0 open; commits e3291d1..198ae89)
Task 9: minor (deferred): admin live-public/live-start publish path still calls the (10s-bounded) relay fetch inside store.mutate
Task 9: minor (deferred): relayPublic re-calls liveStatus that healPublic just made (duplicate RPC per heal)
Task 9: minor (deferred): this.healing not initialized in constructor
Task 9: complete (commits c14d9ea..198ae89, review clean after 1 fix round)
Task 10: dispatched implementer (sonnet), BASE 198ae89 — exact anchors verified by controller (each occurs once)
Task 10: minor (deferred): menu "公开这场直播" folds the privacy warning into the prompt label instead of a separate confirm() like live-start
Task 10: complete (commit ba9ffab, review clean, no fix round; UI verified statically only — no Playwright)
Task 11: dispatched implementer (sonnet), BASE ba9ffab — CLI/routes/banner strings verified by controller; deploy doc already has 一/二/三 so new section is 四
Task 11: complete (commits 9abd9d6, d131654 — controller review; fixed crate-map lines in both READMEs and 三节→四节 the implementer missed)
Final: controller full suite + clippy + node tests running on d131654^ (docs-only delta)
Final: controller full suite on d131654: 1502 passed / 0 failed; strict clippy clean; node server tests 10/10
Final review (opus): Needs fixes — I1 heal treats relay 401 "room not registered yet" as key revoked; I2 manager publish record not bound to room id; I3 paste ignored in live key/title input; minors M1-M4; fix-now deferred: T2 key named "--file", T4 misplaced doc comment, T9 live-stop leaves livePublicError
Ruling: one fix wave, two parallel dispatches split by language (no shared files). Node (fix-node): I1 (on PUT 401 probe GET /live/{id}/lanes with the viewer token from status.url #t=; 401 there → room not registered yet, keep record, retry next tick; only a 401 with the room present is a real key rejection), I2 (store {title,id}; heal re-publishes only same id, different id clears the record without an error), T9 live-stop clears livePublicError. Rust (fix-rust): I3 paste arm for View::Live input (Key → clean_secret, Title → trim newlines), M1 (same probe in the daemon: PUT 401 + own lanes 401 → clear started, don't mark stuck), M3 (daemon refuses empty title after trim with a code the TUI already maps, or Error BadRequest), T2 name starting with "--" rejected, T4 move misplaced doc comment. Leave M2 (mtime-only reload; ext4/APFS have ns mtime) and M4 (restage resets public; ≤30s off list for manager path, daemon path immediate) — cost if wrong: a revoke written in the same second as the last write waits for the next change / a restaged admin-published live is unlisted for up to one heal tick
Fix wave node: 659262c — I1 lanes probe on 401, I2 record {title,id} (legacy adopts), T9 live-stop clears error; node 14/14; controller read the diff: correct
Fix wave rust: 0c5d63c I3 paste, 05b8f48 M1 lanes probe on PUT 401, d41d416 M3 BadRequest empty title, dee46a9 T2 + T4; focused tests/clippy clean; controller read the diff. Scoped re-review (opus) + controller full suite dispatched on HEAD
Final after fix wave on dee46a9: full suite 1507 passed / 0 failed; strict clippy clean; node 14/14
Scoped re-review (opus): all 8 FIXED, no new Critical/Important, Ready to merge. Minors: relayPublic re-derived id (window) — controller fixed (expectId); error path compares title only (leave, very narrow); TUI shows key-rejected ~500ms during restart probe (leave); legacy record adopts current room (leave, one-time, by design)
Controller fix: relayPublic(w,'PUT',title,id) refuses when the room changed since heal compared it; node 14/14 (no dedicated test — fake daemon returns a fixed room)
Branch complete; handing to finishing-a-development-branch

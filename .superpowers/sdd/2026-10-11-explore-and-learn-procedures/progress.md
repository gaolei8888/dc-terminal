# SDD ledger — plan: /Users/lei/Documents/work/dc/dc-octo/docs/superpowers/plans/2026-10-11-explore-and-learn-procedures.md

Spec: /Users/lei/Documents/work/dc/dc-octo/docs/superpowers/specs/2026-10-11-explore-and-learn-procedures-design.md
Worktree: /Users/lei/Documents/work/dc/dc-terminal-explore  branch: explore-and-learn  base: 430d982
Scope (from dispatcher): D1 D2 D3 D5 D6 D7 D8 D9 D10 D11 D12 D13 D14 D15 D16 D19. Skipped: D4 (Jeff weights), D17/D18 (real screen), D20 (endpoint), V1/O1/O2 (other repos, parallel).

## Baseline (430d982, before any change)
- cargo test --workspace -- --test-threads=1: 2126 passed, 0 failed, 4 ignored
- cargo clippy --all-targets: clean
- cargo fmt --check: FAILS on 55 pre-existing files (496 hunks; rustfmt drift, e.g. build.rs, crates/dct-brain/src/approval.rs)

## Pre-flight scan
| Tasks | Shared file / interface | Finding |
|---|---|---|
| D1→D2 | dct_dco::{DcoClient, DcoError, Reply, RUN_TIMEOUT, fake::serve} | consistent |
| D2→D10/D13/D14 | testing::FakeHands (Default, `calls: Vec<String>`) | D13 asserts call strings `confirm_learned accept=[k1]` / `reject=[k1]` → FakeHands must format calls that way and derive Default |
| D2→D10 integration tests | `testing` is cfg(any(test, feature="fake")) | cfg(test) is off for integration tests in tests/*.rs → dct-explore needs feature "fake" on for its own tests (self dev-dep with features) |
| D3↔D7/D8 | judge::ChoiceQuestion vs dct_explore::JevChoice | same shape; From conversion lives in root crate (needed in D16 adapter) |
| D8→D9 | ladder::Who used by TraceStep | order ok |
| D9→D14 | TraceStep.from_procedure | defined in D9, used in D14 |
| D11→D13 | popup name `<platform>-<slug 24>` vs D13 fixture `tiktok-turn-on-automatic-con` | D13's name is only a fixture inside its own test `written()`; no cross-task assertion. ok |
| D12→D13 | Written{procedure, popups, rounds}, Dcv trait (+add/vocab/popups) | consistent; existing FakeDcv in src/procedures.rs tests must gain the 3 methods |
| D14 | publish_step(procedure, &Platform) but ReplayInput has no platform | inconsistent → Ruling R2 |
| D16→D10/D12/D13/D14 | consumes all | consistent |
| D19→llm | Backend, Prompt{system,user,max_tokens}, complete_with_timeout, LlmError::Unavailable | exist in src/llm/mod.rs |
| D3/D15/D16 | src/main.rs, src/lib.rs | sequential edits, no conflict |
| every task | "全量检查" includes `cargo fmt --check` | fails at baseline → Ruling R1 |
| D1..D19 self-consistency | tests vs specified code | no other self-contradiction found on read |

## Rulings
- Ruling R1: Full check per task = `cargo test --workspace -- --test-threads=1` + `cargo clippy --all-targets` (no warnings) + rustfmt clean on every file the task creates or touches (`rustfmt --check --edition 2021 <files>`); do NOT reformat the 55 pre-existing unformatted files — baseline `cargo fmt --check` already fails there (rustfmt drift) and reformatting would bury the diff — cost if wrong: one later mechanical `cargo fmt` commit.
- Ruling R2: D14 ReplayInput gains `platform: &'a Platform` so `preview` can compute publish_step — the plan's publish_step needs the platform's publish words and replay has no other source — cost if wrong: one extra field.
- Ruling R3: Branch name `explore-and-learn` (dispatcher's), not plan's `explore-learn` — dispatcher instruction wins — cost: none.
- Ruling R4: The repo tracks .superpowers/sdd briefs/reports/progress (its .gitignore says so; sdd-workspace rewrote it to `*`, restored with git checkout). Implementers do not commit sdd files; controller commits the ledger/briefs/reports in one docs commit at the end — cost if wrong: one commit to drop.
- Ruling R5: Commits carry no AI attribution / Co-Authored-By (user rule overrides the harness reminder).
- Ruling R6: Every task also runs `cargo check --workspace --target x86_64-pc-windows-msvc` (target installed) and must add no errors/warnings — dct ships on Windows and the D1 extraction nearly broke it; baseline has one pre-existing warning (src/student_projects.rs:669 unused `path`) — cost if wrong: a few seconds per task.

## Tasks
Task D1: dispatched (base 430d982)
Task D1: implemented 0d05d1c; review: spec ✅ approved, 3 minors (clientInfo.version now dct-dco's 0.1.0; timeout message says 10 s always; crate ungated unix). Controller cross-check: ungated unix = real Windows-build regression (dct-game had `#[cfg(unix)] mod dco`, play.rs now re-exports DcoError from ungated unix crate) → Important.
Task D1: minor (deferred): initialize clientInfo.version reports dct-dco 0.1.0 instead of dct version
Task D1: fix round 1/5 (2 addressed, 0 open; commits 0d05d1c..3f37281)
Task D1: complete (commits 430d982..3f37281, review clean) — controller full suite: 2129 passed, 0 failed, 4 ignored; clippy 0 warnings; Windows check OK
Task D2: dispatched (base 3f37281)
Task D2: implemented f893b22 (2137 passed); review dispatched
Task D3: dispatched (base f893b22) in parallel with D2 review (disjoint files; reviewer read-only)
Task D2: review ❌ — Important: parse_el yields frac {0,0,0,0} for real dco help.elements (dco sends window-relative `bounds` in points, no `frac`; only `see` adds frac — confirmed dc-octo src/run/mod.rs:873, src/tools/read.rs:438, element.rs:21). Minors: FakeHands drops window_id/params/popups; non-array `elements` → empty list; missing text/role → "".
- Ruling R7: Fix D2's frac gap with a stateful adapter `pub struct DcoHands { client: DcoClient, .. }` implementing Hands instead of `impl Hands for DcoClient`: it remembers each window's size (from `see` reply window.w/h, else `list_windows` size.w/h) and run_id→window_id (from run_step/run_procedure), and converts help elements' `bounds` to `frac`; parse_el uses `frac` if present, else `bounds`/size, else error. Keeps El's shape so the plan's literal El constructions in D6–D10 tests stay valid — cost if wrong: one wrapper type D16 must construct.
Task D2: fix round 1/5 (3 addressed, 0 open; commits f893b22..3a415f6)
Task D2: complete (commits 3f37281..3a415f6, review clean)
Task D2: minor (deferred): DcoHands::finish_run drops a run's reply (and run_id) if size lookup fails after the run executed; size cache never invalidated (resize), run_windows leaks on completion; list_windows size parsing duplicated; no test for look-recorded size path
Task D3: implemented acc37f9 (2156 passed); DONE_WITH_CONCERNS — no RED run (tests written with impl), criteria serialized alphabetically (serde_json no preserve_order), empty key rejected, timeout by error-text match
Task D5: dispatched (base acc37f9) in parallel with D3 review
Task D3: review ❌ — Important (plan-mandated sketch): criteria sent alphabetically (serde_json Map w/o preserve_order) though ChoiceQuestion is ordered and Jeff's layout is positional.
- Ruling R8: D3 must send criteria in the caller's order: serialize the request body without relying on Map ordering (e.g. write the criteria object in order into the JSON text); do NOT enable serde_json/preserve_order (crate-wide, would change other dct JSON output) — spec says Jev choice options are ordered and Jeff reads positions — cost if wrong: a few lines of hand-built JSON.
- Ruling R9: also refuse to send an api key over plain http to a non-loopback host (cleartext bearer) — reviewer minor; cheap and security-relevant — cost if wrong: user must use https or loopback for keyed judges.
Task D3: minor (deferred): timeout detected by "timed out" substring; 422 branch untested; CLI requires `instructions` though jeff-serve treats it optional; non-string state stringified; --probe success path untested
Task D3: fix round 1 implemented ae175dc (2163 passed); re-review dispatched
Task D5: implemented b3538dd (2160 passed at its run); review dispatched
Task D6: dispatched (base b3538dd)
Task D3: fix round 1/5 (2 addressed, 0 open; commits acc37f9..ae175dc)
Task D3: complete (commits 3a415f6..ae175dc, review clean) — deviation: TDD RED not captured (tests written with impl)
Task D3: minor (deferred): duplicate criteria keys in a judge file fall back to alphabetical order
Task D5: complete (commits ae175dc..b3538dd, review clean)
Task D5: minor (deferred): publish_words entries with surrounding whitespace pass validation; domain blank check not trimmed; override errors lack row index; duplicate names in one override last-wins
Task D6: implemented 716ef94 (2170 passed); concerns: added delete/删除 to general never-tap (dct-brain rates bare Delete as Content but brief test needs it refused); made dct-brain tier::normalize pub for reuse
Task D7: dispatched (base 716ef94) in parallel with D6 review
Task D6: review ✅ approved w/ minors; controller confirms one as real spec gap (spec §4 names 二维码 / 手机验证 screens; lists lack 二维码/qr code/手机验证) → fix round 1 with word-list additions + platform never_tap test + Budget default test.
- Ruling R10: Accept D6's addition of `delete`/`删除` to the general never-tap list — plan test requires Delete refused ("不可撤回的也拦") and dct-brain rates bare Delete as Content — cost if wrong: explore can't tap any label containing delete/删除 (e.g. 删除草稿); user must do it.
Task D6: minor (deferred): `follow` in NEVER also blocks Following/Followers tabs; screen_halt scans user-typed text too (over-stop allowed; D10 should show the matched line)
Task D6: fix round 1 implemented c48a702; re-review dispatched
Task D7: implemented c182fd7 (2184 passed); review dispatched
Task D8: dispatched (base c182fd7)
Task D6: fix round 1/5 (2 addressed, 1 open — new stop words untested, report overclaimed; commits 716ef94..c48a702)
Task D7: complete (commits c48a702..c182fd7, review clean)
Task D7: minor (deferred): Fill keys not de-duplicated while question() renames dup keys to _2 but Opt.key keeps original (lookup mismatch if element ids repeat); CJK goal text is one token (goal-overlap score rarely fires); `score` has unused goal param; state_text doesn't strip newlines from element text (page text could forge "[eN]" lines for Jeff/model); icon-neighbour direction untested; textless input field gets no Fill (only TapIcon)
Task D6: fix round 2 implemented fc98a98; re-review dispatched
Task D8: implemented db9ccba (2190 passed); review dispatched
Task D9: dispatched (base db9ccba)
Task D6: fix round 2/5 (1 addressed, 0 open; commits c182fd7..fc98a98)
Task D6: complete (commits b3538dd..fc98a98 excluding D7's c182fd7, review clean)
Task D8: complete (commits fc98a98..db9ccba, review clean)
Task D8: minor (deferred): NaN answer_confidence would pass the threshold (D3's read_choice already rejects non-finite probs, so unreachable via JevHttp); Jeff key accepted from `all` though Jeff only saw `short`; model name passed through for non-icon options; Jeff-picked preset-named TapIcon carries name None; user key not validated against `all` (D16 must); missing tests (model not called at 2 tries, threshold ==, Jeff picks stuck confidently, why text)
Task D5-add: user addition via coordinator — seed rows for weibo, bilibili, zhihu (+ zhihu article), douyin, kuaishou, wechat_channels, toutiao, wechat_mp, baijiahao; xiaohongshu flagged; all unverified; QR-phone login flag; wechat_mp final publish by user only. Dispatched to D5 implementer.
- Ruling R11: The two per-row flags need schema fields (Platform is deny_unknown_fields): add `#[serde(default)] login_qr: bool` (informational: gate already halts on 二维码/login screens and never sends them to a model; dct do shows it when halting) and `#[serde(default)] publish_by_user_only: bool` (wechat_mp). Consumption of publish_by_user_only is code: D14 replay forces preview (stop before publish step) and D16 never auto-publishes such a platform — the user taps publish — cost if wrong: two booleans and one branch.
- Ruling R12: zhihu gets two rows (`zhihu` on www.zhihu.com for 想法, `zhihu_article` on zhuanlan.zhihu.com/write) because one row = one domain; Chinese rows use vault social-global with "vault" listed unverified (same as the existing xiaohongshu row); publish_words required non-empty → placeholder "发布" listed in unverified (validate requires a word; a placeholder is marked, not claimed) — cost if wrong: data edits.
Task D5-add: session restart; implementer's uncommitted edits verified by controller (9 platform tests pass, rustfmt clean) and committed 3791c1d; no RED evidence. Task D9: committed cc02485 before restart, unreviewed → review now.
Task D9 + D5-add: reviews dispatched
Task D10: dispatched (base 3791c1d, opus)
Task D9: implemented cc02485 (2197 passed); review dispatched
Task D5-add: committed 3791c1d by another controller after a session restart (report says RED evidence lost); review dispatched
Task D10: dispatched (base 3791c1d, model opus) — carries R11 publish_by_user_only handling, redaction-first, validate user key
Task D9: complete (commits db9ccba..cc02485, review clean)
Task D9: minor (deferred): save() does not redact (caller must; no redact_trace helper) — D10/D14 must redact first_lines/vanished/scene text; test name redact_does_not_touch_placeholders doesn't test placeholders; non-atomic save; 16-bit run-id suffix + silent overwrite
Task D5-add: review — Important: no full-check evidence (controller running it in a clean temp worktree at 3791c1d); minors to fix: test must assert all 5 unverified fields; never_tap guesses not marked unverified; comments state unverified facts (weibo 首页直接发, wechat_mp 编辑器难驱动).
Task D9: complete (commits db9ccba..cc02485, review clean)
Task D9: minor (deferred): run dirs created with default umask (0755) though files are 0600; save not atomic (truncate+rewrite); redact is exact/case-sensitive — callers must redact raw text; QuestionRow/Trace free-text fields rely on caller redaction
Task D5-add: complete (commits cc02485..3791c1d, review clean; RED evidence lost, reviewer ran focused test: 9 passed)
Task D5-add: minor (deferred): seed xiaohongshu row (from plan) has guessed publish_words "发布" not listed unverified; magic row count 16 in a test
Task D11: dispatched (base 3791c1d) in parallel with D10 (disjoint files)
Task D5-add: extra commit 0c1ac74 (Chinese rows also mark never_tap unverified; tests check every unverified field) — appeared from the resumed D5 implementer/other controller; data+test only
Task D11: implemented bea7cf4; added ProcedureDraft.notes; workspace run hit 1 failure ui::tests::a_brand_new_folder_is_not_a_reason_to_refuse_an_agent (passes alone; likely concurrent-test interference) — controller to re-verify when quiet; review dispatched
Task D5-add: fix round 1/5 (3 addressed, 0 open — controller-verified tiny diff; commits 3791c1d..0c1ac74)
Task D5-add: full check at 3791c1d (clean temp worktree): tests/daemon_upgrade.rs restarting_replaces_the_daemon_with_one_that_serves FAILED once while D10's suite ran concurrently; passes 3/3 in isolation → load/timing flake (pre-existing test, unrelated code). Re-running workspace --no-fail-fast at 0c1ac74.
CONFLICT 08:05:03: a second, undispatched agent committed D11 (bea7cf4) and is writing D10 in this worktree; my D10 implementer (opus) stopped BLOCKED without writing. Dispatching paused; asked coordinator who owns D10+.
Coordinator decision (user-approved): stale copy of this session terminated; I own D10+. Stale copy committed D11 bea7cf4 and D10 88a686d (no reports, unreviewed). target-check/ was MY stray CARGO_TARGET_DIR, removed; temp check worktree removed.
Full check at 88a686d (controller): 2220 passed, 0 failed, 4 ignored; clippy 0; rustfmt clean on draft/explore files; Windows only pre-existing warning.
Task D11 (bea7cf4) + D10 (88a686d): reviews dispatched (D10 on opus with controller decisions 1-7)
Task D12: dispatched (base 88a686d)
Task D11: review ❌ — Important: non-ASCII popup text slugs to empty → name `<platform>-popup`, second distinct popup silently dropped (and its tap removed from steps) — normal case on Chinese platforms. Minors: no positive test for verified publish_done marker line; no tests for NoPublish/Empty/{ marker/short popup text; newline/empty arg not guarded; dismiss tap with empty vanished kept unflagged; maybe_one_off flags ordinary fill taps.
Task D10: review ❌ (opus) — Important: (1) Tries reset when screen loops back (A↔B never escalates, burns budget; looped action not traced); (2) redaction exact-match only — OCR-split/case-changed values leak into state/answers/first_lines/vanished; (3) controller decision 3a (collapse newlines) missing; (4) no screen_halt on needs_help path (login wall during help → user handed the picker). Minors: look_error message + icon name unredacted; by-user publish recorded ok even if screen unchanged; Published needs a `done` pick after user tap; open_url recorded as Who::User; act.rs 471 lines; missing tests 3b/3c/model By/halt paths/decline under by-user.
Task D12: implemented f95f34c (2228 passed, 5 ignored incl. new #[ignore] real-dcv test — installed dcv lacks V1); review dispatched
Task D10: fix round 1 dispatched (fresh opus implementer; original terminated)
Task D11: fix round 1 dispatched (fresh sonnet implementer; original terminated)
Task D12: complete (commits 88a686d..f95f34c, review clean) — #[ignore] real-dcv test not run: installed dcv lacks V1 (rejects run:<id>)
Task D12: minor (deferred): any show() failure treated as "name free"; fixer returning identical body burns a round; existing popup names not handled/tested; real-dcv test sets process-global env; RealDcv::popups returns empty on missing field
Task D13: dispatched (base f95f34c)
Task D11: fix round 1 implemented 84c65d6; re-review dispatched
Task D11: fix round 1/5 (2 addressed, 0 open; commits f95f34c..84c65d6 — only 84c65d6 is D11)
Task D11: complete (commits bea7cf4 + 84c65d6, review clean)
Task D11: minor (deferred): `back` exempt from arg guard though dcv rejects back with an arg; popup hash uses raw arg (case-only variants → near-duplicate drafts); dismiss tap with empty vanished kept unflagged; maybe_one_off flags ordinary fill taps (spec-conform)
Task D13: implemented 153b806 (2241 passed, 5 ignored); review dispatched
Task D15: dispatched (base 153b806); D14 held until D10 fix lands
Task D13: review ❌ — Important: card renders p.body/p.name but write_drafts may fix the body / rename → user approves unseen steps. → fix round 1 (resume implementer).
Task D13: minor (deferred): no publish_by_user_only notice on the card (render_card has no platform; D16 must add the line); popup approve hashes live dcv state; partial failure after procedure approved untested; learned buttons only as a count
Task D10: fix round 1 implemented e626074 (2254 passed); looped actions traced ok:false, revisit restores ok/progress; re-review dispatched
Task D15: implemented 0300e0d (2261 passed); review dispatched
Task D14: dispatched (base 0300e0d)
Task D15: review ❌ — Important: approved text rendered raw (ANSI/\r/newline could make the user approve something else); lib.rs mod order breaks rustfmt (R1). → fix round 1 (resume implementer), plus opening_sentence split fix.
Task D15: minor (deferred): distinctness only on post_text; same-name campaign overwritten silently; no O_NOFOLLOW on create. OPEN QUESTION (plan open issue 5): terminal check can't stop an agent with its own pty from typing y — user to decide whether text approval also needs Touch ID.
Task D10: fix round 1/5 (5 addressed, 1 new Important — fuzzy redaction runs before screen_halt/tappable/tier, so post text containing halt/banned words hides login walls and they reach models; commits 153b806..e626074)
Task D10: also flagged: asked_publish once per run → a second publish attempt is tapped without asking (plan decision 3: every exploration publish asks) → treat as Important; minors: 2-word publish word scrubbed away; placeholder-only icon name still sent; scene_hash collision rollback risk; popup dismissed in a loop yields no popup draft
- Ruling R13: The D10 round-2 fixer also fixes the same ordering in D14's replay.rs (which reused the scrubber for its help-path halt check) — one root cause, one fix; D14 review waits for it — cost if wrong: D14 diff reviewed after a cross-task edit.
Task D13: fix round 1 implemented b8865b7 (2265 passed); re-review dispatched
Task D15: fix round 1 implemented c8f3243 (2282 passed); re-review dispatched
Task D14: implemented a23ffb3 (14 tests); review held until D10 round 2
Task D19: dispatched (base c8f3243)
Task D13: fix round 1/5 (2 addressed, 0 open; commit b8865b7)
Task D13: complete (commits 84c65d6..153b806 + b8865b7, review clean)
Task D13: minor (deferred): maybe_one_off step numbers / notes still from original draft — could point at wrong step if the Fixer renumbers
Task D15: fix round 1/5 (2 addressed, 1 open — Unicode bidi/format/line-separator chars pass; commit c8f3243) → round 2 (resume implementer), incl. load() re-validation
Task D10: fix round 2 implemented 4d9846f (raw-text Raw type for halt/never-tap/publish matching incl. replay.rs per R13; ask before every publish tap); 2301 passed
Task D15: fix round 2 implemented b6d2413 (format/bidi/separator chars refused at parse and load; ZWJ allowed); RED by disabling check afterwards
Task D19: implemented 1401667 (2301 passed); review dispatched
Task D14: review dispatched (a23ffb3; replay.rs halt ordering later changed by 4d9846f)
Task D16: dispatched (base 4d9846f, opus)
Task D19: complete (commits b6d2413..1401667, review clean) — deviation accepted: prompt sends options as one JSON object (injection-safe) instead of `key: text` lines
Task D19: minor (deferred): model icon name not length-capped/sanitised; only literal key "stuck" treated as stuck; CHATTY_STARTS prefix match rejects legit bodies starting "sure"
Task D15: fix round 2/5 (named chars addressed; open: remaining Cf ranges e.g. U+0890/08E2/13430–1343F, approve() lacks checks; commit b6d2413) → round 3
Task D15: fix round 3 implemented 768eecb (2303 passed) — re-review pending
Task D14: review ❌ — Important: dco reports a closed window as `failed` with note "窗口关了（stale）"/"窗口已经关了（stale）" → replay classifies as NeedsRepair and would explore a dead window (Review Focus 5). Minors: fault words matched over whole raw (step log); answer_run Err path doesn't cancel_run; already_done preview message misleading; Previewed drops learned_keys; test gaps. Fix after D10 round 3 (both touch replay.rs).
Task D10: fix round 2/5 (3 addressed, 2 new Important — raw halt fires on the user's own typed post (e.g. post contains "QR code"/"登录"); placeholder-intent Tap/Fill dropped so the composer can vanish; commits 1401667..4d9846f)
- Ruling R14 (D10 round 3 design): classify each element per look as a VALUE element (its whole folded raw text is contained in some param value and is ≥ 2 words / ≥ 4 CJK chars, or contains the whole value) vs a UI element. VALUE elements: excluded from screen_halt and never-tap/tier word checks, never offered as tap/fill targets by their text, displayed as {param}. UI elements: halt/never-tap/tier/publish matching on raw text; displayed text scrubs exact value occurrences, the value's own lines, and only LONG fuzzy runs (≥ 4 consecutive value words or ≥ 8 CJK chars), never short 2-word coincidences; tap/fill intent = the raw UI label (a short coincidental overlap with the post is UI text, not the value). Rationale: the user's own post is not a screen warning, and a composer placeholder sharing two words with the post is still the composer — cost if wrong: a 2–3 word fragment of the post that also appears inside a longer UI label could reach a model/disk.
Task D15: fix round 3/5 (2 addressed, 0 open; commit 768eecb)
Task D15: complete (commits e626074..0300e0d + c8f3243, b6d2413, 768eecb; review clean) — note round-2/3 RED evidence produced by disabling checks after writing
Task D15: minor (deferred): approve-refusal test asserts only is_err; fix-round-3 report lacks pasted workspace totals
Task D10: fix round 3 implemented 10e3aff (2336 passed); deviation: fragment-as-value only after that param was typed this run (else placeholder "What's happening?" becomes a value element); re-review dispatched
Task D16: committed e1ab528 (agent not yet reported)
Task D10: fix round 3/5 (2 addressed, 1 open + new — composer still vanishes after a failed type (mark_typed before run_step), in replay help (mark_all_typed before start), in explore_from repair (fills not seeded from prefix); squashed (space-less) value matching ignores word boundaries ("Log in" vs "Blog in Paris"), 12-letter single-token rule likewise; remembered Fill offered on screens without the composer; commits e1ab528..10e3aff). Round 4 → fresh opus implementer after D14 fixer leaves replay.rs.
Task D16: implemented e1ab528 (28 tests; commit alone 2331/0/5; shared tree 2336/0/5); review dispatched (opus)
Task D16: review ❌ — Important: unparsable config.toml silently falls back to defaults (brief: config error exit 2). Minors: exit 1/3 by message prefix; Touch ID cancel exits 1; no-draft exits 0 with draft:false; show() error = no procedure; duplicate NoJeff/NoFix in tests; explore_cli.rs ~580 non-test lines; terminal fin errors to stdout. Controller checked: repair path approves w.procedure = renamed draft (D12) — yes-path test requested. → fix round 1 (resume implementer).
Task D14: fix round 1 implemented 3a2a0be; re-review dispatched
Task D10: fix round 4 dispatched (fresh opus implementer, per skill rounds 4-5)
Task D14: fix round 1/5 (all addressed; commit 3a2a0be)
Task D14: complete (commits c8f3243..a23ffb3 + R13/R14 replay parts in 4d9846f/10e3aff + 3a2a0be, review clean)
Task D14: minor (deferred): fault words substring-matched on note (a quoted "stale" in screen text → stop, safe side); stale code also covers window-resize/stale-screenshot failures as stops
Task D16: fix round 1 implemented d692781 (2346 passed); re-review dispatched
Task D16: fix round 1/5 (all addressed; commit d692781); minor fixed now as round 2: config parse error echoes the offending line (could hold a secret) into --json stdout
Task D16: minor (deferred): Touch ID cancel recognised by exact message text (procedures::approve stringifies SignError) — typed error would be sturdier; Config::load (daemon path, untouched) still echoes the bad line to stderr
Task D10: fix round 4 implemented a721a5f (2354 passed); re-review dispatched
Task D10: fix round 4/5 (2 addressed, 1 open — one-word "Login" heading suppressed when its letters equal 2 typed post words; new minor: link_like accepts trailing "."; commits d692781..a721a5f)
- Ruling R15: Round 5 goes to the round-4 implementer (already a fresh opus agent after the original 3 rounds) rather than another fresh one — it has the round-4 context and opus is the top tier — cost if wrong: same blind spot persists; breaker adjudicates after round 5.
Task D16: fix round 2 implemented 102a13d (2355 passed); re-review dispatched
Task D16: fix round 2/5 (1 addressed; new minors: sanitiser leaks escaped-quote suffix and integer values; commit 102a13d) → round 3: drop toml message text entirely (org no-secrets rule)
Task D10: fix round 5 implemented 0cdaa03 (2357 passed); residual: "Login to" vs post "log in to" still VALUE (same shape as OCR-merged "dayfor"); re-review dispatched
Task D10: fix round 5/5 (2 addressed, 0 open; commit 0cdaa03)
Task D10: complete (commits bea7cf4..88a686d + e626074, 4d9846f, 10e3aff, a721a5f, 0cdaa03; review clean after 5 rounds)
Task D10: minor (deferred): stale scrub.rs header doc (dot rule, ≥2 element words); inner-dot link rule loose for OCR-glued sentences; residual "Login to" vs post "log in to" = VALUE (same shape as OCR-merged "dayfor"); scene_hash collision rollback; popup dismissed inside a loop yields no popup draft; by-user publish recorded ok even if screen unchanged; Published needs a later `done` pick; open_url recorded Who::User; repair-seeded fill has no screen position
Task D16: fix round 3/5 (controller-verified: only byte offset taken from toml error; commit 5ed4666)
Task D16: complete (commits 768eecb..e1ab528 + d692781, 102a13d, 5ed4666; review clean)
ALL ASSIGNED TASKS COMPLETE. Controller full check at 5ed4666: 2357 passed, 0 failed, 5 ignored (5th = #[ignore] real-dcv test, needs dcv with V1); clippy 0; Windows only pre-existing warning; rustfmt: all pre-existing touched files were already dirty at baseline (tier.rs 15, dco.rs 12, dco_tests.rs 29, play.rs 6, procedures.rs 54 hunks); NEW file crates/dct-explore/src/lib.rs has 1 rustfmt diff (line 8, module order) → final fix wave.
Final whole-branch review: dispatched (opus) over 430d982..5ed4666
Final review (opus): "with fixes". Critical 1: icon name from model/user not checked against tappable/publish words → can tap Post/Follow without confirm (also bypasses publish_by_user_only). Important 2: popup body `应用: <browser>` rejected by dcv (needs platform short name; plan line 2045 bug); 3: generated names with `_` rejected by dcv is_slug; 4: repaired/declined drafts never used (drive only looks up promote-<p>-<kind>), repair draft login-gate check uses stuck screen's first_lines, prefix duplicates verify_text step; 5: publish confirmation hinges on unverified publish words (Chinese rows "发布" placeholder; xiaohongshu not marked); 6: successful publish can end Stopped (meter before done; needs a done pick); 7: private_screen error drops run_id → parked run uncancellable; 8: campaign file on disk = approval (user decision); 9: Touch ID not bound to card body (sha); 10: card prints Fixer output/screen text raw (ANSI/bidi). Minors 11–18.
- Ruling R16: One final fix wave (opus) covers 1–7, 9 (bind approval to sha of the shown body if dcv's body_sha256 is reproducible; else refuse-if-changed via a show() before Touch ID), 10, and minors 11, 12, 13, plus must-fix deferred: maybe_one_off/notes recomputed or dropped when Fixer changed body, xiaohongshu publish_words unverified, scrub.rs header doc. Item 8 is NOT fixed — needs the user's decision (sign campaigns with the user key / Touch ID) — reported as open question. Item 4 may need dcv support; fixer reads dc-vault (read-only) and implements dct-side resolution or reports blocked. — cost if wrong: residuals surface to the user at finish.
Final fix wave: 10 commits 0dfcceb..360d010 (2391 passed, 0 failed, 5 ignored); item 4 claimed not-a-bug (dcv approve of draft_of replaces original, show.rs:966-1006) — verify in re-review; item 13 extended to all previews; one flake src/session.rs recovering_from_a_failure_after_real_input… passed 3/3 alone; open risk: approval record saved under draft name (dct run/tickets for original name)
Final re-review: items 1–13 + 2 deferred ADDRESSED; fixer's item-4 claim verified (dcv approve of a draft_of draft writes procedures/<target>.md as approved and deletes the draft file → `dcv show <target>` returns the repaired executable body).
- Ruling R17 (breaker, no second fix wave per skill): PARKED, load-bearing, surfaced to user — the item-6 fix returns Published as soon as a publish tap lands on a new screen, so a two-stage publish (发布 → confirm dialog 继续/发布) is reported "发出去了" after stage 1 and the draft lacks the second tap. Suggested fix: Published only when the new screen has no tappable publish-word element or shows a verified publish_done; otherwise keep looping with the budget waived. Cost if left: false success message and an incomplete draft on two-stage platforms (draft still needs the user's approval, which shows the steps).
Final: minor (deferred): approval record saved under the draft name after a repair (dct run/tickets for the original name need a second approval; fails closed); per-tap confirmations reuse confirm_publish events in --json; repair open_url taken from the procedure not the table; "Posts" icon name not caught by whole-word publish detection.
Final controller check at 360d010: 2391 passed, 0 failed, 5 ignored; clippy 0; Windows only pre-existing warning; all new .rs files rustfmt-clean.

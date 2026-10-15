# Task D10 report
Implemented by a stale copy of the controller session (terminated); committed 88a686d without a report. Controller full check at 88a686d: 2220 passed, 0 failed, 4 ignored; clippy 0; rustfmt clean; 12 explore_loop tests pass. No TDD evidence available.

## Fix round 1
Commit: e626074 (on top of 153b806). Files: crates/dct-explore/src/explore.rs, src/explore/act.rs, new src/explore/run.rs, new src/explore/screen.rs, src/trace.rs (doc comment on `ok` only), tests/explore_loop.rs. draft.rs and root src/ untouched.

### Per finding
1. **Failures lost on loop-back.** `Run.tries` is now `HashMap<screen key, ScreenTries{tries, failed_jeff}>`; a failure is charged to the screen where the pick was made (`Run::charge(cur.key)`), never reset on screen change. A↔B: second round at B skips Jeff → model → user.
   Test `a_loop_back_to_a_seen_screen_counts_against_the_screen_where_jeff_picked` (Jeff asked 3x, model 1x, user 1x; trace ok flags asserted).
2. **Fuzzy redaction.** `Scrub` (moved to explore/screen.rs) adds per-value fuzzy pass after the exact passes: whole element replaced by `{name}` when its squashed fold (no spaces/punct) is a substring of the value and it has ≥2 words (or a lone token ≥12 ASCII / ≥4 CJK chars); otherwise every run of ≥2 consecutive folded words matching a contiguous run of the value's words is replaced by `{name}`. Short lone words (Post, 发布) are never replaced. Applied to everything Scrub touches (elements → state/options/first_lines/vanished/answers, help elements, dco notes/errors, icon names).
   Test `a_value_split_or_recased_by_ocr_never_leaves_the_process` (value split across two OCR elements with doubled space + a re-cased copy with prefix; asserts no 2-consecutive-word fragment in saved trace.json, answers.jsonl, any state or option text handed to Jeff). Unit tests in screen.rs for split/recased/respaced/Chinese/short-button cases.
3. **Control chars (3a).** `Scrub::line` = redact then map every `char::is_control` to a space; `els()` uses it. Test `newlines_in_element_text_cannot_forge_a_line_in_the_state` (no `[e9]` line in state; no control chars in Jeff's option texts).
4. **screen_halt on needs_help.** `act::execute` runs `screen_halt(scrub.els(help.elements))` before choosing an answer; hit → `cancel_run` + new `Exec::Halt` → `Outcome::Halted`. Test `a_login_wall_during_help_halts_without_a_picker` (Halt::Login, user not asked, no answer_run, `cancel_run r1` called).
- look_error now takes the scrubber (`dco 出错：<scrubbed message>`); model/user icon names are `scrub.line`d before they become `Act::TapIcon.name` (→ dco arg and TraceStep.arg), and user names given in the help picker are scrubbed before answer_run. Test `an_icon_name_or_dco_error_that_repeats_the_value_is_redacted`.
- Added tests: `an_unknown_user_key_is_never_executed` (3c), `a_colliding_element_id_maps_to_exactly_what_jeff_was_shown` (3b), `a_model_pick_is_sent_as_model_and_traced_as_model`, `declining_under_publish_by_user_only_stops_before_publish`.
- Size: explore.rs 297, act.rs 327, run.rs 211, screen.rs 326 lines.

### RED / GREEN
RED (`~/.cargo/bin/cargo test -p dct-explore --test explore_loop`, before the fix): 16 passed, 5 failed:
- a_loop_back_…: `FakeHands: run_step 被调用的次数比脚本里排的 steps 多` (Jeff asked again at B, loop kept going)
- a_value_split_…: `trace.json 里漏了「launch day」`
- newlines_…: assertion failed, state contained a forged `[e9]` line
- a_login_wall_…: `FakeHands: answer_run 被调用的次数比脚本里排的 answers 多` (help answered on a login screen)
- an_icon_name_or_dco_error_…: why = `看不到画面：could not use hello from dct test（boom）`
The four tests for 3b, 3c, model By and by-user decline passed on first run: characterization tests, behaviour was already right (no RED). For 3b: `all_options` already drops a second element with a duplicate key, and element keys always start with `e` while fixed/fill/attach keys don't, so `question()` never has to rename and Jeff's key maps 1:1. The screen.rs unit tests were written with the implementation (no separate RED).
GREEN: same command → 21 passed, 0 failed; `cargo test -p dct-explore` lib 70 passed.

### Full check (after commit content, with other agents' uncommitted src/ files present in the worktree)
- `cargo test --workspace -- --test-threads=1`: **2254 passed, 0 failed, 5 ignored** (daemon_upgrade flake did not occur).
- `cargo clippy --all-targets`: no warnings.
- `rustfmt --check --edition 2021` on the 6 touched files: clean.
- `cargo check --workspace --target x86_64-pc-windows-msvc`: only the pre-existing src/student_projects.rs:669 warning.

### Decisions
- **How looped actions are traced:** an action that dco executed but that left the screen unchanged or landed on a screen already seen this run is recorded as TraceSteps with `ok: false` (by = who picked it). On arriving back at seen screen S, every step's `ok` and the `Progress` (filled/attached/published) are restored to the snapshot taken when S was last the current screen (`Run::arrive` / `Run::back_to`). Net effect: the `ok` steps are always exactly the path from the start to the current screen (draft.rs already copies only `ok` steps), and every executed action remains in the trace. E.g. A→B (Drafts), B→A (Back), A→B (Drafts) yields `[open_url ✓, Drafts ✓, Back ✗, Drafts ✗]`. trace.rs's `ok` doc comment now says this. dco `failed` replies still leave no step (nothing to record), only a charged failure.
- The state's "已经做了" trail marks `ok: false` steps with「（没起作用）」so Jeff/model can see what did not work.
- Fuzzy redaction runs on every scrubbed text, not only when the param counts as on screen: over-redaction is the safe side.

### Concerns / left as-is (controller said may leave)
- by-user publish (publish_by_user_only) is recorded ok/publish even if the screen did not change.
- `Published` still needs a later `done` pick after the publish tap.
- open_url is recorded as Who::User.
- Fuzzy redaction can redact UI text that happens to equal ≥2 consecutive words of a value (e.g. a value containing "Post now" would hide a "Post now" button). Accepted as the safe side; the publish word alone is never touched.
- A detour step that was a publish and then looped back would be marked ok:false (edge case; publish normally lands on a new "sent" screen).
- Snapshots store one Vec<bool> per visited screen (≤40 steps), negligible.

## Fix round 2
Commit: 4d9846f. Files: crates/dct-explore/src/explore.rs, src/explore/act.rs, src/explore/run.rs, src/explore/screen.rs, new src/explore/raw.rs, **src/replay.rs (D14, per ruling R13)**, tests/explore_loop.rs, tests/replay.rs.

### A — safety checks on raw text
- New `explore/raw.rs`: `Raw` holds the unscrubbed elements (no Debug/Clone/Serialize; private field). It only answers: `halt(scrub)` (screen_halt on raw text; the matched line is scrubbed before it is returned), `tappable(id, platform)` (gate::tappable = never-tap + tier on the raw label), `shown(scrub, platform)` (scrubbed elements for display; a label whose raw fold equals a platform publish word is shown as that word).
- `screen::see` keeps `Raw` inside `Screen` (private) next to the scrubbed look; `Screen::halt` and `Screen::options` (all_options on the scrubbed look, then drop Tap/TapIcon/Fill whose raw label is not tappable, and Tap/Fill whose intent contains a `{param}` placeholder).
- Raw halts everywhere: main loop, the marks look (`Pics`), the explore needs_help path, and replay's needs_help path. Explore help picker and replay options/answer lookup only offer raw-tappable elements. Replay's halt message now carries the scrubbed Halt.
- Tests: `a_login_screen_halts_even_when_the_post_text_says_log_in`, `…_says_scan_to_log_in` (扫码登录), `a_post_saying_follow_us_does_not_make_a_follow_button_tappable`, halt line asserted scrubbed (`{post_text} X`, no 2-word fragment); replay `a_login_wall_during_replay_help_halts_even_when_the_post_text_says_log_in` (no Jeff/model/user, cancel_run, message has no "log in to"). All 15 D14 replay tests still pass.

### B — ask before every publish tap
- Removed `asked_publish`; step 6 asks `Confirm::publish_ok` before every publish tap. Test `every_publish_tap_asks_first_even_after_a_publish_that_did_nothing` (first tap leaves the screen unchanged, the user re-picks publish → asked again, "no" → DeclinedAtPublish, only one Post tap).

### Minor
- Icon name that scrubs to something containing a placeholder → Stopped, nothing sent to dco (also filtered in the help picker and replay answers). Test `an_icon_name_that_is_only_a_placeholder_is_never_sent_to_dco`. The round-1 combined test was split: the dco-error half is now `a_dco_error_that_repeats_the_value_is_redacted`.
- Publish words matched on raw text: `a_publish_word_inside_the_post_text_is_still_recognised_as_publish` (tiktok "Post now", post text starting "Post now …" → still a publish option, Confirm asked).

### RED / GREEN
RED (`cargo test -p dct-explore --test explore_loop`): 21 passed, 6 failed — the two login tests, the placeholder icon test, the publish-twice test and the publish-word test all hit `FakeHands: run_step 被调用的次数比脚本里排的 steps 多` (they went on to act); follow-us failed showing option `("e7", "按钮 {post_text}")`. RED (`--test replay`): 14 passed, 1 failed with `FakeHands: answer_run 被调用的次数…` (help answered on a login wall).
GREEN: explore_loop 27 passed, replay 15 passed, lib 70 passed.

### Full check
- `cargo test --workspace -- --test-threads=1`: **2301 passed, 0 failed, 5 ignored** (with other agents' uncommitted root src/ files present in the worktree).
- `cargo clippy --all-targets`: no warnings. rustfmt --check clean on all 8 touched files.
- Windows check (forced recheck of dct-explore): only the pre-existing src/student_projects.rs:669 warning.

### Decisions
- A label equal to a platform publish word is shown unscrubbed (it is the platform's word, not user content), even if the post text also contains it.
- Tap/Fill options whose scrubbed intent contains a param placeholder are dropped instead of sending `{post_text}` as an intent to dco / into the procedure.
- Raw halt is the single source; the scrubbed elements are never passed to screen_halt anymore.

### Recorded (no fix, as instructed)
- scene_hash collision: two different screens with the same hash would be treated as a loop and roll back ok flags/progress to the earlier screen's snapshot.
- A popup dismissed inside a loop (the dismiss step ends up ok:false after rollback) yields no popup draft, since draft.rs only reads ok steps.
- Still open from round 1: by-user publish recorded ok even if the screen didn't change; Published needs a later `done` pick; open_url recorded as Who::User.

## Fix round 3 (ruling R14)
Commit: 10e3aff. Files: crates/dct-explore/src/explore.rs, src/explore/act.rs, src/explore/raw.rs, src/explore/run.rs, src/explore/screen.rs, new src/explore/scrub.rs, src/replay.rs, tests/explore_loop.rs, tests/replay.rs.
Public API: nothing public changed. `explore`, `explore_from`, `ExploreInput`, `Outcome`, `values_on_screen` and the replay API are unchanged. Crate-internal changes: `Scrub` moved to `explore::scrub` and is still re-exported as `explore::screen::Scrub`. `Raw::new(els, &scrub)` gained a scrub argument. New `Scrub::mark_typed`, `mark_all_typed` and `value_of`.

### Design as implemented
- **Value element**: one of two cases.
  - It contains a whole param value, and the value itself is ≥2 words, ≥4 CJK chars or a ≥12-letter token. The size rule keeps a short value like "test" from swallowing "Latest posts".
  - That param has been typed this run, and the element's whole squashed fold is a substring of the value and is ≥2 words, ≥4 CJK chars or a ≥12-letter token (a lone ≥12-letter token, e.g. a URL, is a small extension of R14).

  A value element is shown as `{param}`. It is left out of screen_halt, never-tap, tier and publish-word checks, and is never tappable.
- **Why "typed"**: before the post is typed, its fragments cannot be on screen from us. Without this gate the placeholder "What's happening?" (2 words) would itself count as a value element of "What's happening at booth 12…", and the composer would never be offered. Typed is marked in `act::execute` just before each `type_param` run_step, from `type_param` steps in an `explore_from` prefix, and for every param in replay (`mark_all_typed`: the procedure may already have typed).
- **UI elements**: rules on raw text, as in round 2. Displayed text = exact value, the value's own lines, and long runs only: ≥4 consecutive value words, or ≥8 consecutive CJK chars inside a non-ASCII token. Tap/fill intents come from that displayed text, so a normal label is never touched.
- **Re-fill**: `Run.fills` remembers the last successful Fill option per param. When a text param is not filled and the screen offers no Fill for it (the composer shows half the post, so it is a value element), that remembered option is offered again with its pre-typing intent. The "text wiped → placeholder back" flow works as before (test `text_wiped_by_attach_…` still passes).
- The publish-word exception applies only to UI elements, so a post that equals a publish word is no longer copied out.

### Tests (RED first)
RED, explore_loop (27 passed, 4 failed):
- `a_typed_post_with_a_halt_word_does_not_halt_but_a_real_login_wall_does`: assertion left/right failed (`Login("{post_text}")`).
- `a_post_that_starts_like_the_placeholder_still_gets_the_composer`: panicked "还有填字的选项".
- `a_chinese_placeholder_inside_the_post_still_offers_fill`: panicked "还有填字的选项".
- `a_post_saying_follow_us_keeps_the_composer_for_typing_again`: panicked "还能再填一次".

RED, replay (15 passed, 1 failed):
- `a_filled_composer_with_a_halt_word_does_not_stop_replay_help`: calls were `["run_procedure", "cancel_run r1"]`.

GREEN: explore_loop 31, replay 16, lib 70. Scrub unit tests moved into scrub.rs and were rewritten for the R14 rules (long runs only; fragments count only after typing; a short value does not match UI text).

Two round-2 assertions were updated deliberately to R14:
- The login-halt tests now expect the reported line to be the UI line as shown ("Log in to X" / "扫码登录"). It is a 2–3 word coincidence, which R14 no longer scrubs. They still halt with no Jeff/model/user asked.
- The replay login-wall test now checks that the message carries the UI line and not the post ("see our").

The round-1 OCR-split/case-changed leak test passes unchanged.

### Full check
- `cargo test --workspace -- --test-threads=1`: **2336 passed, 0 failed, 5 ignored**.
- clippy: no warnings. rustfmt --check clean on all 9 files.
- Windows (forced recheck): only the pre-existing student_projects.rs:669 warning.

### Concerns
- After the post is typed, a real UI element whose whole text is a ≥2-word piece of the post (e.g. a "Log in" button while the post says "Log in to…") becomes a value element and does not trigger a halt by itself. A real wall normally has other UI lines ("Log in to X", "Forgot password"), which still halt. Inherent to R14.
- Halt lines and UI labels can now carry 2–3 word coincidences with the post (by design).
- A post that is exactly a single publish word (e.g. "Post") would make the publish button a value element, so it is not offered (safe: no publish).
- A remembered fill reuses the old field element. If dco needs help on it, the auto-match by text and position usually fails and the user picks.
- Recorded from round 2, still open: scene_hash collision rollback; popup dismissed inside a loop gives no popup draft; by-user publish recorded ok even if the screen didn't change; Published needs a `done` pick; open_url recorded as Who::User.

## Fix round 4
Commit: a721a5f (on top of 3a2a0be). Files: crates/dct-explore/src/explore.rs, src/explore/{raw,run,screen,scrub}.rs, src/replay.rs, tests/explore_loop.rs, tests/replay.rs. Root src/ untouched.
Public API: no change. `explore`, `explore_from`, `ExploreInput`, `Outcome`, `values_on_screen` and the replay API have the same signatures. Crate-internal changes: removed `Scrub::mark_all_typed`. Added `Raw::has_field`, `run::remember_fill` and `run::seed_from_prefix`.

### Per finding
1a. **Retry after a failed type.** `run::remember_fill` puts a Fill option into `Run.fills` before `act::execute` runs it. It only adds the option when that param has no entry yet. A successful Fill still overwrites the entry through `advance`. `mark_typed` still runs before `type_param`, which is the privacy-safe side: a half-typed post is still scrubbed as a value. Test `a_failed_type_then_retry_still_offers_the_composer`: type_param returns failed and the composer comes back. On the second round the user is offered `fill_post_text_*` with intent "What's happening?".
1b. **Replay help.** `mark_all_typed` is gone. Before each needs_help, `typed_before(procedure, at_step)` marks a param typed only if one of its `type_param` steps has n < at_step. Test `replay_help_before_the_post_is_typed_still_offers_the_composer`: help at step 3 (`What is happening?`), post "What is happening at booth 12, come by". Jeff is shown the element and its answer `e1` goes to answer_run.
1c. **Repair.** `run::seed_from_prefix` replaces the mark loop in `explore_from`. For each prefix `type_param` it marks the param typed. If the step right before is `tap_by_intent` and the param is a text param, it also seeds `fills[param]`. The seeded option has key `fill_{param}_again`, intent = that tap's arg passed through `scrub.line` (skipped if it has a placeholder), and a synthetic field El. Test `explore_from_after_a_prefix_that_typed_offers_fill_again_with_the_prefix_intent`: the prefix tapped "What's happening?" and typed, then the text was wiped. The Fill is offered again, and picking it sends `tap_by_intent: What's happening?` then `type_param: post_text`.
2. **Word-aligned value matching** (scrub.rs, new `Sq`: squashed chars plus the squashed span of each whitespace word).
   - Piece of a value (needs typed): the element's squashed text has to occur in the value and cover at least 2 whole value words that are not all-CJK. Partial words are allowed at the two edges, so OCR mid-word splits ("y for our") and missing spaces ("dayfor") still match. Otherwise the element needs at least 4 CJK chars (character-based, as before).
   - The single ≥12-letter token rule is tightened. A lone token counts only if it looks like a link: one whitespace word containing `/ . @ #`, with ≥12 squashed chars. A lone UI heading word ("Verification", "Login-required") is never a value element.
   - Whole value contained: the match must start and end on element word boundaries. CJK chars are boundaries on their own.
   - Loop tests: `a_log_in_button_halts_when_the_post_says_blog_in_paris` and `a_lone_heading_that_is_one_word_of_the_post_still_halts`. The second uses heading "Login-required" with post "…login-required…". The brief's "Verification" is not a halt word in gate.rs, so a "Verification" heading can't halt on its own. That case is covered by the unit test `value_of("Verification") == None`.
   - Unit tests `pieces_are_matched_on_word_boundaries` and `a_whole_value_counts_only_on_word_boundaries` cover: Log in / Blog in Paris, Verification, the mid-word split, the missing space, a link, and "This here" vs value "is here". They were written after the implementation, so they have no RED; one of them caught that a link sits mid-word, which led to dropping the boundary requirement for link-like tokens.
3. **Remembered Fill only where its field is.** `Screen::options` offers `fills[p]` only when `Raw::has_field(intent, p)` is true: either a UI element's raw label folds equal to the intent, or a value element of that param is on screen. Test `the_remembered_fill_is_not_offered_on_a_screen_without_the_composer`: after typing, the run taps Drafts to reach a "Your drafts" list, and no `fill_*` option is offered there.

### RED / GREEN
RED (`~/.cargo/bin/cargo test -p dct-explore --test explore_loop --test replay`, before any fix): explore_loop 31 passed, 5 failed; replay 22 passed, 1 failed.
- a_failed_type_…: `打字失败后还能再填：[("e5", "按钮 Drafts", None), ("scroll_down", …), …]` (no fill)
- explore_from_after_a_prefix_…: `开头打过的字被清掉后还能再填：[("e5", Some("按钮 Drafts")), …]`
- the_remembered_fill_is_not_offered_…: `…不该给填字：[…, ("fill_post_text_e2", Some("在「What's happening?」里填 {post_text}")), …]`
- a_log_in_button_… and a_lone_heading_…: `Stopped { … }` instead of Halted (the run went on and asked Jeff)
- replay_help_before_…: `left: Stopped("求助时用户说停，停下")` (composer filtered out of pickable)
After the scrub fix alone, the two halt tests passed and the other 3 explore tests still failed. GREEN after all fixes: lib 72, explore_loop 36, replay 23, hands_client 3. All pre-existing tests pass unchanged, including the D14 replay tests and the round-1 OCR leak test.

### Full check
- `cargo test --workspace -- --test-threads=1`: **2354 passed, 0 failed, 5 ignored**. The daemon_upgrade flake did not occur.
- `cargo clippy --all-targets`: no warnings.
- `rustfmt --check --edition 2021` on all 8 changed files: clean.
- `cargo check --workspace --target x86_64-pc-windows-msvc` (forced recheck of dct-explore): only the pre-existing src/student_projects.rs:669 warning.

### Residual risks
- A failed `type_param` still marks the param typed. If nothing was typed, a UI element that is a ≥2-word piece of the post is treated as a value element (inherent to R14) until the run ends.
- If a Fill attempt on the wrong field fails first, the remembered option points at that field until a Fill succeeds. Finding 3's gate means it is offered only where that field's label or a value element is on screen.
- The seeded repair Fill uses a synthetic field (id `prefix_<param>`, zero frame). If dco asks for help on it, the auto-match by text and position fails and the user picks.
- Partial words at the edges: a UI element covering two whole post words plus a partial edge word (e.g. "log in Paris" for "Blog in Paris") is still a value element. Same class as the R14 concern.
- A link-like lone token only has to occur somewhere in the value; word alignment is not required.
- Replay: a param typed by a procedure step whose action is not `type_param` would not be marked typed. The closed vocabulary has no other typing action today.

## Fix round 5
Commit: 0cdaa03 (on top of a721a5f). Files: crates/dct-explore/src/explore/scrub.rs, tests/explore_loop.rs. No API change.

### Per finding
1. **(Important) One-word element = two joined post words.** In `Fuzzy::has_piece`, the non-CJK, non-link branch now has three conditions:
   - the element has at least 2 words of its own (`e.words.len() >= 2`);
   - the match covers at least 2 whole value words;
   - every inner element word boundary maps onto a value word boundary.

   Element "Login" with post "Please log in to see the demo" is now a UI element and halts. The OCR cases still count as VALUE:
   - "y for our": 3 element words, boundaries aligned;
   - "Launch dayfor our": boundaries after "launch" and after "dayfor" fall on value boundaries.

   Loop test: `a_login_heading_halts_when_the_post_says_log_in`. Unit test additions: `value_of("Login") == None`; `value_of("log in")` is still VALUE.
2. **(Minor) `link_like` with trailing punctuation.** A token is link-like only if it contains `/`, `@` or `#`, or a `.` between two alphanumerics (domain shape), and still needs ≥12 squashed chars. A leading or trailing `.` does not count.
   - Loop test: `a_heading_with_a_trailing_dot_is_not_a_link` ("Login-required." heading, post "…login-required…", halts).
   - Unit test additions: `Login-required.` → None; `example.com` (<12) → None; `@leining_tech_official` → VALUE.

### RED / GREEN
RED (`~/.cargo/bin/cargo test -p dct-explore --test explore_loop`, before the fix): 36 passed, 2 failed. Both the new tests returned `Stopped { … }` instead of `Halted`: the run went on and asked Jeff.
GREEN: lib 72, explore_loop 38, replay 23, hands_client 3.
While writing the unit tests I tried to also assert that `"Login to"` is not VALUE. It still counts as VALUE: "login" covers "log"+"in", but both of the element's own boundaries align, which is the same shape as the required "dayfor" merge case. I dropped that assertion (see residual risks).

### Full check
- `cargo test --workspace -- --test-threads=1`: **2357 passed, 0 failed, 5 ignored**.
- `cargo clippy --all-targets`: no warnings.
- `rustfmt --check` on both files: clean.
- Windows (forced recheck of dct-explore): only the pre-existing src/student_projects.rs:669 warning.

### Residual risks
- A multi-word element where one element word is two post words merged (e.g. "Login to" for a post with "log in to") is still VALUE. This is what lets OCR-merged words ("dayfor") count as VALUE, and you can't have one without the other. A lone "Login" or "Log in to X" (the "x" is not in the post) still halts.
- A word that OCR splits with an inserted space ("La unch day") no longer aligns and becomes a UI element; display then scrubs only long runs.

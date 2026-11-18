# scene-fixes report (branch feat/scene-fixes)

## Fix 1: shrink screenshot + clearer model failure
- llm::Prompt gets `image_mime: Option<String>` (None = image/png); used in the OpenAI data: URL and Anthropic media_type (src/llm/mod.rs, http.rs). No-image and PNG bodies unchanged (existing tests still pass; new jpeg-mime test).
- src/game/scene.rs: `shrink()` runs `sips -s format jpeg -s formatOptions 70 -Z 1000` through an injectable `Sips` closure, temp files in a 0700 dir (std, removed on drop; tempfile is dev-only). Failure/empty output -> original PNG + `image_note`.
- Vision::pick now returns Result<VisionAnswer, VisionFail{Silent,Timeout,Error}>; SceneStop::NoVision(VisionFail); stop_line says timed out vs errored in Chinese. Record gets `image_bytes`, `image_note`.
- Tests: success sends jpeg bytes+mime and cleans dir, 0700; failure and success-without-output fall back; Timeout/Unavailable mapping; stop lines.
- Mutation: sending original bytes when sips succeeds -> a_successful_shrink... red.

## Fix 2: effect detection
- play.rs: TapAt.settle: Option<TapSettle> (alias of SwipeSettle), TapSettleReq; Dco::tap_at gets `settle: Option<&TapSettleReq>` (trait + DcoClient; read timeout raised for that call). Reply `settle` parsed from top level of the tap_at result; absent/malformed -> None, not an error, no_tap_at flag untouched.
- scene.rs: `effective()`; dct sends TAP_SETTLE {500, 6000, region 0,0,1,0.8}; record outcome.by = dco_settle|texts.
- Tests: all cases from the brief + dco client tests (request sent, parsed, old dco, malformed).
- Mutations (all red): invert changed, ignore timed_out, drop number rule, plain text diff instead of numbers, exact-match words instead of edit distance 1, word min length 1, not sending settle.
- DEVIATIONS: (a) a "word" needs >=4 alphanumeric chars, not 3: the brief's own case HINT -> NIE needs NIE (3 chars, edit distance 3 from hint) to be ignored. (b) "numbers" are all digit runs in all texts (4/6 -> 4,6; 5+ -> 5), not play::numbers(), because that drops "5+" and would call the jitter effective.

## Fix 3: drag room
- parse_pick: optional `"from":{"x","y"}`; a present-but-broken from makes the whole pick None (never silently a tap). Pick.from added.
- Loop: from present -> action.kind "drag" {from_x_bp, from_y_bp, x_bp, y_bp}, distance (Euclid in bp) < 200 -> skipped "拖的距离太短"; no_tap/forbidden/dedup on the drop point only; executes Dco::swipe(from, to); no settle for drags, text rule only.
- The prompt text is UNCHANGED: it does not ask the model for `from` yet.
- Mutations (red): min-distance removed, swipe args swapped, from range not validated, broken from -> tap, kind label.

## Verification
- cargo +1.99.0 test --workspace --locked: all pass except the known flaky ones (session::tests::recovering_from_a_failure..., daemon::web_tests::enabling_starts_a_listener...), which pass when rerun alone. clippy -D warnings clean. (cargo fmt not installed on 1.99.0; code hand-formatted.)

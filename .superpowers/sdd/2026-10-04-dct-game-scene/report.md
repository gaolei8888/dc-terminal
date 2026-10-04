# dct game scene: implementation report

All four tasks done, one commit each, nothing pushed. `cargo +1.99.0 test --workspace --locked` and
`clippy --workspace --all-targets --locked -- -D warnings` clean before every commit (only the known flaky tests
failed intermittently under load, a different one each run; each passes when rerun alone).

## Process note (deviation)
For all four tasks I wrote the tests and the implementation in the same pass, so I did NOT observe a separate RED run.
Instead I ran the listed mutation checks to prove the tests bite (below).

## Task 1: llm::Prompt image (faf431c)
- Files: src/llm/{mod,http,cli}.rs; `image_png_base64: None` added at every Prompt construction (bridge.rs, session.rs, cli.rs, game/advisor.rs).
- Tests: OpenAI/Anthropic with image (image block first, text second), and a pin that without an image both bodies equal the old JSON exactly.
- Mutation: dropping the text block when an image is present -> openai image test red. Restored.
- CLI backend ignores the image (comment added).

## Task 2: Dco::capture / tap_at (a964693)
- Files: crates/dct-game/src/{play,dco,dco_tests,screen}.rs, Cargo.toml, Cargo.lock.
- `DcoClient::call_full` returns (body, whole content array); `call` is a thin wrapper, behaviour unchanged.
- `Region`, `TapAt`, `TAP_AT_MAX_AVOID=16` in play.rs. `tap_at` does list_windows (match by `app`) then tap_at; "unknown tool" (either JSON-RPC -32602 -> dco_too_old, or bad_request containing "unknown tool") sets `no_tap_at` and returns code `unsupported`; not_allowed/private_screen pass through.
- Mutations (all red): removed avoid truncation; removed the no_tap_at flag set; capture looking for the wrong content type.
- Extra needed by Task 3 (near_forbidden): `Element.frac: Option<[u16;4]>` (basis points), filled by `see_text` from `frac` (0..1) or `bounds` / `window.size`. `Element` has 7 constructors, all updated with `frac: None`.
- NEW DEPENDENCY: `base64 = "0.22"` added to dct-game (already in the root crate, so no new crate in Cargo.lock, one line changed). Needed to decode the image item.
- list_windows reply shape (`windows[].window_id`, `.app`) taken from dc-octo tests.

## Task 3: scene loop (99396d7)
- Files: crates/dct-game/src/{scene,scene_tests,lib}.rs.
- Pure fns parse_pick, to_bp, in_regions, near_forbidden, quantise, FORBID const (comment: will move to dcv rules); Vision trait; scene() loop; SceneStop; `mood_for_stop`.
- Order per step: see_text (any failure, incl. private_screen, stops) -> capture -> vision. Nothing captured or sent to the model when see_text fails.
- Mutations (all red): no_tap check disabled; capture moved before see_text.
- Choices not spelled out in the plan: sink receives `SceneStep {record, png, say}`; `screen.png` is `png/NNNN.png` by step number; an unparseable model answer counts as a skip; dco `not_allowed` is a skip; `--dry-run` does one step then stops with DryRun (avoids repeated model calls on the same screen); no theme/mood at start (per plan), "wait" mood at stop for private/old-dco/no-vision/skipped3/noop5; after-tap see_text failure writes an `error` record and stops.

## Task 4: command (see commit subject)
- Files: src/game/{scene.rs (new),cli.rs,mod.rs,profile.rs,skill.md}, src/main.rs, tests/game_scene_cli.rs.
- Order in the command: profile -> LLM (not configured/unreachable -> plain sentence, exit 0, dco never touched) -> recorder -> connect dco.
- `no_tap` parsed in profile.rs (max 16, must be inside window), stored on `profile::Loaded.no_tap` (not on dct_game Profile, to avoid touching its many constructors). Built-in candy-crush and local level files untouched.
- Records: ~/.dct/learn/<game>/<date>/steps.jsonl + png/NNNN.png, dirs 0700, files 0600; the PNG written is the exact one sent to the model.
- No image resize: no `image` crate in the tree; the 900px image from dco is sent as is (plan's stated fallback).
- Tests: arg parsing, no_tap parsing, recorder perms, LlmVision with fake backend (JSON, garbage, failure), run_core with fake dco+vision, and binary-level tests (fake dco socket + fake CLI model script): dry-run full flow, private screen (model script never run, no include_image see), no [llm] (exit 0), bad args (exit 2).
- Mutations (red): recorder dir mode 0700->0755; no_tap loading disabled.

## Concerns
- tap_at does not exist in real dco yet; the real list_windows/tap_at reply shapes are inferred from the plan and dc-octo tests, never run against a real dco. The old-dco path (unknown tool -> plain message, exit 0, zero taps) is tested.
- Real see `elements` use pixel `bounds` (handled via window.size); the prototype's `frac` is also accepted.
- rustfmt is not installed for toolchain 1.99.0, so fmt was not run (the repo's game code uses long lines anyway).
- Not exercised: real gateway/qwen call; Windows build of the new unix-gated code (module split by cfg(unix), non-unix prints the Mac-only message).

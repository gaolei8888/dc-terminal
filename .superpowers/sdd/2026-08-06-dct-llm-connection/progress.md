# SDD ledger — plan: docs/superpowers/plans/2026-08-06-dct-llm-connection.md

Worktree: /Users/lei/work/dc/dc-terminal/.claude/worktrees/llm-connection
Branch: feat/llm-connection (from d8a20fe)
Baseline: 411 tests passing
Note: cargo is NOT on PATH by default — implementers must `export PATH="$HOME/.cargo/bin:$PATH"`

Pre-flight: 2 plan defects fixed in 7d76695 (CliBackend dead env field; Task 8
explanation type contradiction + `git add -A`). One suspected defect withdrawn
after checking: .superpowers/sdd/.gitignore contains `*`, so ledger files are
already ignored.

Task 1: complete (commits 7d76695..1bcaf98, review clean)
Task 2: complete (commits 1bcaf98..1ecceb4, review clean)
Task 3: complete (commits 1ecceb4..e8f2a1a, review clean)
Task 3: carry-forward to Task 7 — read_claude_oauth returns Option<String> (bare
  printable token) while read_codex_auth returns Option<Credential>. Task 7 must
  wrap it in Credential::Bearer at the call site, and must not store or log the
  bare String. Consider making the signatures symmetric.
Task 3: minor (deferred): auth_mode field is parsed but never read; a non-null
  non-string OPENAI_API_KEY falls through to Bearer instead of Key.
Task 3: minor (deferred): non_empty does not trim, so a whitespace-only token on
  the non-macOS ~/.claude/.credentials.json path reads as present.
Task 3: minor (deferred): Codex empty-string token case is correct by
  construction but has no assertion.
Task 4: complete (commits e8f2a1a..2bf06dd, review clean)
Task 4: minor (deferred): a panicking backend is reported as Timeout, not
  Unavailable (recv_timeout Disconnected is folded into the same arm).
Task 4: minor (deferred): worker panics print to stderr and garble the alt-screen
  TUI; pre-existing pattern across 6 other thread::spawn sites, not new here.
Task 4: carry-forward to Task 8 — complete_with_timeout BOUNDS the wait but still
  BLOCKS the calling thread for up to d. Task 8 must call it from a spawned
  worker, never from tick(). Also: no in-flight cap exists, so Task 8 firing on
  the transition into Failed (once) rather than while Failed is load-bearing.
Task 4: minor (deferred): Prompt derives Debug and prints its full contents — no
  credential may ever be placed in a Prompt field.
Task 5: CRITICAL found in review — run_real deadlocks: write_all to child stdin
  happens before wait_with_output reads stdout/stderr. Prompt > pipe buffer +
  child emitting output = permanent hang. Origin is the PLAN's verbatim code
  (controller-authored), not the implementer. Fixing; plan doc updated to match.
Task 5: fix round 1/5 (1 addressed, 0 open; commits cae8a66..f541263). Writer
  thread now spawned before wait_with_output, join AFTER wait; EOF preserved;
  BrokenPipe benign; no unwrap on the panic path. Regression test uses real
  `cat` with a 600KB payload, recv_timeout(10s)-bounded, #[cfg(unix)]. Plan doc
  corrected too.
Task 5: complete (commits 2bf06dd..f541263, review clean)
Task 5: minor (deferred): if wait_with_output itself errors, the writer thread is
  orphaned holding ChildStdin — detached thread on a rare path, no parent hang.
Task 6: review — spec PASS, quality Needs work. 2 Important entering fix loop:
  (a) dual-timeout has no regression guard; verify.rs factors out
      build_probe_agent() so a no-network test asserts timeout_connect, http.rs
      inlines it inside untestable send_real.
  (b) all 4 injected-Sender tests use |_,_,_| and never inspect arguments; a
      regression hardcoding Wire::Openai inside complete() would pass all 8.
Task 6: minor (deferred): OpenAI max_tokens is rejected by current reasoning
  models demanding max_completion_tokens (fails loudly, not silently).
Task 6: minor (deferred): send_real ignores wire, sending x-api-key and
  anthropic-version on OpenAI requests too.
Task 6: minor (deferred): Anthropic extract_text takes content[0]
  unconditionally; a leading thinking/tool_use block yields Malformed.
Task 6: minor (deferred): HTTP_TIMEOUT 20s has no stated relation to any caller
  budget, unlike verify.rs which justifies its 4s.
Task 6: note: RED phase never observed — 0 tests ran, not a failing test.
Task 6: fix round 1/5 (2 addressed, 0 open; commits 57efba4..a16f37f).
  build_http_agent extracted + asserts both timeouts at 20s with zero IO;
  sender-capture test pins url, credential (== not format), and top-level
  Anthropic system. Verified to fail against an injected hardcoded-Wire bug.
Task 6: complete (commits f541263..a16f37f, review clean)
Task 6: minor (deferred): pre-existing unused `pid` warnings at src/session.rs
  378 and 486 — untouched by this work, noted for the final review.
Task 7: CRITICAL found in review — the oauth closure mapped kimi/glm/deepseek/
  qwen-api to read_claude_oauth(), so an Anthropic Keychain token would be sent
  as Bearer to moonshot.cn / bigmodel.cn / deepseek.com / dashscope. claude.toml
  has no [api], so NoApiEndpoint fires first — meaning exfiltration to a third
  party was the ONLY reachable effect of that branch. Origin: PLAN's verbatim
  code (controller-authored). Rule going in: a CLI's OAuth token may only reach
  that CLI's own endpoint.
Task 7: 3 Important also entering fix loop: (a) precedence unpinned — flipping
  or_else to OAuth-first passes all 7 tests; (b) blanket Debug for dyn Backend
  added to production for a test-only need, matches!() is the repo idiom;
  (c) describe() carries jargon and its test is trivially satisfiable.
Task 7: fix round 1/5 (1 critical + 3 important addressed, 0 open; commits
  17ef482..cbc70d0). Independent audit confirms no cross-program credential
  path via builtins: claude.toml/codex.toml have no [api], so their OAuth is
  structurally unreachable as a network credential.
Task 7: PROCESS ERROR (controller): every dispatch specified `cargo test --lib`,
  which never compiles tests/. Task 2 (1ecceb4) broke tests/concurrency.rs:13
  and tests/slow_input.rs:40 (missing Profile.api/headless) and it went unseen
  for 5 tasks. Remaining tasks must run the FULL `cargo test`.
Task 7: residual (for final review): oauth_lookup keys on profile NAME, not
  destination host. A hand-authored ~/.dct/profiles/claude.toml carrying an
  [api] block — or cfg.llm.base_url — could still direct a real Keychain token
  to an arbitrary host. Same defect class as the Critical; not closed, only
  narrowed. No in-app profile writer exists, so it needs hand-authored files.
Task 7: minor (deferred): describe() says "设置文件" without naming a path, and is
  hardcoded Chinese in a 4-language app.
Task 7: complete (commits a16f37f..cbc70d0 + 8f0bbaa test-fixture fix, review clean)
  Full `cargo test` now verified by controller: 493 passing (457 lib + 9
  integration binaries). Compile break closed.
Task 8: CRITICAL found in review — feature is ON BY DEFAULT with no consent.
  config.rs defaults to provider=claude/transport=cli, so with NO config file
  resolve() succeeds and every failure ships 2000 chars of raw PTY text to a
  third party. Contradicts this project's deny-by-default consent model AND the
  vision doc's own "off by default, opt in per project" rule, which the
  controller applied to auto-answering but not to this. Origin: controller
  design. Fix: absent [llm] section = feature off.
Task 8: 2 Important: (a) UI re-issues Request::Explanation every 16ms and
  reassigns app.message every frame, starving all other messages while attached
  to a Failed session; (b) explanation_slot is never cleared, so a re-failure
  shows the previous answer and two in-flight workers race last-writer-wins.
Task 8: minor (deferred): each transition spawns 2 threads plus an unkillable
  `claude -p` child; no in-flight cap.
Task 8: minor (deferred): explanation unreachable from the board view; only the
  Attached view shows it.
Task 8: fix round 1/5 (1 critical + 2 important addressed, 0 open; commits
  3887b1f..7509f46). Reviewer independently traced every backend-construction
  path: feature cannot be enabled without an explicit [llm] section, and a
  broken config fails CLOSED.
Task 8: complete (commits 8f0bbaa..7509f46, review clean, 1 parked)
Task 8: parked — generation compare is not atomic with the slot write
  (session.rs:602 gen.load() sits outside the slot.lock() at :603; :586 releases
  the slot lock before the bump at :587). Two nanosecond-wide interleavings let
  a stale worker write. Ruling: real but bounded and cosmetic — worst case is
  the previous failure's sentence shown briefly. Fix shape recorded: hold one
  slot lock across clear+bump, move gen.load() inside the worker's lock.
Task 8: minor (deferred): while an answer is pending the UI still issues one
  Request::Explanation IPC per 16ms frame.
Task 9: complete (commit 1ab6876). LIVE RESULTS: claude -p via Keychain SSO
  round-tripped (9.4s, no key anywhere); codex exec via auth.json round-tripped;
  all 4 refusal paths give actionable Chinese and exit 1; kimi+http with no key
  correctly refuses instead of reaching for the Anthropic token (Critical
  verified at RUNTIME); HTTP send_real exercised against a local endpoint with
  the emitted request inspected (path /v1/messages, system TOP-LEVEL, creds on
  both headers). STILL UNVERIFIED: 4 vendor base_urls (need real keys),
  Wire::Openai path (no builtin profile uses it), dc_llm/Ollama (not running),
  explanation output quality against a real failing session.
  User's ~/.dct/ restored to its prior state: no config.toml, no secrets.toml.

FINAL WHOLE-BRANCH REVIEW: NEEDS WORK.
CRITICAL — exfiltration class open AGAIN, 3rd variant, via CLI transport this
  time. Task 2 gave kimi/glm/deepseek/qwen-api a [headless] ["claude","-p"]
  block while they also carry [env] ANTHROPIC_BASE_URL to a third-party host.
  resolve()'s Cli branch passes env to the child but never injects the profile
  secret (session.rs:282 does), so `provider="kimi"` runs claude -p at
  moonshot.cn with no token -> Claude CLI falls back to Keychain OAuth. Also
  simply broken: no vendor key is ever supplied. Origin: controller (Task 2
  also violated the project's own "only live-verified CLIs declare headless").
IMPORTANT — describe() is hardcoded Chinese, bypassing src/i18n.rs in a
  2-language app; "设置文件" names no path.
IMPORTANT — daemon.rs:99 resolve-failure warning goes to /dev/null
  (client.rs:67 nulls daemon stderr): opted-in but misconfigured = silence.
FIX-NOW — http.rs:51 extract_text takes content[0] unconditionally; any leading
  thinking/tool_use block makes the feature fail 100%, silently.
STALE — the design doc still says an absent [llm] section defaults to on.
FINAL FIX WAVE: all 6 findings addressed (4da3ee1, 3349b77, c1ba83c, ab55b2f,
  b01043b). Scoped re-review found ONE new verified bypass in the new gate:
  host_of split the authority on / ? # but not backslash, while ureq's url
  crate (WHATWG) treats backslash as a terminator. PoC confirmed by the
  reviewer running both parsers: "https://evil.test\@api.anthropic.com" showed
  api.anthropic.com to the check and connected to evil.test. Fixed in d3019ff
  with a gate-level test. Controller independently confirmed both tests pass.
  517 passing, 0 clippy warnings.
CLASS VERDICT: NARROWED, NOT CLOSED. Remaining, out of dct's reach: a
  hand-written profile with [env] ANTHROPIC_BASE_URL and no [secret] makes the
  CHILD CLI use its own login — dct cannot control what claude does with env it
  is handed. The interactive session.rs::create path has the same shape and is
  gated only in the UI, not structurally (pre-existing, not from this branch).

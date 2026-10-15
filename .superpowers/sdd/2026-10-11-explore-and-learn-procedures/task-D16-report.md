# Task D16 report: `dct do`

Status: DONE. Commit `e1ab528` on `explore-and-learn`: "dct do: replay if approved, else explore, draft, confirm and approve; --json events for dc.ai". No AI attribution. `.superpowers/` is not committed.

## What was built

`dct do "<goal>" --platform <p> --campaign <name> [--window <id>] [--preview] [--json]`

- `src/explore_cli.rs`: the public interface from the brief (`DoArgs`, `parse`, `WindowPick`, `pick_window`, `run`), plus:
  - `split_params`: a param is a file param if its name is `video`/`image` or its value is an existing file. Everything else is a text param.
  - `params_for(campaign, platform)`: if the campaign has no entry for the platform, the error is "活动 <name> 里没有给 <p> 的文字" (exit 2).
  - `impl From<JevChoice> for ChoiceQuestion`, which keeps the order. `JudgeJeff(Arc<dyn JudgeBackend>)` implements `Jeff`, and any `JudgeError` becomes `None`. `NoJeff` and `NoFix` are the fallbacks used when nothing is configured.
  - `Env` + `drive(args, platform, params, env) -> i32`: the whole wiring sits behind traits, so the tests run it against `FakeHands` and a fake `Dcv`. The steps:
    1. `list_windows` → `pick_window`.
    2. `dcv popups` (if dcv is missing, it stops with exit 1 before anything on screen is touched).
    3. `dcv show <vault>:promote-<p>-<post|video>`. If it is executable, `replay` runs with `preview` passed through. If not, it explores.
    4. On `NeedsRepair`, it tells the user and calls `explore_from(prefix)`. `write_drafts` then sees that the name exists and writes `--draft-of`.
    5. The trace is saved to `~/.dct/explore/runs/<id>/trace.json` whatever the outcome.
    6. On `Published`: `draft` (the dismiss words come from `dcv vocab` `popup_dismiss`) → `write_drafts` → `render_card` → `with_platform_notes` → y/N → `apply_confirmation`. The learned keys come from the steps marked ok.
  - `with_platform_notes`: on `publish_by_user_only` platforms, this adds a "最后发布一下要你自己点…以后重放也只停在发布前" line just before the final `[y/N]` line. It also sets `publish_by_user_only: true` and `publish_note` in the card JSON.
  - Halted output: Login + `login_qr` says "请用手机扫窗口里的二维码登录，登录好再跑一次". Plain login, captcha and warning each have their own sentence. The matched screen line is not echoed, even in terminal mode.
- `src/explore_cli/io.rs`: the user I/O (`Say` trait, `Talk<R: BufRead, W: Write>`), with readers and writers injected.
  - Terminal mode:
    - It prints a numbered option list and "需要你来选（<why>）".
    - It saves the marks picture to `runs/<id>/ask-<n>.jpg` with mode 0600 (base64 decoded) and prints the path.
    - It accepts an option number or key, and `q`/`停` to stop. A bad answer is told what was wrong and asked again, at most 3 times, then it stops.
    - If the chosen icon has no text it asks "这个图标叫什么？".
    - End of input means stop.
    - y/N questions accept y/yes/是/好/确认. Anything else means no.
  - JSON mode prints one event per line:
    - `ask` (why, options[{key,text,needs_name}], picture path)
    - `error` (bad answer, then it re-reads, at most 3 times)
    - `confirm_publish`, `card` (card JSON + message), `note`
    - a final `done | previewed | stopped | declined | choose_window | error`, carrying `exit`.

    Answers are one JSON line each: `{"key","name"}`, `{"stop":true}`, `{"verdict":"yes"}`. Anything else counts as no or stop.
- `src/explore_cli/real.rs` contains the real wiring.
  - On unix it does the following:
    - Reads `~/.dct/config.toml`.
    - Builds the platform table as `Table::seed().with_override(~/.dct/platforms.toml)`.
    - Loads the campaign with `campaign::load`.
    - Connects with `DcoClient::connect_with_timeout(~/.dco, RUN_TIMEOUT)` → `DcoHands`.
    - Jeff comes from `[judge]` via `judge::from_config`. If it isn't configured, or `from_config` fails, there is a note and it uses `NoJeff`.
    - `[llm]` goes through `llm::resolve::resolve`, using the same claude/codex OAuth rule as `cli.rs::oauth_lookup`, into `LlmModel::new(backend, DEFAULT_TIMEOUT)`. That serves as both the Model and the Fixer. If it isn't configured or can't connect, it uses `NoModel`/`NoFix`.
    - `approve_proc` = `procedures::approve(&RealDcv, keys, platform_enclave, approvals, name, &[], now)`.
    - The run id comes from `trace::new_run_id` + getrandom.
  - On other platforms it prints "dct do 要连本机的 dco（现在只有 macOS 版）…" and exits 2. Argument errors are still reported first, also with exit 2.
- `src/main.rs`: `Some("do") => std::process::exit(dct::explore_cli::run(&args[1..]))`, plus a HELP entry.
- `src/lib.rs`: `pub mod explore_cli;`.
- `Cargo.toml`/`Cargo.lock`: `dct-dco` is now a direct path dependency of the root crate. It was only transitive before, and `DcoClient` isn't re-exported by `dct-explore`.
- `README.md`, `README.zh-CN.md`: rows for `dct judge`, `dct campaign approve` and `dct do` in the commands table, plus a `[judge] base_url = "http://127.0.0.1:8765"` example with the threshold and fallback rule.

Exit codes:
- 0: ok. That covers posted, previewed/replayed, and also "posted, but no draft could be written" (`DraftError`).
- 3: the user said no to publish or to the card, the user stopped, halted at login/captcha/warning, the explore/replay stopped, or dco blocked a step.
- 2: argument, config, platform-table or campaign errors; several browser windows; no browser window; an explicit `--window` that isn't a browser window in the list; non-unix.
- 1: dco connect or `list_windows` error, dcv missing (`popups` failed), dcv rejected the draft, the approval failed, or a replay error "dco 出错…".

## TDD evidence

RED: I wrote the 28 tests first, against `todo!()` stubs.
```
$ ~/.cargo/bin/cargo test --lib explore_cli -- --test-threads=1
...
test explore_cli::tests::several_browser_windows_need_an_explicit_window ... FAILED   (panicked: not yet implemented)
test explore_cli::io::tests::a_typed_choice_is_a_number_or_a_key_and_is_checked_against_the_options ... FAILED
...
test result: FAILED. 0 passed; 28 failed; 0 ignored; 0 measured; 1722 filtered out
```
GREEN:
```
$ ~/.cargo/bin/cargo test --lib explore_cli -- --test-threads=1
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 1722 filtered out
```
One mid-way failure was a bug in my test script, not in the code: `json_mode_asks_the_user_when_no_one_else_can` was missing the second marks `look`, and FakeHands panicked "look 被调用的次数比脚本里排的 looks 多". I fixed the script.

Tests:
- From the brief: `parse_requires_platform_and_campaign`, `several_browser_windows_need_an_explicit_window` (Review Focus 5, extended with an explicit id that isn't in the list and with no browser windows at all), `file_params_are_told_apart_from_text`.
- Also: parse refusals; a campaign without the platform; the Jeff/judge adapter (order kept, all 5 `JudgeError`s → None).
- End-to-end against fakes:
  - several Chrome windows → exit 2, and only `list_windows` was called;
  - an approved procedure is replayed, not explored;
  - `--preview` passes through as `stop_before`;
  - wechat_mp replay is always a preview and tells the user to tap publish;
  - explore → draft (`--source import:agent`, `--learned-from run:…`) → card → yes → approve with the right name, trace saved, campaign text never echoed;
  - card no → exit 3 and nothing approved;
  - declining at publish → exit 3, no draft, no Post tap;
  - JSON event stream (confirm_publish, card, final done with exit 0, no param text);
  - JSON user asks (two `ask` events, `{"stop":true}` → stopped);
  - QR login on xiaohongshu → "手机…扫…再跑";
  - dcv missing → exit 1 before any screen action;
  - a broken procedure is repaired with `explore_from` (no `open_url`) and drafted with `--draft-of promote-x-post`;
  - card notes for `publish_by_user_only`.
- Nine io tests cover choice parsing and validation, re-asking, the icon name prompt, end of input / q / 3 strikes, the 0600 picture plus its printed path, JSON answer validation, JSON events, the y/N vs verdict parsing, and tell.

## Full check

- `cargo test --workspace -- --test-threads=1` in the shared tree, which also held another implementer's uncommitted dct-explore edits: **2336 passed, 0 failed, 5 ignored**. `daemon_upgrade` did not flake.
- Commit `e1ab528` checked on its own in a throwaway detached worktree, without the other party's uncommitted edits: full suite **2331 passed, 0 failed, 5 ignored**, `explore_cli` 28/28, clippy clean. The worktree has been removed. That 2331 is 2301 base + the tests from 768eecb + my 28.
- After the concurrent commit `10e3aff` landed on top, `cargo test --lib explore_cli` still gives 28/28.
- `cargo clippy --all-targets`: 0 warnings.
- `rustfmt --check --edition 2021 src/explore_cli.rs src/explore_cli/io.rs src/explore_cli/real.rs`: clean. My hunks in main.rs and lib.rs are formatted. I did not reformat the files' existing diffs, which rustfmt reports from bridge.rs, clipboard.rs and others.
- `cargo check --workspace --target x86_64-pc-windows-msvc`: only the pre-existing `src/student_projects.rs:669` warning. A `DEFAULT_THRESHOLD` dead-code warning on Windows came up and was fixed by moving the constant into the unix module.
- Smoke test on the built binary, which touches no dco and no vault: no args → exit 2 with usage; unknown platform with `--json` → one `{"event":"error","exit":2,…}` line; unknown flag → exit 2.

## Decisions

- The approved-procedure lookup only treats `show` with `executable == true` as an approved procedure. A `show` error counts as "no procedure". `popups` runs first, so a missing dcv still fails early with exit 1.
- If a draft can't be written after a successful post, it still exits 0 with the reason: the user's goal, posting, was met.
- A replay `Stopped` gets exit 1 only when the message starts with "dco 出错". All other `Stopped` and `Blocked` results get exit 3. This is matched on a string because `ReplayOutcome::Stopped` carries no kind.
- Learned buttons from a successful replay are only reported ("还没确认"); `dct do` does not ask to confirm them. In explore mode, only the learned keys from ok steps go into `apply_confirmation`. Keys from steps that didn't help stay unconfirmed, so dco won't replay them.
- `render_card` already ends its text with "[y/N]". The platform line goes in before that last line.
- Halt lines and window titles: halt lines are never echoed. Window titles in the `Choose` list are passed through `trace::redact` against the params.
- The other module (`src/explore_cli/real.rs`) holds the unix/non-unix wiring. `src/explore_cli.rs` still has about 580 lines of non-test code, past the ~500 guideline: the flow, Learn, the replay messages and the adapters. The terminal/JSON I/O was split into `src/explore_cli/io.rs` as you directed.

## Untested (needs the real dco/dcv/screen/Touch ID)

- `real.rs` as a whole:
  - the actual `DcoClient` connection and `DcoHands` against a live dco;
  - `RealDcv` show/popups/vocab/add/approve;
  - `procedures::approve` with Touch ID;
  - `[judge]` from_config against `jeff-serve`;
  - `[llm]` resolve;
  - reading the user's `~/.dct/platforms.toml` and campaigns.
- Real stdin/stdout use, such as interleaving with a tty, and dc.ai reading the JSON stream.
- Whether real dco `see` marks (JPEG base64) decode cleanly into `ask-<n>.jpg`. The tests use synthetic base64.

## Concerns

- Another implementer was editing and committing in the same tree while I worked: uncommitted edits to `crates/dct-explore/src/*` and tests, then commit `10e3aff` on top of mine. I staged only my own files. My commit passes on its own, and my tests still pass on top of `10e3aff`.
- `ReplayOutcome::Stopped(String)` has no kind, so telling exit 1 from exit 3 depends on the "dco 出错" prefix. A typed reason in dct-explore would be sturdier.
- The JSON card event includes the `popup_rules` text ("看到「…」就点「…」"). That is screen text taken from the already-redacted trace, so it has no param values, but it is still popup text.

## Fix round 1

Commit `d692781`: "dct do: unparsable config exits 2, a cancelled Touch ID is a no, early errors go through Say; repair yes-path test". Only `src/explore_cli.rs` and `src/explore_cli/real.rs` are staged. I did not touch `crates/dct-explore`.

- **Config errors (IMPORTANT).** There is a new `pub fn load_config(path) -> Result<Config, String>`. A missing file gives `Config::default()`. A file that can't be read or parsed is an error that names the path. `real.rs` now uses it instead of `Config::load`, and an error exits 2. In `--json` mode that is one `{"event":"error","exit":2,…}` line; in terminal mode it goes to stderr. Test: `config_missing_is_defaults_and_unparsable_is_an_error` (tempdir paths, no real home).
- **Touch ID cancel → exit 3.** `procedures::approve` turns the `SignError` into plain text (`anyhow!("{e}")`), so its type is lost. But the text is exactly `SignError::Cancelled.to_string()` ("没有通过指纹确认"), and `apply_confirmation` passes the error up unchanged. `touch_id_cancelled` compares against the enum's own Display, using `e.chain()`. A match gives exit 3 with a `declined` event saying the draft stays in dcv and is not in effect. Any other approval error still gives exit 1. This depends on the message text. It would be sturdier if `procedures::approve` returned a typed error, but that file is outside my scope. Test: `a_cancelled_touch_id_is_the_user_saying_no` covers cancelled → 3/declined and another error → 1.
- **Repair yes-path test.** `a_repaired_draft_is_approved_under_its_new_name_never_the_original`: an existing approved `promote-x-post`, replay fails, `explore_from` takes over, the draft is written with `--name promote-x-post-0001` and `--draft-of promote-x-post`, the user says yes, and `approve_proc` is called only with `social-global:promote-x-post-0001`. It passed on the first run: the existing code already did this, so this test was not part of the RED set.
- **Nothing else writes to stdout in --json mode.** Audit:
  - `DcoClient`, `DcoHands` and `LlmModel` never print.
  - `CliBackend` pipes the child's stdout, and `RealDcv`/`security` use `.output()`.
  - `procedures::approve`, `campaign::load` and `judge::from_config` don't print. The `println!` calls in `keys`, `campaign`, `judge` and `procedures` are all in their own `run_cli` commands, which `dct do` never reaches.
  - `Config::load` is no longer called.

  The early-error path no longer uses `println!` and goes through `Talk::tell`. Test: `dct_do_writes_to_stdout_only_through_say` checks that the non-test source of `explore_cli.rs`, `io.rs` and `real.rs` has no `println!(`, `print!(` or `dbg!(`. That is a source-level guard: process stdout can't be captured inside a unit test.
- **Duplicate test types removed.** The tests now use `super::{NoFix, NoJeff}`.

RED (after the tests were written, with a `todo!()` `load_config`):
```
test explore_cli::tests::a_cancelled_touch_id_is_the_user_saying_no ... FAILED
test explore_cli::tests::config_missing_is_defaults_and_unparsable_is_an_error ... FAILED
test explore_cli::tests::dct_do_writes_to_stdout_only_through_say ... FAILED
test result: FAILED. 29 passed; 3 failed
```
GREEN: `cargo test --lib explore_cli` gives 32 passed, 0 failed.

Full check:
- Workspace: 2346 passed, 0 failed, 5 ignored. The tree included the other fixer's commits up to `3a2a0be` and their uncommitted dct-explore test edits.
- `cargo clippy --all-targets`: 0 warnings.
- rustfmt `--check` on my three files: clean.
- Windows target: only the pre-existing `student_projects.rs:669` warning.

## Fix round 2

Commit: see `git log` ("dct do: config errors give path, line and column only, never the source line or quoted values"). Only `src/explore_cli.rs` is staged.

- `load_config` now parses with `toml::from_str` directly, so it gets a `toml::de::Error`. The new `config_error_text` builds `<path> 解析不了（第 L 行第 C 列：<message>）`. The line and column come from `span()` over the file text (a sliced prefix with a safe `get`, so a bad offset can't panic). The source snippet from toml's own Display is never used.
- `scrub_toml_message` cleans `e.message()`:
  - Anything in double quotes is a value and becomes `"…"`.
  - Anything in backticks is kept only if it is a short lowercase word (24 bytes or fewer, `[a-z0-9_]`) or a single punctuation character. Everything else becomes `` `…` ``.
  - Newlines become 「；」 and control characters are dropped.

  Without this, serde's messages would leak values even with the source line removed: `invalid type: string "…"` and `unknown variant `sk-…``. The RED run below caught the second one leaking.
- Test `a_config_error_names_the_line_but_never_echoes_the_file` covers four cases:
  - a syntax error, `api_key = sk-SECRET123 oops` on line 3: the path and 第 3 行 are present, SECRET is not;
  - a type error, `threshold = "sk-SECRET456"`;
  - an unknown enum variant, `transport = "sk-SECRET789"`: no SECRET, and `` `cli` `` is kept;
  - a single-line message with no newline that keeps its "expected …" tail.

RED:
- First run: the message included `3 | api_key = sk-SECRET123 oops`.
- After the first cut, the extended test failed again on `unknown variant `sk-SECRET789``, and a backtick inside a quote swallowed the end of the message.

GREEN: `cargo test --lib explore_cli` gives 33/33.

Full check:
- Workspace: 2355 passed, 0 failed, 5 ignored (the tree includes the other fixer's commits).
- clippy: 0 warnings.
- rustfmt `--check` on my files: clean.
- Windows target: only the pre-existing `student_projects.rs:669` warning.

## Fix round 3

Commit: "dct do: config errors carry only path, line, column and a fixed category; drop toml's message". Only `src/explore_cli.rs` is staged.

- `scrub_toml_message` and `config_error_text` are removed. No part of toml's or serde's message is used any more.
- `parse_config` works in two steps:
  1. `text.parse::<toml::Table>()`. If this fails, the category is `语法错误`.
  2. If it succeeds, `toml::from_str::<Config>`. If that fails, the category is `字段的值不对`.

  This tells the two kinds apart by which step failed, not by matching error text. The only things kept from the error are `span().start` and the category.
- `line_col` turns the byte offset into `第 L 行第 C 列：`, using a safe `get` on the slice. If there is no span it adds nothing.
- The final text is `<path> 解析不了（第 L 行第 C 列：<category>）`.
- Test `a_config_error_names_the_line_but_never_echoes_the_file` has five cases:
  - a syntax error (`api_key = sk-SECRET123 oops`);
  - a type error (`"sk-SECRET456"` in a float field);
  - an unknown enum value (`"sk-SECRET789"`);
  - an escaped quote (`"ab\"cd"`): neither `ab` nor `cd` appears;
  - an integer secret (`provider = 123456789012`): neither the number nor `12345` appears.

  Each case checks that the path, the right line number and the right category are present. It also checks that the text after the path has no banned fragment and no `expected`, backtick or double quote, i.e. none of toml's own message.

RED: the new test failed on the first case. The message still carried toml's text ("invalid string；expected `\"`, `'`") and no category.
GREEN: `cargo test --lib explore_cli` gives 33/33.

Full check:
- Workspace: 2357 passed, 0 failed, 5 ignored.
- clippy: 0 warnings.
- rustfmt `--check` on my files: clean.
- Windows target: only the pre-existing `student_projects.rs:669` warning.

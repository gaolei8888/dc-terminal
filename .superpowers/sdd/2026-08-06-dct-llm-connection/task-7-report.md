# Task 7 report: resolve a backend from config + profile + secrets, add `dct llm check`

## What was done

- Created `src/llm/resolve.rs`: `ResolveError`, `describe()`, and `resolve()` exactly as given in
  the brief. Credential order is fixed as specified — `Transport::Cli` resolves to
  `Credential::Inherit` and consults no credential store; `Transport::Http` tries
  `SecretStore` first, then the injected `oauth` closure, then `NoCredential`.
- Added `pub mod resolve;` to `src/llm/mod.rs`.
- Added `pub fn llm_check() -> i32` to `src/cli.rs`, body copied verbatim from the brief.
- Wired `dct llm check` into `src/main.rs`'s existing subcommand `match` (see "How wired" below)
  and added one line to the `HELP` text.
- One necessary addition not in the brief: a blanket `impl fmt::Debug for dyn Backend` in
  `src/llm/mod.rs` (see "Deviation" below) — required for the brief's own test code to compile.

## Test commands and actual output

Baseline before touching anything:
```
$ cargo test --lib
test result: ok. 446 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.74s
```

Red phase — `src/llm/resolve.rs` created containing **only** the test module from the brief,
and `pub mod resolve;` added to `mod.rs` so the compiler actually attempts to build it (see
"Confirmation of a real red phase" below):
```
$ cargo test --lib llm::resolve
error[E0433]: cannot find type `ResolveError` in this scope
  --> src/llm/resolve.rs:55:23
...
error[E0425]: cannot find function `describe` in this scope
  --> src/llm/resolve.rs:84:21
...
error: could not compile `dct` (lib test) due to 22 previous errors; 3 warnings emitted
```
22 real compile errors (`ResolveError`, `resolve`, `describe`, `Profile`, `Credential`,
`SecretStore` all unresolved) — not "0 tests ran."

After writing the implementation, first attempt still failed to compile:
```
error[E0277]: `dyn llm::Backend` doesn't implement `Debug`
    = note: required for `std::sync::Arc<dyn llm::Backend>` to implement `Debug`
note: required by a bound in `Result::<T, E>::unwrap_err`
```
4 occurrences, one per `.unwrap_err()` call in the brief's test module — see "Deviation."

After adding the `Debug` impl for `dyn Backend`:
```
$ cargo test --lib llm::resolve
running 7 tests
test llm::resolve::tests::every_error_explains_itself_in_a_self_contained_sentence ... ok
test llm::resolve::tests::an_unknown_provider_is_named_in_the_error ... ok
test llm::resolve::tests::http_needs_an_api_block ... ok
test llm::resolve::tests::a_profile_without_a_headless_command_is_refused_by_name ... ok
test llm::resolve::tests::http_without_any_credential_is_refused ... ok
test llm::resolve::tests::the_cli_transport_needs_no_credential_at_all ... ok
test llm::resolve::tests::http_uses_an_oauth_token_when_there_is_no_key ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 446 filtered out; finished in 0.00s
```

Full suite and build:
```
$ cargo test --lib
test result: ok. 453 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.24s

$ cargo build
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.64s
```
453 = 446 + 7, matching the expected count exactly. Only pre-existing warnings remain
(`unused variable: pid` in `src/session.rs`, from earlier tasks, untouched by this one).

```
$ cargo fmt -- --check
(clean)

$ git diff --check
(clean)
```

Manual smoke test (isolated `$HOME`, no real credentials or network touched), confirming
`dct llm check` runs end-to-end and produces a self-contained Chinese message:
```
$ HOME=$TMPHOME ./target/debug/dct llm check
provider: nope
transport: Cli
连不上：找不到叫「nope」的 agent。请在配置里把 provider 改成一个已有的名字。
exit=1
```

## Confirmation of a genuinely failing red phase

Per the carry-forward warning about a prior task where "0 tests ran" was mistaken for red,
I made sure the module was actually wired into compilation before checking for failure:
I wrote `src/llm/resolve.rs` containing only the `#[cfg(test)] mod tests` block from the
brief, and added `pub mod resolve;` to `src/llm/mod.rs` *before* running the test command
(the brief's Step 3 lists that mod-wiring under the implementation step, but doing it early
was necessary to force the compiler to actually try to build `resolve.rs` and fail for real,
rather than silently skip an undeclared file and report zero matching tests). The result was
22 genuine `E0433`/`E0425` compile errors naming every symbol the implementation was
supposed to provide (`ResolveError`, `resolve`, `describe`). This is unambiguously a real
red phase, not a vacuous one.

## Commit

SHA: `17ef482` on branch `feat/llm-connection`
```
feat(llm): resolve a backend from config, profile, and secrets

Credential order is fixed here: an explicitly entered key beats an OAuth
token we found in another program's storage, because what the user typed
should outrank what we guessed on their behalf.

The CLI transport resolves to Inherit and consults no credential store at
all. Adds 'dct llm check', which runs the configured connection for real —
this is what makes the 'verified against a live endpoint' bar checkable.
```
Files: `src/llm/resolve.rs` (new), `src/llm/mod.rs`, `src/cli.rs`, `src/main.rs`.
No AI/Co-Authored-By attribution line, per project convention.

## How the subcommand was wired, and why that matches local style

`src/cli.rs` does **not** itself contain the top-level `match` that dispatches subcommand
names — that dispatch lives in `src/main.rs` (`Some("ps") => dct::cli::run_ps(...)`,
`Some("stop") => { ... dct::cli::run_stop(...) ... }`, etc.). `src/cli.rs` only holds the
pure/testable subcommand *implementations* the dispatcher calls into. So I followed that
existing division of labor exactly:

- `src/cli.rs`: added `pub fn llm_check() -> i32` (body verbatim from the brief) alongside
  the other `run_*` functions, right before the `#[cfg(test)] mod tests` block — the same
  place `run_prune` sits.
- `src/main.rs`: added one `match` arm, in the same style as the existing ones (`prune`
  takes no arguments and is commented on why; `stop`/`kill` parse trailing args). Since
  `llm check` is two tokens, I matched on `Some("llm") if args.get(1).map(|s| s.as_str())
  == Some("check")`, mirroring how `stop`/`kill` slice `args[1..]` for their own sub-args.
  Exit code is propagated with `std::process::exit(...)`, same pattern as `stop`/`kill`.
  Also added one line to the `HELP` constant, matching how every other subcommand is listed
  there (not requested by the brief, but omitting it would leave `llm check` undiscoverable
  next to every sibling command that *is* documented).

This diverges from the brief's Files list, which named only `src/cli.rs` as needing
modification — but the brief's own instruction ("follow the existing subcommand style
already in `src/cli.rs`... read how current subcommands are dispatched and registered")
only makes sense once you see that "dispatched and registered" actually happens in
`main.rs`. I judged matching the real local idiom (Files list notwithstanding) to be more
faithful to that instruction than leaving `llm_check` unreachable from the binary.

## Deviation from the brief

The brief's test module calls `.unwrap_err()` on `Result<Arc<dyn Backend>, ResolveError>`
in four tests. `Result::unwrap_err` requires `T: Debug`, and the `Backend` trait (from Task
4) has no `Debug` bound, so `Arc<dyn Backend>` does not implement `Debug` — this does not
compile as given. This is a genuine gap in the brief surfaced by actually running the red
phase, not a design ambiguity, so I fixed it minimally rather than asking: added

```rust
impl fmt::Debug for dyn Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<llm backend>")
    }
}
```

to `src/llm/mod.rs`, right after the trait definition, with a comment explaining why (it
exists solely so `Arc<dyn Backend>` satisfies `Debug`-bound call sites like
`resolve`'s tests; it never reads `self`, so it can't leak anything, and it does not require
`CliBackend` or `HttpBackend` themselves to derive `Debug` — which would have been a much
larger, riskier change, since `CliBackend` holds an `Arc<dyn Fn>` runner that isn't `Debug`
at all).

## Self-review

- Credential precedence matches the spec exactly: `Cli` → `Inherit`, no store touched;
  `Http` → `SecretStore` first, then `oauth(name)`, then `NoCredential`. Verified by
  `http_without_any_credential_is_refused` and `http_uses_an_oauth_token_when_there_is_no_key`.
- The `oauth` closure in `llm_check` wraps `read_claude_oauth()`'s bare `String` in
  `Credential::Bearer` in the same expression via `.map(Credential::Bearer)` — no bare token
  is ever bound to a named variable, logged, or interpolated, per the Task 3 carry-forward.
- `describe()` builds messages only from the provider name (`n: &String`), never from a
  `Credential` — grepped `resolve.rs` to confirm no credential value is ever formatted into
  a `describe()` string. All four messages name the provider and state a concrete next
  action ("在配置里把 provider 改成...", "换一个 provider", "把 transport 改回 cli",
  "在主界面按 c 填一个"), and the `every_error_explains_itself_in_a_self_contained_sentence`
  test enforces length and forbids the word "Error".
- No new crate dependencies were added; `Cargo.toml` untouched.
- `cargo fmt`, `cargo test --lib` (453/453), `cargo build`, and `git diff --check` all clean.
- Ran a manual smoke test of the built binary against an isolated `$HOME` with an unknown
  provider, confirming `dct llm check`'s error path end-to-end without touching any real
  credentials or network — output matches `describe()`'s wording.

---

# Fix round 1 (code review response)

Review returned spec PASS with one CRITICAL and three Important findings, plus two cheap
"also fix" items. All addressed below.

## CRITICAL: Anthropic OAuth token would be sent to third-party vendors

`llm_check`'s `oauth` closure mapped `"claude" | "kimi" | "glm" | "deepseek" | "qwen-api"`
all to `read_claude_oauth()`. Those four non-claude profiles' `[api].base_url` point at
`api.moonshot.cn`, `open.bigmodel.cn`, `api.deepseek.com`, and `dashscope.aliyuncs.com` —
unrelated third-party servers. `send_real` (`src/llm/http.rs`) puts whatever `Credential`
it's given into the `Authorization: Bearer` header, so this sent the user's Anthropic OAuth
token to those four vendors. Worse, `claude` itself has no `[api]` block (it's CLI/SSO-only),
so `NoApiEndpoint` fires before any credential use for `provider = "claude"` — meaning
exfiltrating the token to a third party was the *only* reachable effect of that match arm.

**Fix:** extracted the mapping into a small, named, independently testable function in
`src/cli.rs`:

```rust
fn oauth_lookup(
    name: &str,
    claude: &dyn Fn() -> Option<crate::llm::creds::Credential>,
    codex: &dyn Fn() -> Option<crate::llm::creds::Credential>,
) -> Option<crate::llm::creds::Credential> {
    match name {
        "claude" => claude(),
        "codex" => codex(),
        _ => None,
    }
}
```

`kimi`/`glm`/`deepseek`/`qwen-api` now always get `None` — they have no OAuth relationship
with the user and must go through an explicitly entered key (`SecretStore`). A doc comment
on `oauth_lookup` states the rule (a CLI's OAuth may only ever be offered to that CLI's own
endpoint) and explains why, so it isn't re-added later. `llm_check` now calls
`oauth_lookup(n, &|| read_claude_oauth().map(Credential::Bearer), &read_codex_auth)`.

**Test** (`src/cli.rs`, `oauth_lookup_never_offers_one_vendors_token_to_another`): injects
fake `claude`/`codex` closures (never touches Keychain or `auth.json`), asserts `"claude"` and
`"codex"` get their own fake token back, and asserts all four vendor names get `None`
regardless of what the fake closures return.

## IMPORTANT (a): credential precedence had no regression protection

None of the 7 original tests supplied both a `SecretStore` key and an `oauth` result for the
same provider, so flipping `.or_else()`'s order (trying OAuth before the key) would still
pass all 7.

**Fix:** extracted the precedence logic out of `resolve()` into its own function,
`select_credential(name, secrets, oauth) -> Result<Credential, ResolveError>`, in
`src/llm/resolve.rs`. Added `an_explicit_key_outranks_an_oauth_token_found_elsewhere`: builds
a real `SecretStore` (via `tempfile::tempdir()` + `SecretStore::set`) with a key for `"kimi"`,
passes an `oauth` closure that also returns `Some(Credential::Bearer(...))`, and asserts the
result equals `Credential::Key("sk-explicit-key")` via `assert_eq!` (`==`, never formatted —
`Credential`'s `Debug` stays redacting).

## IMPORTANT (b): reverted the blanket `impl Debug for dyn Backend`

That impl was added in the original round to satisfy `.unwrap_err()`'s `T: Debug` bound in
four tests. Per review, it traded a compile-time tripwire (nothing in the codebase can
accidentally `{:?}`-print a `Backend`, a type family that transitively holds `Credential`)
for test-only convenience — worth removing even though the impl itself never read `self` and
so couldn't currently leak anything.

**Fix:** removed the impl and the now-unused `use std::fmt;` from `src/llm/mod.rs`. Rewrote
the four affected assertions in `src/llm/resolve.rs` to use
`assert!(matches!(r, Err(ResolveError::X(ref n)) if n == "..."))`, matching the idiom at
`src/secrets.rs:296` (`assert!(matches!(code, ErrorCode::SecretsFileBroken { .. }))`) — no
`Debug` bound needed at all.

## IMPORTANT (c): `describe()` jargon and a trivially satisfiable test

The original four messages contained the literal English words `provider`, `transport`, and
`cli` (Rust field/type names echoed straight into user-facing Chinese), and the test only
checked `contains('x')`, length `> 8`, and absence of the word `"Error"` — a string like
`"x xxxxxxxxx"` would have passed.

**Fix:** rewrote all five messages (four original plus the new `NoModel`) in plain Chinese
with no internal field/type names, each naming the provider and a concrete next step:

| Error | Message |
|---|---|
| `NoSuchProvider(n)` | 设置文件里写的「{n}」不是 dct 认识的名字，把它换成 claude 试试。 |
| `NoHeadlessCommand(n)` | 「{n}」还没法自己在后台回答问题，把设置文件里这一项换成 claude 试试。 |
| `NoApiEndpoint(n)` | 「{n}」没有可以直接连接的网址，把设置文件里"直连"这一项关掉，改回让它自己登录。 |
| `NoCredential(n)` | 「{n}」还没有密钥。在主界面按 c 填一个。 |
| `NoModel(n)` | 「{n}」还没有指定用哪个型号，把设置文件里这一项填一个具体的型号名。 |

`NoCredential`'s "在主界面按 c 填一个" is a real, existing UI affordance (verified against
`src/ui/board.rs`/`src/ui/grid.rs`: `KeyCode::Char('c') if is_plain_key(&key) => open_secrets(app)`),
not a hypothetical action. There is currently no in-app screen for editing `[llm]`
provider/transport (`src/ui/settings_view.rs` only has language selection today, confirmed by
reading the file), so the other three messages point at the settings file — the one lever
that genuinely exists right now — using plain Chinese ("设置文件里这一项") rather than
internal identifiers.

**Test**, following the pattern at `src/secrets.rs:338-347`
(`every_error_explains_itself_in_plain_chinese_with_a_real_next_step`, renamed from
`..._self_contained_sentence`): for all five variants, asserts (case-insensitively) the
absence of `provider`, `transport`, `cli`, `agent`, `error`, and separately asserts each of
the first four messages contains `"设置文件"` and the credential message contains `"按 c"`.

## Also fixed (cheap, Task 9 depends on it)

- **Wire-specific HTTP path**: `resolve()` hardcoded `/v1/messages` regardless of wire.
  Extracted `http_url(base, wire) -> String` (`Wire::Anthropic` → `/v1/messages`,
  `Wire::Openai` → `/v1/chat/completions`), tested directly in
  `anthropic_and_openai_wires_hit_different_paths` (also covers the trailing-slash-on-base
  case). Checked all four `[api]`-bearing builtin profiles (`kimi`, `glm`, `deepseek`,
  `qwen-api`) — all currently use `wire = "anthropic"`, so this fix is currently latent for
  builtins but matters for any future/custom `Wire::Openai` profile, which the schema already
  allows.
- **No more guessed default model**: `cfg.llm.model.clone().unwrap_or_else(|| "claude-3-5-sonnet"...)`
  replaced with `.ok_or_else(|| ResolveError::NoModel(name.to_string()))?`. Added
  `ResolveError::NoModel(String)` variant, its `describe()` arm, and test
  `http_without_a_model_is_refused_instead_of_guessing_one`.

## Exact commands and actual output

```
$ cargo build
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.18s
(only pre-existing `unused variable: pid` warnings in src/session.rs, untouched by this task)

$ cargo test --lib
test result: ok. 457 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.30s

$ cargo fmt -- --check
(clean, no output)

$ git diff --check
(clean, no output)
```

457 = 453 (prior) + 4 new tests: `oauth_lookup_never_offers_one_vendors_token_to_another`
(`src/cli.rs`), `an_explicit_key_outranks_an_oauth_token_found_elsewhere`,
`http_without_a_model_is_refused_instead_of_guessing_one`, and
`anthropic_and_openai_wires_hit_different_paths` (all three in `src/llm/resolve.rs`). Note
`every_error_explains_itself_in_a_self_contained_sentence` was strengthened in place (renamed
`..._in_plain_chinese_with_a_real_next_step`), not added as a new test, and now also covers
`NoModel`.

One test needed a second pass: `anthropic_and_openai_wires_hit_different_paths` initially used
`"https://x.test/v1"` as the fake base and failed with
`left: "https://x.test/v1/v1/messages", right: "https://x.test/v1/messages"` — the real
builtin profiles' `base_url` values (e.g. kimi's `https://api.moonshot.cn/anthropic`) never
include a `/v1` segment, so appending `/v1/messages` is correct against them; my fake test
base was unrepresentative. Fixed by using a bare `"https://x.test"` fixture instead.

Manual smoke test confirming the CRITICAL fix's real-world behavior (isolated `$HOME`, no
real network/credentials touched — `kimi` now gets `NoCredential` instead of silently trying
to read the real macOS Keychain for a Claude token and sending it to Moonshot):
```
$ HOME=$TMPHOME ./target/debug/dct llm check   # config: provider = "kimi", transport = "http"
provider: kimi
transport: Http
连不上：「kimi」还没有密钥。在主界面按 c 填一个。
exit=1
```

## Commit

SHA: `cbc70d0` on branch `feat/llm-connection`, message:
```
fix(llm): stop offering Anthropic OAuth to third-party vendor endpoints
```
(full body in git log; covers the CRITICAL finding and all three Important findings plus the
two cheap fixes). Files: `src/cli.rs`, `src/llm/mod.rs`, `src/llm/resolve.rs`,
`docs/superpowers/plans/2026-08-06-dct-llm-connection.md`. English, no AI/Co-Authored-By
attribution line, per project convention. The unrelated `.superpowers/sdd/.gitignore` change
present in the working tree (from the sdd-workspace tooling, per its own diff comment) was
again left out of this commit.

## Plan doc update

`docs/superpowers/plans/2026-08-06-dct-llm-connection.md`, Task 7 section: replaced the
vendor→Claude-OAuth `match` block with the corrected `oauth_lookup`-based version and a
comment explaining why, and added a "Fix round 1" addendum summarizing all four review
findings and the two cheap fixes, so a future reader of the plan doesn't reconstruct a design
that already shipped with a credential-exfiltration bug.

## Self-review

- Verified by reading `src/llm/http.rs::send_real` that `Credential::Bearer`/`Key` really do
  go into the `Authorization` header sent to whatever `url` the caller constructed — that's
  what made the original mapping a real vulnerability, not just a style issue.
- Verified by reading all four vendor builtin profiles (`profiles/kimi.toml`, `glm.toml`,
  `deepseek.toml`, `qwen-api.toml`) that their `[api].base_url` values match exactly what the
  review cited, and confirmed none of them has any legitimate OAuth relationship with the
  user (they're plain API-key services).
- Verified `oauth_lookup`'s test never calls `read_claude_oauth`/`read_codex_auth` — only
  injects fake closures — so it cannot touch the real Keychain or `~/.codex/auth.json`,
  matching the constraint and this project's existing discipline in `src/llm/creds.rs`.
- Verified `NoCredential`'s "按 c" claim against the actual key-handling code in
  `src/ui/board.rs` and `src/ui/grid.rs`, and verified there is genuinely no UI for
  provider/transport today by reading `src/ui/settings_view.rs` in full, rather than assuming
  either way.
- Re-ran the full suite and build after every edit, not just at the end, to catch the
  `http_url` test's bad fixture immediately rather than bundling it with an unrelated change.
- No new crate dependencies added; no `unsafe`; no test reads real credentials or hits the
  network.

# Task 5 report: `dct-page` — public listing page and tokenless viewing mode

## Implemented

- `crates/dct-page/public.html` (new): the public listing page served at `GET /`. Polls
  `/live/public` every 5s (paused while `document.hidden`, immediate pull on
  `visibilitychange`), renders cards linking to `/live/<id>` using `textContent`
  only (title, lane names, viewer count are all untrusted free text).
- `crates/dct-page/src/lib.rs`: added `const PUBLIC_SRC = include_str!("../public.html")`
  and `pub fn public_page() -> &'static str` (no `<!--SHARED-->` substitution needed,
  since the listing page doesn't use the shared terminal-painting code). Added the
  three tests from the brief verbatim into `mod tests`.
- `crates/dct-page/live.html`:
  - Added `function tokenHeaders()` right after the `TOKEN` claim IIFE — returns
    `{ "x-live-token": TOKEN }` when a token exists, `{}` otherwise.
  - Replaced both `x-live-token` call sites (`fetchLanes`'s `headers: {...}` object
    literal, and `pull()`'s `var headers = {...}`) to route through `tokenHeaders()`.
  - Added `endedPublic` / `backToList` to both `STRINGS.zh` and `STRINGS.en`.
  - `render()` now special-cases `state === "ended" && !TOKEN`: shows the
    tokenless-appropriate message plus a link back to `/`, built with
    `createElement`/`textContent`/`appendChild` (no `innerHTML`).
- `crates/dct-srv/src/lib.rs`:
  - `pub use live::{Control, Live, PublicEntry};` (previously only `Live`).
  - `public_page_route()` now returns `dct_page::public_page()` instead of the
    Task-4 placeholder string.
- `crates/dct-srv/tests/serves.rs`: added `use dct_srv::Control` to the existing
  import line, and appended the `#[ignore]`d manual-acceptance test
  `serve_a_public_room_for_a_manual_look` exactly as specified in the brief
  (minus the already-present imports, see Deviations).

## TDD evidence

RED (before writing `public.html`/`public_page()`):
```
$ cargo test -p dct-page
error[E0425]: cannot find function `public_page` in module `super`
  --> crates/dct-page/src/lib.rs:235:24
error[E0425]: cannot find function `public_page` in module `super`
  --> crates/dct-page/src/lib.rs:244:52
error: could not compile `dct-page` (lib test) due to 2 previous errors
```

GREEN (after Steps 3–4):
```
$ cargo test -p dct-page
running 10 tests
test tests::the_public_page_is_here ... ok
test tests::the_live_page_omits_the_token_header_when_it_has_none ... ok
test tests::every_exit_the_student_page_has_is_a_get_to_a_live_path ... ok
test tests::the_page_is_actually_here ... ok
test tests::neither_page_is_empty ... ok
test tests::both_pages_share_one_painter ... ok
test tests::neither_page_ever_uses_inner_html ... ok
test tests::a_student_page_that_gets_no_new_frame_backs_off_instead_of_hammering ... ok
test tests::a_single_failed_frame_does_not_end_the_broadcast_for_the_student ... ok
test tests::the_student_page_has_no_way_to_send_anything_to_a_session ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Along the way, `neither_page_ever_uses_inner_html` actually went RED once more,
for a real reason — see Deviations §1 below — and was fixed and re-verified GREEN.

`cargo test -p dct-srv` (71 lib tests + 3 `tests/serves.rs` tests, the new manual
one `#[ignore]`d): all passed, no regressions from exporting `Control`/`PublicEntry`
or from swapping the placeholder route.

## Mutation checks (all done, all confirmed red then restored green)

1. `public.html`: `t.textContent = it.title;` → `t.innerHTML = it.title;`.
   `cargo test -p dct-page neither_page_ever_uses_inner_html` → **FAILED**
   (`public.html 里出现了 innerHTML`). Reverted → **ok**.
2. `live.html`: reverted `fetchLanes`'s `headers: tokenHeaders(),` back to
   `headers: { "x-live-token": TOKEN },`.
   `cargo test -p dct-page the_live_page_omits_the_token_header_when_it_has_none`
   → **FAILED** (`还有地方直接写死了令牌头`). Reverted → **ok**.

Final `cargo test -p dct-page` re-run after both mutations were undone: 10/10 pass.

## Manual acceptance scaffold

`cargo test -p dct-srv --test serves --no-run` compiles cleanly (no code run).

Additionally ran it briefly in the background and hit it with curl, then killed it:
```
公开页：http://127.0.0.1:59692/   （标题里那段 <script> 必须原样显示成文字）

$ curl -s http://127.0.0.1:59692/                  # public.html served, <!doctype html>...
$ curl -s http://127.0.0.1:59692/live/public
[{"id":"demo01","title":"手工验收 · <script>alert(1)</script>","lanes":["前端","后端"],"viewers":0}]
$ curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:59692/live/demo01/frame
200
$ curl -s http://127.0.0.1:59692/live/demo01/lanes
{"lanes":["前端","后端"],"viewers":0,"public":{"title":"手工验收 · <script>alert(1)</script>"}}
```
Confirms: `/` serves the real `public.html`, `/live/public` lists the published
room with the raw (unescaped-at-server) title JSON — escaping is correctly the
client's job via `textContent`, tokenless `GET /live/{id}/frame` and
`/live/{id}/lanes` work for a public room. Process was killed immediately after
(`pkill -f serve_a_public_room_for_a_manual_look`); confirmed no longer running.

## Full verification before commit

```
$ env -u TERM cargo test --workspace --locked --no-fail-fast
... all crates: 0 failed (dct-srv: 71 + 3 passed, 1 ignored; dct-page: 10 passed;
    dct/dct-link/dct-terminal suites unaffected and green)
$ cargo clippy --workspace --all-targets --locked -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.70s   (no warnings)
$ git diff --check
(clean, no whitespace errors)
```

## Files changed

- `crates/dct-page/public.html` (new)
- `crates/dct-page/live.html` (modified)
- `crates/dct-page/src/lib.rs` (modified)
- `crates/dct-srv/src/lib.rs` (modified)
- `crates/dct-srv/tests/serves.rs` (modified)

Committed as `583a37a`: `feat(page): public listing page and tokenless viewing of public rooms`.

## Deviations from the brief's reference code

1. **`public.html`'s HTML comment could not literally contain the word
   "innerHTML".** The brief's own reference comment text reads `**只用
   textContent**，不许 innerHTML`. Because `neither_page_ever_uses_inner_html`
   scans the *entire* file text (not just the `<script>` body) for the literal
   substring `innerHTML`, that comment would make the test fail against the
   file it's describing — confirmed by actually hitting this failure
   (`public.html 里出现了 innerHTML`) on the first test run after writing the
   brief's exact HTML. Fixed by rewording the comment to describe the rule
   without using the literal API names (`innerHTML`/`outerHTML`/
   `insertAdjacentHTML`) anywhere in the file, including in the comment
   itself. No functional change — same rule, same enforcement, just phrased
   so the guard doesn't trip on its own documentation.
2. Everything else (file paths, function signatures — `public_page() ->
   &'static str`, `Live::publish`/`Live::push`/`dct_srv::serve`, `Control`,
   `PublicEntry`, `PublishKeys::add`/`save`, `KeyFile::open`/`keys()`,
   `Routes::LiveOnly`) matched the real source exactly as given in the brief;
   no other deviations were needed. Confirmed via direct reads of
   `crates/dct-srv/src/lib.rs` and `crates/dct-srv/src/live.rs` before writing
   any code.
3. `tests/serves.rs` already imported `Duration`, `Config`, `Relay`, `Routes`,
   `Live`, and `Arc` at the top of the file; only `Control` needed to be added
   to the existing `use dct_srv::{...}` line — no duplicate/new `use` block
   was introduced.

## Concerns

None outstanding. The manual scaffold test is left `#[ignore]`d as instructed.

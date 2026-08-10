# Task 6 report: 四处显示

## What changed, and where

- `src/ui/widgets.rs`
  - Added `pub(crate) fn session_label(s: &crate::session::SessionInfo) -> &str`,
    placed right before `short_path` (the other small session/path helper in this
    file). Returns `&s.tag` when non-empty, else `&s.profile`.
  - Added the failing-first test `session_label_falls_back_to_the_profile_when_there_is_no_tag`
    in `mod tests`, verbatim from the brief.

- `src/ui/board.rs`
  - Import line now includes `session_label`:
    `use super::widgets::{pad_to, session_label, status_label, status_style, truncate};`
  - Session row rendering (previously `board.rs:205-212`): the profile cell
    (`pad_to(&s.profile, 10)`) is replaced by a name cell built from
    `session_label`, and the `activity` truncation width is reduced to make
    room. See column arithmetic below.

- `src/ui/grid.rs`
  - Import line now pulls in both `session_label` and `truncate` (grid.rs did
    not import `truncate` before):
    ```rust
    use super::widgets::{
        char_width, pad_to, screen_to_lines, session_label, status_label, status_style, truncate,
    };
    ```
    (rustfmt wrapped it onto multiple lines; the set of names matches the brief.)
  - Tile title (`grid.rs:469` in the brief, now shifted a couple of lines by
    the new import): `format!("{} {} ", info.id, info.profile)` →
    `format!("{} {} ", info.id, truncate(session_label(info), 20))`.
  - Reply-box recipient (`grid.rs:358-361` in the brief): `s.profile` → `session_label(s)`
    inside the existing `.map(|s| format!("{} {}", s.id, session_label(s)))`.
  - Added the real (not skeleton) test `the_reply_box_is_addressed_by_name` in
    `mod tests`, described below.

- `src/ui/attach.rs`
  - No new import (per the brief, it reads `s.tag` directly).
  - Session title construction now renders `"{project} · {tag}"` when `tag`
    is non-empty, else just `project`, and passes that combined string
    (renamed `here`, was `project`) into the existing
    `crate::i18n::msg::session_title[_disconnected]` calls. The signature of
    `session_title` itself is untouched, exactly as the brief requires.

## Column arithmetic for the list row (board.rs)

Old layout inside the session row (after the fixed 2+3+8 prefix for the
highlight gutter/id/status):

```
pad_to(&s.profile, 10)         // profile cell: 10 cols
truncate(&s.activity, 76)      // activity: truncated to 76, i.e. up to 77 cols when it truncates
```

New layout:

```
pad_to(&truncate(session_label(s), 15), 16)   // name cell: 16 cols
truncate(&s.activity, 70)                      // activity: truncated to 70 (≤71 cols when truncated)
```

Reasoning, matching the comment copied verbatim from the brief into the code:

- The name cell grows from 10 → 16 columns (+6). Longest built-in profile
  name is 8 columns (`opencode`/`deepseek`/`qwen-api`); a user-chosen name is
  described as being up to 12 CJK characters (12 × 2 = 24 display columns,
  but the budget here is capped at 16, consistent with the project-name
  column elsewhere in the same file).
- `truncate(s, max)` returns up to `max + 1` display columns when it actually
  truncates (the `…` is appended *after* the length check). To make the
  *padded* cell land on exactly 16 columns, `truncate` is called with `15`,
  not `16` — mirroring the existing project-name cell two lines up
  (`pad_to(&truncate(&g.name, 17), 18)`), which the brief points to as the
  precedent for this exact off-by-one convention.
- The `activity` truncation shrinks from 76 → 70 (−6), exactly offsetting the
  name cell's +6, so the **total row width is unchanged**: `2+3+8+16+70 ==
  2+3+8+10+76`.

## Tests

### Exact commands and pre-change failure lines

Step 2 (before implementing `session_label`), both new tests fail to *compile*
the whole crate because `session_label` does not exist yet:

```
$ cargo test --lib ui::widgets::tests::session_label_falls_back_to_the_profile_when_there_is_no_tag
error[E0425]: cannot find function `session_label` in this scope
   --> src/ui/widgets.rs:659:20
    |
659 |         assert_eq!(session_label(&s), "claude");
    |                    ^^^^^^^^^^^^^ not found in this scope
error: could not compile `dct` (lib test) due to 2 previous errors
```

```
$ cargo test --lib ui::grid::tests::the_reply_box_is_addressed_by_name
error[E0425]: cannot find function `session_label` in this scope
   --> src/ui/widgets.rs:659:20
   ...
error: could not compile `dct` (lib test) due to 2 previous errors
```

(Same underlying error for both — the crate doesn't compile at all until
`session_label` exists, which is the expected "FAIL" the brief describes.)

After implementing `session_label` and the four display sites, both pass:

```
$ cargo test --lib ui::widgets::tests::session_label_falls_back_to_the_profile_when_there_is_no_tag
test ui::widgets::tests::session_label_falls_back_to_the_profile_when_there_is_no_tag ... ok

$ cargo test --lib ui::grid::tests::the_reply_box_is_addressed_by_name
test ui::grid::tests::the_reply_box_is_addressed_by_name ... ok
```

### `the_reply_box_is_addressed_by_name` — written as real code, not a skeleton

Copied the structure of the existing `the_reply_box_names_who_it_is_addressed_to`
(three sessions in the same project, focus on the second, open the reply box
with `i`). Deviation from a literal copy: the other two sessions are given
`profile = "codex"` instead of the default `"claude"`. Reason: the target
session's own profile is `"claude"`, and after this task's change *every*
tile title also renders via `session_label` (falling back to profile when
there's no tag) — so sessions 1 and 3 would still show `"claude"` in their
own tile titles regardless of the reply-box fix, which would make a global
`!c.contains("claude")` assertion meaningless. Giving the untagged sessions a
different profile isolates the assertion to the session actually under test:

```rust
#[test]
fn the_reply_box_is_addressed_by_name() {
    let (mut app, _dir) = App::test_app();
    let mut sessions: Vec<SessionInfo> = (1..=3)
        .map(|i| {
            let mut s = session(i, SessionState::Idle);
            s.profile = "codex".into();
            s
        })
        .collect();
    sessions[1].profile = "claude".into();
    sessions[1].tag = "修登录白屏".into();
    app.set_sessions(sessions);
    app.view = View::grid(1);
    handle_key(&mut app, key(KeyCode::Char('i'))).unwrap();

    let c = grid_text(&mut app);
    assert!(c.contains("2修登录白屏"), "回复行要写名字，不是 profile：{c}");
    assert!(!c.contains("claude"), "有名字就不该再露出 profile：{c}");
}
```

## Mutation results (does each new test actually go red?)

1. `the_reply_box_is_addressed_by_name` — temporarily reverted only the reply
   "who" line in `grid.rs` (`session_label(s)` → `s.profile`), left the test
   as written, ran it:
   ```
   thread '...the_reply_box_is_addressed_by_name' panicked at src/ui/grid.rs:2371:9:
   有名字就不该再露出 profile：...→2claude：▌打字回复，或者直接回车表示同意
   test result: FAILED. 0 passed; 1 failed
   ```
   Confirmed red. Restored the fix, reran — green.

2. `session_label_falls_back_to_the_profile_when_there_is_no_tag` —
   temporarily reverted `session_label`'s body to `&s.profile` unconditionally
   (i.e. reverted the display logic the test exercises), ran it:
   ```
   thread '...session_label_falls_back_to_the_profile_when_there_is_no_tag' panicked:
   assertion `left == right` failed
     left: "claude"
    right: "修登录白屏"
   test result: FAILED. 0 passed; 1 failed
   ```
   Confirmed red. Restored the real implementation, reran — green.

Both tests fail when the corresponding display change is reverted and pass
with it in place, so neither is a test that "was already true regardless of
the change."

## Scoped and full test runs

```
$ cargo test --lib ui
test result: ok. 354 passed; 0 failed; 0 ignored; 0 measured; 313 filtered out
```
(One incidental failure on the first run of this command,
`ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`,
with message "没等到滚屏内容攒够" — a pre-existing timing-sensitive scroll
test unrelated to this task's files. It passed in isolation and passed again
on a clean rerun of the same `cargo test --lib ui` command; not touched by
this task's diff, see the "deviation" note below.)

```
$ cargo test --lib session
test result: ok. 112 passed; 0 failed; 0 ignored; 0 measured; 555 filtered out
```

Final full gate:

```
$ cargo fmt                                      # no further changes after the initial run
$ cargo clippy --all-targets -- -D warnings       # clean, no warnings
$ cargo test
test result: ok. 667 passed; 0 failed  (lib)
+ 8 integration test binaries, all "test result: ok", 0 failed
+ 0 doc-tests
```

## Deviations from the brief, with reasoning

1. **Step 1's second test was prose, not code** — written out in full as
   instructed, following `the_reply_box_names_who_it_is_addressed_to`'s
   skeleton, with the profile-swap adjustment explained above so the negative
   assertion (`!c.contains("claude")`) is actually meaningful rather than
   accidentally true.
2. **Import line wrapping in `grid.rs`** — the brief gives the import as one
   line; `cargo fmt` wraps it across multiple lines because it exceeds the
   configured width. The set of imported names is identical to what the
   brief specifies; only the formatting differs, and `cargo fmt` is
   authoritative here per the task's own constraints.
3. **One flaky pre-existing test observed, not caused by this change** —
   `ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
   failed once under `cargo test --lib ui` with a timing message
   ("没等到滚屏内容攒够"), unrelated to session naming/display and not in a
   file this task touches (`src/ui/mod.rs`, scroll-to-bottom-on-resize
   logic). It passed both in isolation and on a clean rerun of the same
   scoped command, and the final full `cargo test` run was 667/667 green, so
   no code change was made for it.

No other deviations. `src/proto.rs` was not touched; `PROTOCOL_VERSION`
untouched.

---

## Fix round 1: the two unbounded display sites

The coordinator's review found that only 2 of 4 display sites were bounded
(`board.rs`'s list row and `grid.rs`'s tile title). The other two —
`grid.rs`'s reply-box `who` and `attach.rs`'s title — passed `session_label`
straight through with no truncation. `session::NAME_MAX_CHARS` is **24
characters**, not 12: it's deliberate headroom for a model that ignores the
12-character instruction in the naming prompt, so a worst-case all-CJK tag is
**48 display columns**, not the 24 I'd checked against in the original report.
Fixed both sites and added width tests neither path had.

### What changed

- `src/ui/grid.rs` — the reply-box recipient now bounds the name the same way
  the tile title does:
  ```rust
  .map(|s| format!("{} {}", s.id, truncate(session_label(s), 20)))
  ```
  Id stays unbounded and first, per the coordinator's instruction — it's the
  one thing that must never be lost if a session gets stopped mid-type.

- `src/ui/attach.rs` — the tag is bounded before it goes into the title:
  ```rust
  format!("{project} · {}", truncate(&s.tag, 15))
  ```
  with a Chinese comment next to it explaining the 15 (see below). Also added
  `truncate` to the existing `use super::widgets::{...}` import line (attach.rs
  previously needed no new import for the base feature; this fix does).

- Added `grid_text_at(app, width, height)` in `grid.rs`'s test module — a
  parametrized twin of the existing fixed-120×30 `grid_text` helper, needed
  because the new width test has to render at both 80 and 60 columns.

- Added `screen_text` in `attach.rs`'s test module — attach.rs had no buffer-
  to-string helper before; copied board.rs's version verbatim (full-buffer
  scan, whitespace stripped), since CJK wide-char cells intersperse stray
  space glyphs in the continuation cell and every other screen-text helper in
  this codebase already strips them for that reason.

### Chosen widths, and why

**Reply box (`grid.rs`): bound = 20, same as the tile title.** The brief's
own instruction picked this number directly ("bound the name the same way the
tile title does") rather than asking me to compute a fresh one — reusing the
tile's existing 20-column policy also keeps "how wide can a name get" a single
answer instead of a third number to justify. `truncate(x, 20)` returns up to
21 display columns when it truncates.

**Attach title (`attach.rs`): bound = 15.** Computed from the actual title
layout, not guessed. Using `display_width` (the same function `truncate`/
`pad_to` use) on the literal pieces of `i18n::msg::session_title`:

| piece | zh | en |
|---|---|---|
| prefix (`会话 {id} · ` / `Session {id} · `, id=1) | 9 | 12 |
| separator (` · `) | 3 | 3 |
| suffix (` —— F2 返回看板` / ` —— F2 goes back`) | 15 | 16 |

English is the tighter case (12 + 3 + 16 = 31 fixed columns). At the
narrowest width this repo's width tests use (60 columns, 58 usable once the
2-column border is subtracted), that leaves 58 − 31 = 27 columns for
`project` + the name together. `project` in this view is normally just the
last path segment (`short_path`), so budgeting roughly half of that headroom
to the name and leaving the rest for `project` gives `truncate(&s.tag, 15)`
(≤16 columns once the `…` is added). This was **not** left as pure arithmetic
— I verified it by rendering the actual title row at width 60 and 80, in both
languages, with a full 24-CJK-char tag and a short project (`/w/a`), and
confirmed the "F2 返回看板" / "F2 goes back" hint survives intact in all four
combinations before writing the permanent width test. The reasoning above is
also inline as a Chinese comment at the call site in `attach.rs`.

### New width tests

`src/ui/attach.rs`, `a_long_name_never_pushes_the_way_back_off_the_title`:
renders an attached view with a 24-CJK-char tag (`"修".repeat(24)`, 48 display
columns) at 80 and 60 columns, asserts the screen text contains `"F2返回看板"`
(whitespace-stripped) at both widths.

`src/ui/grid.rs`, `a_long_name_never_pushes_the_draft_off_the_reply_row`:
renders a grid reply box with the same 24-CJK-char tag at 80 and 60 columns,
typing a longer, realistic draft (`"继续，麻烦顺手也把这个改掉"`, 14 characters
/ 28 columns — see "why not a 2-character draft" below), and asserts both the
literal draft text and the `▌` cursor are present in the rendered output at
both widths.

**Why not a 2-character draft.** My first version of the reply-box test typed
just `"继续"` (2 characters). Measuring it after the fact: the recipient
prefix (`→ {id} {name}：`) with an *unbounded* 48-column name comes to 54
columns; `"继续"` + the cursor add 5 more, for 59 total — one column under the
60-column test width. The test passed with the bug still in the code, purely
by accident of that one-column margin; the mutation check below caught this
(see the first, failed attempt in the mutation section). Switching to the
14-character draft pushes the unbounded total to 54 + 28 + 1 = 83 columns,
which overflows *both* 60 and 80, so the test is a genuine regression check
at both widths, not a coincidence of the exact string picked.

### Test commands and mutation results

Scoped runs, plus the required one full run at the end:

```
$ cargo test --lib ui::attach::tests::a_long_name_never_pushes_the_way_back_off_the_title
test result: ok. 1 passed

$ cargo test --lib ui::grid::tests::a_long_name_never_pushes_the_draft_off_the_reply_row
test result: ok. 1 passed

$ cargo test --lib ui
test result: ok. 356 passed; 0 failed  (one incidental failure on a prior run of this
same command, see "pre-existing flaky test" below — not from this fix)

$ cargo fmt && cargo clippy --all-targets -- -D warnings
clean, no warnings

$ cargo test
test result: ok. 669 passed; 0 failed  (lib) + all integration binaries green
```

**Mutation 1 — reply box.** Reverted `truncate(session_label(s), 20)` back to
plain `session_label(s)` and reran the *2-character-draft* version of the test
first (before switching to the 14-character draft): it stayed **green**,
proving that version wasn't discriminating (documented above). Rewrote the
draft to the 14-character string, reran the same revert:

```
thread 'ui::grid::tests::a_long_name_never_pushes_the_draft_off_the_reply_row' panicked at src/ui/grid.rs:2418:13:
80 列下一个 48 列宽的名字把光标顶出了屏幕：┏▶1修修修修修修修修修修…空闲a┓┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃┃→1修修修修修修修修修修修修修修修修修修修修修修修修：继续，麻烦顺手也把这个改掉
test result: FAILED. 0 passed; 1 failed
```
Red, at width 80 (the loop stops at the first failing width). Restored
`truncate(session_label(s), 20)` — green again.

**Mutation 2 — attach title.** Reverted `truncate(&s.tag, 15)` back to plain
`s.tag`, reran:

```
thread 'ui::attach::tests::a_long_name_never_pushes_the_way_back_off_the_title' panicked at src/ui/attach.rs:436:13:
80 列下一个 48 列宽的名字把退出提示顶出了标题：┌会话1·/w/a·修修修修修修修修修修修修修修修修修修修修修修修修——F2返回看─┐│...
test result: FAILED. 0 passed; 1 failed
```
Red — the rendered hint is literally cut to `"F2返回看"`, missing the final
`板`, confirming the exact failure mode the coordinator described. Restored
`truncate(&s.tag, 15)` — green again.

### Pre-existing flaky test, observed again

`ui::tests::entering_a_session_always_lands_at_the_bottom_even_without_a_resize`
(in `src/ui/mod.rs`, not touched by this task) failed once more under
`cargo test --lib ui` and once under the full `cargo test` run, both times
with "没等到滚屏内容攒够" — the same timing-sensitive scroll test flagged in
the original report. Passed in isolation and on a clean rerun both times;
final full `cargo test` was 669/669 green.

### Note on `.superpowers/sdd/.gitignore`

An unrelated file, `.superpowers/sdd/.gitignore`, kept showing up modified
(rewritten to a bare `*`) after running `cargo test`/`cargo clippy` in this
session — its own committed comment says some `sdd-workspace` tooling does
this on every run and expects it to be reverted afterward. Reverted it with
`git checkout --` before each commit; not part of this task's diff.

### Deviations

None beyond what's noted above (the draft-string revision, made mid-task
after the mutation check caught the first version not discriminating). Did
not touch `board.rs` or `widgets.rs` (already-bounded sites, per instruction).
Did not change `NAME_MAX_CHARS`.

---

## Fix round 2: the disconnected title still overflowed

The coordinator's second review found that `truncate(&s.tag, 15)`'s budget
was computed against `session_title` (the connected string), but
`attach.rs`'s `draw` picks `session_title_disconnected` whenever
`app.connected == false`. That string adds a whole extra clause
(`（连接已断开，画面可能过期）` / `(disconnected, may be out of date)`), so
even a 15-column-bounded name could still push the "F2" exit hint off-screen
on the disconnected path — the one state where a user most needs to see how
to leave.

### What changed

`src/ui/attach.rs` — the `here` string now omits the name entirely (not just
truncates it further) whenever `app.connected` is false, regardless of
whether `s.tag` is set:

```rust
if s.tag.is_empty() || !app.connected {
    project
} else {
    // ...truncate(&s.tag, 15) path, unchanged...
}
```

with a Chinese comment above the `if` explaining the product reasoning the
coordinator asked for: when disconnected, the line's budget belongs to the
warning and the way out, not the name — the user is already looking at the
session, so the name is the least useful thing on that line.

**Did not touch** `session_title_disconnected` itself, the i18n strings, or
try to make the pre-existing (name-independent) overflow fit — confirmed by
probe (below) that this is a separate, already-existing problem, out of
scope per the coordinator's explicit instruction.

### Verifying the pre-existing overflow, and why the new test only checks Chinese

Before writing the permanent test, I rendered the disconnected title with an
**empty** tag (i.e. with none of this feature's changes in play) at 80 and 60
columns, both languages, using a temporary probe test (removed before the
final diff):

```
Zh width=80 contains hint: true
Zh width=60 contains hint: true
En width=80 contains hint: true
En width=60 contains hint: false   :: "...{id=1}·/w/a(disconnected,maybeoutofdate)——F2"
```

The English string at 60 columns already loses "goes back" with **zero**
characters of name — that overflow predates this branch entirely, exactly as
the coordinator described (title around 106 columns pre-truncation). Asserting
the hint's presence there would make the new test fail regardless of whether
this feature's own fix is correct, which isn't a fair test of this feature.
The Chinese string fits at both widths with an empty tag, so the new test
uses `Lang::Zh` (the default) — a clean way to prove "omit the name" is
sufficient without also asserting on a hole this fix isn't responsible for
and isn't in scope to close.

### New test

`src/ui/attach.rs`,
`a_disconnected_title_drops_the_name_to_save_room_for_the_way_out`: builds a
disconnected attached view (`app.connected = false`) with a 24-CJK-character
tag (`"修".repeat(24)`, 48 display columns), at 80 and 60 columns, and
asserts both that the screen text contains `"F2返回看板"` and that it does
**not** contain `'修'` (the name).

### Test commands and mutation result

```
$ cargo test --lib ui::attach::tests::a_disconnected_title_drops_the_name_to_save_room_for_the_way_out
test result: ok. 1 passed

$ cargo test --lib ui::attach::tests
test result: ok. 30 passed; 0 failed

$ cargo test --lib ui
test result: ok. 357 passed; 0 failed  (no flake this run)

$ cargo fmt && cargo clippy --all-targets -- -D warnings
clean, no warnings

$ cargo test
test result: ok. 670 passed; 0 failed  (lib) + all integration binaries green
```

**Mutation.** Changed the guard from `s.tag.is_empty() || !app.connected` back
to `s.tag.is_empty()` (i.e. put the name back into the disconnected branch)
and reran the new test:

```
thread 'ui::attach::tests::a_disconnected_title_drops_the_name_to_save_room_for_the_way_out' panicked at src/ui/attach.rs:451:13:
80 列下断连标题不该再画名字，它把预算让给了警告和退路：┌会话1·/w/a·修修修修修修修…（连接已断开，画面可能过期）——F2返回看板─────┐│...
test result: FAILED. 0 passed; 1 failed
```

Red at width 80 (the loop stops at the first failing width — the name is
visibly back in the title as `修修修修修修修…`). Restored
`s.tag.is_empty() || !app.connected` — green again, and reran
`ui::attach::tests::a_long_name_never_pushes_the_way_back_off_the_title`
(the connected-path test from fix round 1) to confirm the connected path is
untouched — still green.

### Deviations

None. Did not modify `session_title_disconnected`, the i18n strings, or
attempt to fix the pre-existing name-independent overflow at 60-column
English — confirmed by probe that it predates this branch and is out of
scope, per the coordinator's explicit instruction.

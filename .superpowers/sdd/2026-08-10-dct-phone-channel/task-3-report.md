# Task 3 report: settings page becomes a list of settings

**Status:** complete, fix round 1 addressed. Commits `badc974` (original) and
`adacd36` (fix round 1) on `feat/phone-channel` (base `1a5344c`).

## Brief inaccuracies found, and what I did about them

The brief's Files list said only `src/ui/view.rs`, `src/ui/settings_view.rs`,
`src/i18n.rs`. Two more files needed real changes, and one of them hid an
actual behavioural bug the brief never anticipated.

### 1. `open_settings()` in `src/ui/mod.rs` — not listed, and its existing logic is now wrong (real bug, not just a missing file)

`open_settings()` (the `l`-key handler) pre-selected the cursor by finding
**the current language's position in `Lang::all()`** and feeding that index
straight into `View::Settings { state }`. That was correct when the top-level
list *was* the language list. After the refactor, `state` indexes into
`SettingsItem::all()` — an enum with no relationship to language count.

Left unfixed: a user whose current language is not `Lang::all()`'s first
entry (today, that's Chinese — `Lang::all() = [En, Zh]`, so `Zh` is index 1)
presses `l` and the cursor silently lands on **"phone"** instead of
**"language"**. It doesn't panic (both enums happen to have exactly 2 entries
today, so the index is never out of range) — it just points at the wrong
row. That's the quiet kind of bug the coordinator's carry-forward notes have
flagged before as the dangerous kind.

Fixed by moving the "preselect current language" intent to the moment a user
actually *enters* the language sublist (where "which one is current" is
still a meaningful question), and having `open_settings()` always select
`SettingsItem::Language` regardless of `app.lang`. Regression test:
`ui::tests::opening_settings_always_selects_language_regardless_of_current_lang`
— loops over every `Lang` and asserts the cursor always lands on `Language`.
Mutation-verified (see table below): reverting to the old lang-index logic
turns this test red.

### 2. `src/ui/pick.rs` — one construction site, mechanical

`View::Settings { state: ListState::default() }` appears in a test
(`seeding_never_yanks_the_user_out_of_a_screen_they_opened`) and needed the
new `lang: None` field. No behavioural change, compile-only fix.

### 3. The brief's own interface line has a structural gap it doesn't name

The brief says (Interfaces): `Produces: ... View::Settings { state: ListState }
（下标改为映射 SettingsItem::all()）` and (Step 3): "`Language` 进语言列表（把
今天的语言选择逻辑原样搬进去）" — "enter the language list, move today's
language-selection logic into it verbatim." Nothing in the brief says **how**
"the language list" is represented as a distinct navigable place with its own
Esc/Ctrl+Q behaviour, distinct from the top-level list.

I extended the existing `View::Settings` struct variant (not a new top-level
`View` variant — the plan's file-table only lists `View::Settings` restructure
+ `View::Phone` for the whole feature, and a new sibling variant would have
forced changes to several *other* exhaustive matches across the file that
have nothing to do with this task):

```rust
Settings {
    state: ListState,        // top-level cursor, indexes SettingsItem::all()
    lang: Option<ListState>, // Some = inside the language sublist, indexes Lang::all()
},
```

`lang: Some/None` is the single source of truth for which layer the user is
on. Both escape routes (`Esc`, handled locally in `settings_view::handle_key`,
and `Ctrl+Q`, handled globally via `view::back_one_level`) now agree: from
the sublist they drop back to the top-level list; from the top-level list
they still go to the board, unchanged. Every construction site of
`View::Settings` needed `lang: None` or `lang: Some(_)` added — `mod.rs`
(×2), `pick.rs` (×1), `view.rs`'s own width-fits-every-view test (×1),
`settings_view.rs` (multiple). This is why the file footprint grew past what
the brief listed; it's not optional scope creep, the struct literal doesn't
compile otherwise.

I did **not** touch `View::Phone` and did not add a third top-level `View`
variant of my own — see the placeholder section below.

## Placeholder left for Task 4

`src/ui/settings_view.rs`, in `handle_key`'s top-level `Enter` match, the
`Some(SettingsItem::Phone)` arm:

```rust
Some(SettingsItem::Phone) => {
    // TODO(task-4, dct-phone-channel): 手机通知页还没接线
    // （`View::Phone` 是 Task 4 的产物）。Task 3 是纯 UI
    // 重构、不掺任何手机通知逻辑，这一支特意留成空操作——
    // 光标停在原地，什么都不发生。Task 4 把这一支换成
    // `app.view = View::Phone { .. }` 就是它要接的线。
    app.view = View::Settings { state, lang: None };
}
```

"Phone" is a real, visible, selectable row in the top-level settings list
today (label from the new `Key::Phone` i18n entry, en: "phone", zh:
"手机通知"). Pressing Enter on it is a deliberate no-op — cursor stays on
the top-level list, nothing else happens, no crash. Task 4 replaces the body
of this arm with a dispatch to `View::Phone { status: ... }` once that type
exists. Covered by
`ui::settings_view::tests::entering_the_phone_item_does_not_crash_or_open_the_language_list`.

## Mutation table

| # | Mutation | Where | Test that should catch it | Result |
|---|---|---|---|---|
| 1 | `SettingsItem::at`'s `.get(i)` → `all()[i.min(1)]` (out-of-range clamps to last item) | `view.rs` | `an_out_of_range_index_selects_nothing` | **RED** — caught |
| 2 | Top-level `move_sel_n` length: `SettingsItem::all().len()` → `Lang::all().len()` | `settings_view.rs` | `pressing_down_from_language_selects_phone` (added per brief's explicit instruction for this case) | **GREEN — not caught**, and structurally *cannot* be caught by a black-box test today: both lengths are 2, so the mutation is behaviorally identical to the original. The brief itself anticipated this ("如果没有测试失败，补一条针对方向键能走到 Phone 的测试"); I added that test, it's real coverage for "does Down reach Phone at all," but it cannot discriminate *where* the length number came from while the two enums coincidentally have equal length. This is a genuine, disclosed limit, not a missed test. |
| 3 | Delete `back_one_level`'s `View::Settings { lang: Some(_), .. }` arm (Ctrl+Q from sublist falls through to the generic `_ => Board` case) | `view.rs` | `ctrl_q_from_the_language_list_goes_back_to_the_settings_list` (added; not requested by brief, added because I introduced this branch) | **RED** — caught |
| 4 | Delete `escape_hint`'s `View::Settings { lang: Some(_), .. }` arm (bottom-left hint falls through to "Ctrl+Q back to board") | `view.rs` | `escape_hint_from_the_language_list_says_back_to_settings_not_the_board` (added) | **RED** — caught |
| 5 | Revert `open_settings()` to the old `Lang::all()`-position preselect | `mod.rs` | `opening_settings_always_selects_language_regardless_of_current_lang` (added) | **RED** — caught |
| 6 | `Some(SettingsItem::Phone)` arm accidentally opens the language sublist (`lang: Some(ListState::default())`) instead of staying put | `settings_view.rs` | `entering_the_phone_item_does_not_crash_or_open_the_language_list` (added) | **RED** — caught |

Mutations 1–2 are the ones the brief named explicitly. Mutations 3–6 are
mutations I ran against my own additions (the nested-view design, the
`open_settings` fix, and the Phone placeholder) since those are exactly the
places the brief left underspecified and therefore least reviewed — all
caught, no test gaps left after the fact.

## Manual verification (Step 5, required, not optional)

Built `cargo build --release --bin dct`, ran it inside `tmux` against an
isolated `HOME` (`/tmp/dctverify-home` — had to be short: the scratchpad
path is long enough to blow macOS's `SUN_LEN` limit on the daemon's unix
socket path, which is a separate, unrelated discovery worth flagging for
whoever writes fixtures under the scratchpad directory). Never touched the
user's real `~/.dct` daemon (verified running throughout, untouched, before
and after).

Sequence and what was on screen at each step:

1. `l` from the board → top-level list titled "Settings" (locale in this
   fresh sandbox resolved to English), two rows: `language` (cursor here),
   `phone`.
2. `Enter` on `language` → title becomes "Settings · language", `English`
   preselected with `✓`, `中文` below it — the preselect-current-language
   behaviour survived the refactor.
3. `Down`, `Enter` → language switched immediately: board redrew fully in
   Chinese ("还没有会话，按 n 新建"), bottom bar showed "设置" as the
   confirmation message (same feedback string as before the refactor).
4. Checked `/tmp/dctverify-home/.dct/settings.json` on disk:
   `{"lang":"zh","view_mode":null}` — written before the process was even
   asked to quit.
5. `q` to quit the UI. Killed the leftover daemon process, relaunched `dct`
   fresh (new process, same `HOME`) → **board came up already in Chinese**
   ("还没有会话，按 n 新建", "q 退出", "n 新建", "x 移除") — **the language
   choice survived a full restart**, confirming `save_lang`/`resolve()`
   round-trip through disk correctly under the new structure.
6. Additional check beyond the brief's script: `l` → `Enter` on 语言 → `Esc`
   → back at the top-level "设置" list (not the board) with both rows
   visible, cursor still on 语言. Then `Down`, `Enter` on 手机通知 → no
   crash, stayed on the top-level list, cursor on 手机通知, bottom-left hint
   correctly read "Ctrl+Q 回看板" (top-level, unaffected by the sublist's
   different hint). This directly exercises the Esc/Ctrl+Q nesting behaviour
   this refactor introduces.

All tmux/daemon test processes were cleaned up afterward; the user's real
`dct` session (pid 6197/6198, running since Sunday) was confirmed
untouched before and after.

## Test commands and output tails

Failing-test checkpoint (Step 2, before implementation — `cannot find type
SettingsItem`), then green after implementation:

```
$ cargo test --lib ui::settings_view -- --test-threads=1
running 11 tests
test ui::settings_view::tests::an_out_of_range_index_selects_nothing ... ok
test ui::settings_view::tests::choosing_a_language_applies_it_and_writes_it_to_disk ... ok
test ui::settings_view::tests::entering_the_language_item_opens_the_list_preselected_on_the_current_language ... ok
test ui::settings_view::tests::entering_the_phone_item_does_not_crash_or_open_the_language_list ... ok
test ui::settings_view::tests::escaping_out_of_settings_changes_nothing ... ok
test ui::settings_view::tests::escaping_the_language_list_goes_back_to_the_settings_list_not_the_board ... ok
test ui::settings_view::tests::every_language_is_listed_in_its_own_language ... ok
test ui::settings_view::tests::phone_is_a_settings_item_too ... ok
test ui::settings_view::tests::pressing_down_from_language_selects_phone ... ok
test ui::settings_view::tests::the_first_item_is_language ... ok
test ui::settings_view::tests::the_top_level_list_shows_both_settings_items ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 724 filtered out
```

Full workspace suite (after all mutation reverts, final state):

```
$ cargo test -- --test-threads=1
test result: ok. 738 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 29.54s
[... 15 more integration-test binaries, each: 0 failed ...]
```

```
$ cargo fmt --check
(no output, exit 0)

$ cargo clippy --all-targets
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.72s
(no warnings)

$ git diff --check
(no output, exit 0)
```

`i18n::tests::every_key_is_listed_for_the_guards` has a hardcoded count
(`ALL_KEYS.len() == 100`); adding `Key::Phone` required bumping it to `101`
and adding `Phone` to the `ALL_KEYS` list — not mentioned in the brief, but
a five-minute mechanical consequence of adding any i18n key in this file,
caught immediately by the guard test itself.

## Concerns for whoever reviews this / writes Task 4

- Task 4's brief should point at `settings_view.rs`'s
  `Some(SettingsItem::Phone)` arm (see Placeholder section) and at the
  `Key::Phone` i18n entry I added (en: "phone", zh: "手机通知") if it wants
  to reuse that label for the phone page's own title — I picked generic
  wording since I don't know Task 4's exact page title yet.
- `Key::BackToSettings` ("Ctrl+Q 回设置"/"Ctrl+Q settings") is now used by
  two different destinations: `EnterSecret{return_to_settings: true}` (goes
  to `View::Secrets`, the API-key page) and `View::Settings{lang: Some(_)}`
  (goes back to the top-level settings list). Both are legitimately "back to
  a settings screen" and the text doesn't name which one, so I reused it
  rather than adding a near-duplicate key — flagging in case a future
  reviewer wonders why the same key serves two views.
- The `SUN_LEN` unix-socket-path-length crash is unrelated to this task's
  code but cost real debugging time during manual verification; worth
  remembering if any later task's fixtures or verification scripts spin up
  a real daemon under a long temp path (the repo's own scratchpad path is
  long enough to trigger it).

## Fix round 1 (commit `adacd36`)

Coordinator review: spec ✅, quality Approved, four items to fix. Also
corrected my own claim in this report's mutation table: the length
coincidence (`SettingsItem::all().len() == Lang::all().len() == 2`) does
**not** end when Task 4 lands — Task 4 adds `View::Phone`, a separate page,
not a third settings row. The blind spot outlives Task 4 and only closes
when a third language or a third settings entry shows up, whichever comes
first.

### 1. Important — `idle_help` was not level-aware (`escape_hint` was)

`src/ui/view.rs`: `View::Settings { .. } => help_items(...)` returned
`("Esc", Key::Cancel)` for both the top-level list and the language
sublist, so inside the sublist the bottom bar read left `Ctrl+Q 回设置` /
right `Esc 取消` — the two halves disagreed about where Esc goes, exactly
the failure mode this file's own `EnterSecret { return_to_settings: true }`
comment names: 「两处文案哪怕只有半句话不一致，都是「底栏说什么就得真能做到什么」
这条原则被破坏了一半」.

Added a `View::Settings { lang: Some(_), .. }` arm before the existing one,
using the already-existing `Key::BackToSettingsWord` string ("返回设置" /
"back to settings" — the same string `EnterSecret`'s equivalent arm uses),
placed and commented to mirror that precedent exactly. Test:
`ui::view::tests::language_list_idle_help_also_says_back_to_settings`,
mirroring `secret_view_from_settings_idle_help_also_says_back_to_settings`.
Mutation-verified: deleting the new arm turns the test red (`assertion
failed: 底栏说什么就得真能做到什么：↑↓ 选择  Enter 确认  Esc 取消`).

### 2. Minor — length-source tripwire

Added to `src/ui/settings_view.rs`'s test module, verbatim per the
coordinator's spec:

```rust
#[test]
fn the_length_coincidence_that_hides_a_wrong_move_sel_n_source() {
    assert_eq!(SettingsItem::all().len(), Lang::all().len(), "...");
}
```

This doesn't make mutation #2 catchable today — nothing can, given the
coincidence — but it turns "silently uncatchable forever" into "loudly
uncatchable until the day it isn't," which is the whole point: the day
someone adds a third `SettingsItem` or a third `Lang`, this test goes red
and *that* is the forcing function to finally write the "arrow keys reach
the last item" test the length parity currently makes pointless.

### 3. Minor — `Key::Phone` English text

`src/i18n.rs`: `en: "phone"` → `en: "phone notifications"`, matching what
`zh: "手机通知"` actually says (a non-programmer reading "phone" alone
has no idea what pressing Enter on that row does). No test changes needed
— existing i18n guards (`no_english_entry_contains_han_characters`,
`no_entry_is_empty_in_either_language`) already cover the new string.

### 4. Minor — `escape_hint_cols_fits_every_view` missing the sublist case

`src/ui/mod.rs`: added `View::Settings { state: ListState::default(), lang:
Some(ListState::default()) }` to the view list, mirroring how
`View::PickProject`'s two `typing_path` states are both already listed
there. No live risk today (the sublist's string happens to already be
measured via the `EnterSecret` case using the same `Key::BackToSettings`),
but it closes the gap for the moment the sublist gets its own distinct hint
string.

### Not addressed (by design — recorded by the coordinator as a merge gate, not mine to fix)

The phone row is a visible, selectable dead end while the bottom bar
advertises `Enter 确认`, technically violating this file's own 「屏幕上写着
做不到的操作比不写更糟」 rule. Correct for Task 3 in isolation; the
coordinator is tracking it so the branch cannot reach `main` before Task 4
gives the row something real to do.

### Re-verification after fix round 1

```
$ cargo test --lib ui::view -- --test-threads=1      # 95 passed
$ cargo test --lib ui::settings_view -- --test-threads=1  # 12 passed
$ cargo test --lib ui::tests:: -- --test-threads=1    # 64 passed
$ cargo test --lib i18n:: -- --test-threads=1         # 18 passed
$ cargo test --lib -- --test-threads=1
test result: ok. 740 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
$ cargo fmt --check   # exit 0
$ cargo clippy --all-targets   # no warnings
$ git diff --check    # clean
```

740 lib tests, up from 738 (the two new tests:
`language_list_idle_help_also_says_back_to_settings` and
`the_length_coincidence_that_hides_a_wrong_move_sel_n_source`).

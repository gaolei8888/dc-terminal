# Task 1 review: [[keep]] parsing (23318f1..4c8edca)

Spec verdict: PASS
Quality verdict: approved (minor findings only)

Ran in the worktree: `cargo test --lib keep` (86 ok), `config` (25 ok), `i18n` (21 ok).

## Spec checks
- One bad entry skips only itself: yes (src/keep.rs:257-296, `continue` per entry; test at :373).
- Duplicate names keep the first: yes (:287; test :402).
- `~` and `~/x` expanded against `home`: yes (:223-231; test :356).
- Missing file -> empty: yes (:302-307; test :456).
- Broken [[keep]] does not stop Config::load reading [llm]: yes, Config has no deny_unknown_fields, keep is a separate parse. Test src/config.rs:+32.
- Protocol bumped 26 -> 27 with doc entry; all 10 pinned shapes moved. server.mjs has no version pin (checked), so the report's open concern is closed.
- zh+en i18n present for both variants and all sub-reasons, added to the all-codes test (no newline, no Han in en). Wording is plain.
- `load` on unreadable or unparseable file: returns ([], []) silently, as the brief says (:302-307, :242-244; test :456 covers missing and unparseable).
- Names match what later tasks need: Keep, parse, load, WarningCode::KeptSessionProblem{name, reason}, KeptSessionReason{DirGone, ProfileGone, StartFailed, CrashLooping, NameClash} (Copy). No profile-name branching.
- No panics on odd TOML: `keep = 3`, `keep = [1,2]`, non-string fields, and blank fields all go through safe paths. The only `expect` (:286) is unreachable.

## Findings
1. src/keep.rs:429-436 (minor) `a_keep_of_the_wrong_type...` only checks `w.len()`. It should assert the index (0 for `keep = 3`, 1 and 2 for `[1, 2]`) and `NotATable`, so it pins the documented behaviour.
2. src/keep.rs tests (minor) Missing cases the brief and task focus call out:
   - a name with surrounding spaces is trimmed (`"  a  "` -> `"a"`);
   - a duplicate detected after trimming (`"a"` and `" a "`);
   - a non-string field (`name = 5`, `dir = 1`) -> `MissingField`.
   The code handles all three (text_field trims and rejects non-strings), but nothing pins them.
3. src/config.rs:+32 (minor) The new test would also pass before the change, because Config never read `keep`. It is a regression guard only, and its entry is not actually "missing fields" in the brief's sense (it lacks profile and dir, so it is). Optionally also add a `keep = 3` variant.
4. src/i18n.rs:+68 and src/keep.rs:253 (minor) When `keep` is not an array, the warning uses index 0, which renders "Skipped entry 0 of the always-on sessions" / "第 0 个常驻会话没能用上：格式不对". This is odd for a user. Consider a dedicated reason such as `KeepNotAList`, or wording that omits the number when index is 0. `NotATable` zh "格式不对" is also vague.
5. src/keep.rs:226 (minor) Only `~` and `~/` are expanded. A Windows user writing `~\work` gets a literal path. The user said most people use Windows, so consider accepting `~\` too. Not in the spec, so it is optional.
6. src/keep.rs:271-286 (nit) The `Vec` + `try_into().expect("三个字段")` dance is clumsy. Calling `text_field` three times with `let ... else` would be simpler and drop the expect.
7. Report (nit) The flaky session::tests failures are pre-existing, per the implementer's baseline run. I did not re-run the full suite.

No blocking findings.

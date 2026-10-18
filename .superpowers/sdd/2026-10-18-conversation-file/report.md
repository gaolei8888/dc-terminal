# Report: conversation id from the agent's own per-process file

Status: DONE_WITH_CONCERNS (minor, see end). Worktree .worktrees/conv-file, branch feat/conversation-id-from-agent-file.

## Commits
- 960b8d3 feat(conversation-id): read an agent's current conversation id from its per-process file
- 281be3e feat(profile): declare where claude-based profiles find their live conversation id
- 6a23d6e feat(restart): learn each session's live conversation id from the agent's file when saving
- 341622a docs(restart): /clear and /resume inside Claude are now followed

## What changed
- src/conversation_id.rs: `looks_like_an_id` (1..=128 chars, ASCII alnum or `-`), `file_path(template, pid, home)`
  (`~/` -> home, `{pid}` -> pid), `read_from_file(path, key, dir, pid)` (JSON object; key must be a valid id
  string; `cwd` if present must equal session dir after canonicalizing both; `pid` if present must match; any
  failure -> None, silent).
- src/profile.rs: `conversation_file` / `conversation_file_key` (`#[serde(default)]`, Chinese docs) and
  `learns_conversation_from_file()` = both set AND `resumes_by_id()`. Declared in claude, deepseek, glm, kimi,
  qwen-api. NOT in qwen and NOT in dc: dc.toml runs `qwen`, so excluded per "check each one's command".
  Struct literals in src/ and tests/ updated.
- src/session.rs: `Session::learn_conversation_from_file` called in `write_last_sessions` for every non-Stopped
  session (covers normal saves and the final restart save); only when pty alive; overwrites in-memory
  `conversation_id`. Birth `submitted` = `resumes_by_id && (conversation given || continuing)`.
- Read cost: saves only on create/stop/kill/prune/discard, Working->Idle in tick, restart saves; never per key.
  One small file read per qualifying live session per save.
- Failed --continue: dead process -> not read; Stopped -> not recorded. Test
  `a_failed_continue_learns_nothing_from_a_leftover_file`. A bad id that slipped in is handled by the existing
  `daemon::tests::a_session_whose_conversation_is_gone_is_restarted_fresh_and_reported` path.
- READMEs (en/zh): /clear limit replaced by "restart brings back the conversation you're in; dct reads Claude's
  per-window file; falls back to --continue if it can't".

## Tests
New: conversation_id parse accept/reject (good, no cwd/pid, symlinked cwd, wrong cwd, wrong pid, missing/empty/
non-string/bad-chars/too-long key, non-object, junk JSON, missing file, templating); profile declarations and
both-or-neither; session: --continue records learned id, file id change (/clear) replaces old id, other-dir file
ignored, by-id profile without file unchanged, failed --continue learns nothing.
`cargo test`: all green (lib 1962 passed, 1 ignored; all integration suites ok).
`cargo +1.99.0 clippy --all-targets -- -D warnings`: clean. fmt not run.

Mutation checks (all reverted):
- cwd check disabled -> `rejects_a_file_for_another_directory` and `an_agent_file_for_another_directory_is_ignored` FAIL.
- gate reverted to old -> `a_continued_session_records_the_id_learned_from_the_agent_file` FAIL.
- is_alive gate removed -> `a_failed_continue_learns_nothing_from_a_leftover_file` FAIL.

## Real /clear check
`tmux -L probe`, `env -u DCT_SESSION_ID -u CLAUDECODE claude` in /private/tmp/claude-502/cf, claude 2.1.302, no dct daemon.
- start: ~/.claude/sessions/80316.json `pid 80316, sessionId 80529fcc-eeda-4cf8-883a-a15f09226629, cwd /private/tmp/claude-502/cf`
- after one message: unchanged; transcript 80529fcc-...jsonl created
- `/clear`: sessionId -> 47735c4d-49f2-41a9-b99f-ff4485d3ed39 (same pid/cwd); 47735c4d-...jsonl (14 KB) created at once
- `/resume 80529fcc-...`: sessionId back to 80529fcc-..., same pid
- `/exit`: claude deleted sessions/80316.json
Conclusion: id follows /clear and /resume; README limit removed. Cleanup: probe server killed, temp project folder
~/.claude/projects/-private-tmp-claude-502-cf (my two transcripts + empty memory dir) and the temp dir deleted.

## Concerns
1. Accepting claude's "trust this folder" prompt left a trust entry for /private/tmp/claude-502/cf in claude's own config.
2. Not verified that `claude --resume` works on a /clear-ed conversation with no message after; fallback covers it.
3. Id refreshed only at saves; graceful restart's final save always reads it, but after a daemon crash a /clear with
   no turn afterwards may leave the old id.
4. Behaviour change: --continue sessions of by-id profiles now count as submitted, so drain_resumable may ask them
   for a handoff (when no id learned and an id-holding sibling exists). Matches reality.
5. Windows: unverified whether claude writes this file there; falls back silently.
6. dc excluded although listed in brief (runs qwen).

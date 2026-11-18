# Task 3 report
Status: DONE_WITH_CONCERNS. Commit b2359f4.

Manual test (qwen 0.24.6, fresh git tmp dirs, env OPENAI_BASE_URL=https://dc-llm.tzspace.cn/v1, OPENAI_MODEL=qwen3.8:27b, key from ~/.dct/secrets.toml [dc], not printed):
- `qwen --approval-mode=yolo --session-id $id -p "..."` then `--resume $id -p "..."` works in -p mode.
- Run 1: resume answered 蓝色长颈鹿 (but a fresh session also did: qwen's global auto-memory ~/.qwen/memories saved the phrase from run 1, so that control was polluted).
- Proof from session file: resume appended the new turn to the same session jsonl (same sessionId, history loaded; `qwen sessions list` shows no extra session).
- Run 3 (locker number QX2194021812): resume answered it remembers the QX-prefixed number (model refused to echo it); a fresh session answered UNKNOWN.
- `--resume <nonexistent uuid>` -> "No saved session found with ID ...", exit with error (so an unrecorded id fails loudly).
- --chat-recording is on by default (resume worked without it); dct's command needs no extra flag.
- Tested through the dc gateway, so both qwen and dc profiles are declared (same binary/env).

Changes: profiles/qwen.toml, profiles/dc.toml add session_id_args/resume_id_args; src/profile.rs: qwen, dc moved into claude_based_profiles_resume_by_id, removed from unverified list (comment updated). Left alone: claude_based_profiles_declare_continue_as_resume_args and unverified_profiles_declare_no_resume_args (resume_args --continue still undeclared for qwen/dc).
Tests: cargo test all green (1827 lib), clippy +1.99.0 -D warnings clean.

Concerns:
- Side effect: my tests wrote ~/.qwen/memories/user/secret-passphrase.md (+ MEMORY.md entry) with the phrase 蓝色长颈鹿; harmless but can be deleted. Also qwen global memory leaks across sessions, so "remembers X" tests need a fresh-session control.
- Only -p (headless) mode tested; interactive TUI resume by id not tested (same code path expected).
- Ripgrep EACCES warning in qwen install (unrelated).

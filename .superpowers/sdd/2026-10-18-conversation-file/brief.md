# Brief: learn each session's conversation id from the agent's own per-process file

Worktree: /Users/lei/work/dc/dc-terminal/.worktrees/conv-file (branch feat/conversation-id-from-agent-file). Work ONLY there.

## Problem
dct's graceful restart resumes each session by conversation id (`resume_id_args`, e.g. `claude --resume {id}`).
dct only knows the id when it started the conversation itself (`session_id_args`, `--session-id {uuid}`), see
`src/session.rs` around line 1394 (`conversation_id = match conversation { ... }`). Sessions that were brought
back with `resume_args` (`--continue`) get `conversation_id: None` forever, so the next restart uses
`--continue` again. That just failed for real: in a directory whose latest Claude conversation was a background
agent's, `claude --continue` refused and the session died. Also known limit "I3": after `/clear` or `/resume`
inside claude, dct still holds the old id.

Claude Code writes `~/.claude/sessions/<pid>.json` for every running interactive process, e.g.
`{"pid":97016,"sessionId":"ddc91785-...","cwd":"/Users/lei/work/dc/dc-terminal","kind":"interactive",...}`.
Verified on this machine (claude 2.1.302): the pid is the claude process dct spawned (child of the daemon,
`PtySession::process_id()`), and sessionId equals the conversation transcript id, also for a session started
with `--continue`. The file is undocumented/internal, so everything must degrade to today's behaviour.

## Requirements
1. Generic, no branching on profile names (repo rule). Add two optional profile fields in `src/profile.rs`
   next to `session_id_args` / `resume_id_args`, with `#[serde(default)]` and doc comments in the file's
   Chinese style, e.g. `conversation_file` (path template, `~` = home, `{pid}` = agent pid) and
   `conversation_file_key` (JSON key holding the id). Pick names that fit; keep both-or-neither semantics
   like `resumes_by_id`. Declare them in every profile toml that runs the `claude` binary and already has
   `resume_id_args` (claude, dc, deepseek, glm, kimi, qwen-api — check each one's `command`; qwen runs a
   different binary: do NOT add it there). Only for profiles that resume by id.
2. When dct writes `last-sessions.toml` (the snapshot near `src/session.rs:1085-1110`), for each session whose
   profile declares the file and whose process is alive: read the file, parse JSON, accept the value only if
   - the key holds a non-empty string that looks like an id (reuse whatever validation exists; at minimum
     ASCII alnum and `-`, sane length),
   - the file's `cwd` (if present) equals the session dir (compare canonicalized paths; if the file has no
     `cwd`, accept),
   - the file's `pid` (if present) equals the session's pid.
   On accept, store it into the session's in-memory `conversation_id` (so later saves and
   `conversation_id()` at line ~1236 see it too). On any failure keep the existing value. Never panic, never
   print noise per save (at most nothing).
3. Recording gate: today an id is only written if `s.submitted`. A session started with `resume_args`
   (`--continue`) continued an existing conversation, so it must count as resumable too: treat it like
   `submitted = true` from birth (the `resumed_by_id` precedent), but ONLY when the profile also resumes by id
   (otherwise there is nothing to record anyway). Think about whether a failed `--continue` (process exits at
   once) can leave a bad record; the existing restore code detects "resume failed, exited by itself" — make
   sure a bad id still falls back to opening a fresh session, and add/point to a test for that.
4. Read cost: this runs per save; keep it to one small file read per qualifying session. If saves happen on a
   hot path (check how often `save` runs), make sure it's fine; a cheap approach is fine.
5. Tests (TDD, in the existing test modules): parsing accept/reject cases (good file, wrong cwd, wrong pid,
   missing key, junk JSON, missing file), the --continue session now records the id learned from the file,
   an id change in the file (simulating `/clear`) replaces the old id on the next save, profile both-or-neither.
   Use a temp dir for the file path (make the path template injectable or point HOME-free path at a temp dir;
   don't touch the real ~/.claude). Mutation-check: break the cwd check and confirm a test fails, revert.
6. Real check of `/clear` (needed to decide the docs): in an ISOLATED setup only — a private
   `tmux -L probe` server, `env -u DCT_SESSION_ID`, a short temp dir like /private/tmp/claude-502/cf, run the
   real `claude` there (its real ~/.claude is fine to read, do not edit other files), send one short message,
   note sessionId in ~/.claude/sessions/<pid>.json, run `/clear`, note it again, then `/exit`. Report whether
   the id changed. Do NOT start, stop or talk to the user's real dct daemon (~/.dct/daemon.sock); do NOT touch
   ~/.dct or ~/.local/bin. Do not run a dct daemon at all unless it's with HOME set to a short temp dir.
   Kill the probe tmux server afterwards and delete the temp claude conversation files you created only if
   they are clearly yours (in the temp-dir project folder).
7. Docs: READMEs (README.md and README.zh-CN.md) list the `/clear` known limit for restart — update it to match
   what step 6 found (if the id follows `/clear` and `/resume`, remove the limit; mention that dct reads Claude's
   own per-window file and falls back to `--continue` when it can't). Keep wording plain.

## Constraints
- Run `cargo +1.99.0 clippy --all-targets -- -D warnings` and `cargo test` — all green.
- Do NOT run `cargo fmt` (repo isn't fmt-clean; it reformats ~74 unrelated files).
- Commit messages in English, NO Co-Authored-By or any AI attribution line. Small focused commits.
- Never dispatch subagents.
- Write your full report to /Users/lei/work/dc/dc-terminal/.superpowers/sdd/2026-10-18-conversation-file/report.md
  (what changed, test output summary, the `/clear` finding with evidence, mutation check result, concerns).
  Return only: status, commit list, one-line test summary, concerns.

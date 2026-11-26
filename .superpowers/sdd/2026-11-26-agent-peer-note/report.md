# Report: agent peer note (feat/agent-peer-note)

Commits: b68ce7a (feature), b633e53 (docs).

- `note_args` profile field + `fill_note`; claude/kimi/glm/deepseek/qwen-api use
  `--append-system-prompt {note}`, codex uses `-c developer_instructions={note}`.
- Note appended at spawn only when the mesh address name is known
  (`Mesh::address_name`, same lookup as `dct send`) and args reach the program
  intact (Windows .cmd/.bat shims get no note: cmd.exe would cut the multi-line text).
- Daemon: mesh setup moved before the restore thread; `refresh_peer_note` runs
  once there and on every delivery tick (name read under mesh lock, set after release).
- Mutation check: forcing the name check to pass made
  `spawn_passes_no_note_when_not_logged_in` fail; restored.
- Controller verified: clippy 1.99 clean; `cargo test note` 28 passed.

Open: Windows gets no note yet; a session keeps the name it started with;
codex flag tested once by hand on 0.172.0.

# Task 1 report

Status: DONE. Commit: see `git log -1` on feat/match3-play ("feat(game): match-3 move picking: legal swaps, cascade simulation, scoring"), no Co-Authored-By.

Did: wired dct-game into workspace (members + dependency line), created the crate with the brief's code verbatim.
Only deviation: in tests.rs the two `use` lines are in the order `use crate::sim::try_move;` then `use crate::*;` (swapped; no semantic change). Clippy needed no changes.

Test: `cargo test -p dct-game` -> 15 passed, 0 failed, no warnings.

Mutations (all restored; diff -r against saved copy identical):
1. sim.rs `pass == 0` -> `99` (cleared): 7 tests red (horizontal_three..., four_in_a_row..., five_in_a_row..., l_shape..., gravity_cascade..., triggered..., equal_scores...).
2. delete `out.wrapped += 1;`: l_shape_makes_a_wrapped_candy red.
3. `>= 5` -> `>= 6`: five_in_a_row_makes_a_bomb red.
4. cascade weight 0.5 -> 0.0: a_chain_reaction_outscores_the_same_clear_without_one red.
5. `count == 1` -> `count == 99`: single_cell_classes_become_unknown red.

Clippy: `cargo clippy --workspace --all-targets --locked -- -D warnings` clean.

Files changed: Cargo.toml, Cargo.lock, crates/dct-game/{Cargo.toml,src/lib.rs,board.rs,sim.rs,choose.rs,tests.rs}.

Concerns: `.superpowers/sdd/.gitignore` shows modified in the worktree (rewritten to `*` by the sdd-workspace script; not by me, not committed). Per its own comment it should be restored.

# Task D13 report
Implemented in src/learn.rs: Card/render_card (plain-Chinese steps, popup rules, learned count, one-off + notes + popup_failures warnings, JSON), Verdict, apply_confirmation (Yes: approve_proc first, then popups via show->body_sha256->approve, then confirm_learned accept; failure at procedure returns Err, nothing else happens. No: confirm_learned reject only).
RED: `cargo test --lib learn::` -> 8 compile errors (render_card, Verdict, apply_confirmation not found).
GREEN: 12 passed in learn::.
Full: cargo test --workspace -- --test-threads=1: 2241 passed, 0 failed, 5 ignored. clippy: 0 warnings. rustfmt --check src/learn.rs ok. Windows check: only pre-existing unused `path` warning (first attempt hit transient dct-explore compile errors from concurrent agents; retry clean).
Files: src/learn.rs only. Cargo wiring fine (FakeHands reachable).
Concerns: learned buttons shown as a count only (signature gives keys, no labels); "No" skips confirm_learned when learned is empty; popup approval is show+approve per popup (not atomic across popups).

## Fix round 1
Changes: Written gains `final_body` (set by write_drafts to the body dcv accepted); render_card builds steps from w.final_body and header name from w.procedure. plain_step: text param only if name in {text,title,description,caption} or ends with `_text` (so `context` is not text). New tests: the_card_shows_the_body_the_fixer_produced, the_card_header_shows_the_renamed_procedure, notes_and_popup_failures_appear_on_the_card, only_real_text_params_become_the_text_to_post (miss case: context). Two older card tests now set w.final_body instead of d.body.
RED: `cargo test --lib learn::` -> E0560 `Written` has no field `final_body`.
GREEN: learn:: 16 passed. Full workspace: 2265 passed, 0 failed, 5 ignored; clippy 0 warnings; rustfmt ok; Windows check only the pre-existing unused `path` warning.

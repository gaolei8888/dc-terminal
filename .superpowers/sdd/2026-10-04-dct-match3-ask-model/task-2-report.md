# Task 2 report
Implemented per brief in play.rs, navigate.rs, cli.rs, play_tests.rs.
- Options/NavOptions gained a lifetime and advisor/ask_always/ask_budget/goal; all construction sites default to None/false/0/"". ASK_SHOWN=8.
- Ask block placed after `let Some(pick)`; offered computed in a scoped block (no borrow issue). Trigger rule as in brief (pick.is_some() check dropped since pick is already usize there).
- Records: `decider` on every candidate record, `ask` only when asked.
- Extra (coordinator): show_status("think") before adv.pick, show_status("look") after (also when advisor returns None); only when actually asked. Test the_octopus_thinks_only_while_the_model_is_asked: events ["think","look","swipe"] vs ["swipe"] (and Down advisor case).
- Placeholder test replaced with a real one using board ONE (3x4, found by brute force: exactly one candidate): calls==0, decider rules.
- Brief's tests were used verbatim, including the ask_budget one (passed as-is).
- Added test a_choice_outside_the_offered_list... because mutation 3 (remove the k<len filter) survived the brief's tests; now killed.
- Mutations: offered>=1 (killed by single-candidate test), budget->true (killed by ask_budget test), filter removed (killed by new test), think removed (killed by octopus test).
- RED: compile failure was inherent before the fields existed (tests written against the new API); I did not separately capture compile output.
- Workspace: 1674+ tests green; clippy -D warnings clean. rustfmt not installed for 1.99.0, not run.

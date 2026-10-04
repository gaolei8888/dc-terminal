# Task 1 report

Commit de7af37. Files: crates/dct-brain/src/ticket.rs (Region, Waiver, Ticket.waiver, canonical_bytes append, 4 new tests + base_ticket/waiver helpers + `Tweak` type alias for clippy type_complexity).
Ticket literal sites given `waiver: None` (compiler confirmed no others): ticket.rs procedure_ticket(), ticket.rs action_ticket_digest_is_stable, approval.rs ticket_for, sign.rs ticket(), src/procedures.rs (~line 346). No existing assertion changed.

RED/GREEN: honest note: I wrote the implementation before the tests (did not observe the compile-failure RED). Instead verified tests bite via mutations (all restored):
- M1 always append a "none" marker when waiver is None: 4 fail (new bytes test, a_waiver_is_appended, and the existing procedure/action digest vector tests).
- M2 drop region.h_bp append: changing_any_waiver_field_changes_the_digest fails.
- M3 drop skip_serializing_if: json_has_no_waiver_key... fails.
GREEN: dct-brain 59 passed; workspace --locked all ok (1707 + others, no flaky failures); clippy -D warnings clean; git diff --check clean. rustfmt not installed for 1.99.0, fmt not run.
Concerns: none; existing digest vectors still pass, so no-waiver bytes unchanged.

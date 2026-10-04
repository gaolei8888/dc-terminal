# Task 2 report
Files: crates/dct-brain/src/sign.rs (role_may_sign(tier, role, waiver), waiver_is_sane, VerifyError::BadWaiver + Display, 7 new tests).
RED: new tests failed to compile (no BadWaiver). GREEN: dct-brain 66 pass; workspace 1707+ pass, 0 failed; clippy -D warnings clean.
Order in verify: version/device/time/batch/signature unchanged; then waiver sanity (BadWaiver regardless of signer role; tampered waiver reports BadSignature first); then role_may_sign. Non-waiver tickets: identical path.
Signing side: role_may_sign is only called from verify; sign_one/sign_batch do no role check, nothing to change.
Error for Money/Critical/no-waiver Content by Auto is the existing WrongRole{need:User}.
Mutations: removing region check -> region + kind tests red. Removing `tier == Content` alone or `rule_can_waive` alone, or `kind` check in role_may_sign alone: NO test red, because the guards overlap (rule_can_waive is true only below Critical, so with required_signer User only Content passes; waiver_is_sane already rejects bad kinds before role_may_sign). Removing both overlapping guards -> Money/Critical test red; removing kind in both places -> kind test red. Kept as defense in depth.

## Fix round 1 (critical: subject not checked)
Fix: role_may_sign(&Ticket, role) now requires waiver_applies(t): subject is Subject::Action with tool == "tap_at" AND waiver.kind == "no_text". Procedure and any other tool get no exception. BadWaiver sanity check unchanged.
RED (before fix): the_exception_covers_only_the_tap_at_action and in_a_batch_the_exception_is_judged_ticket_by_ticket failed.
New tests: exception covers only tap_at (Procedure/tap Auto -> WrongRole; tap_at Auto ok; all three User ok); tap_at with kind "other" refused via role_may_sign directly and BadWaiver via verify; batch (tap_at ok, Procedure rejected). Existing waiver tests moved to tap_at tickets.
Mutations: subject check -> `true`: Procedure/batch tests red; tool == "tap_at" -> any non-empty tool: "tap" test red.
Results: dct-brain 69 pass; workspace green (zombie_reaping flaked once, passed alone and on full rerun); clippy -D warnings clean.
Spec (dc-terminal repo, branch feat/dco-settle-and-moods, commit 375d630): section 2 updated; dco must also reject waivers on tools other than tap_at.

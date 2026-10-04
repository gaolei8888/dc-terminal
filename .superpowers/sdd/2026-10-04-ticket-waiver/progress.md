# SDD ledger — plan: docs/superpowers/plans/2026-10-04-ticket-waiver.md
Worktree: /Users/lei/work/dc/dc-terminal-waiver, branch feat/ticket-waiver (from main 3b9f440). Spec: /Users/lei/work/dc/dc-terminal/docs/superpowers/specs/2026-10-04-ticket-waiver-design.md (dco-reviewed).

## Preflight scan
| Pair / task | Produces vs consumes | Finding |
|---|---|---|
| T1 -> T2 | Waiver/Region/Ticket.waiver consumed by verify | match |
| T1 -> T3 | Waiver/Region consumed by waiver_for_tap | match |
| T1 own | every `Ticket {` literal across crates/src needs `waiver: None` | compiler lists them |
Ruling: git commit messages in English with no Co-Authored-By (user standing rule).
Ruling: security-sensitive change (ticket format / verify exception) — per-task reviews kept for T1 and T2 on a capable model; T3 single reviewer; final whole-branch review mandatory before merge. User chose "rule waives, auto key signs" (option 1) on 2026-10-04.
Task 1: minor (deferred): no deny_unknown_fields on Waiver/Region (unknown keys ignored on read; signature fails closed — acceptable; never add it to Ticket); region plausibility is Task 2.
Task 1: complete (commits 6fbacda..de7af37, review clean)
Task 2: implemented (a1b8d11); review dispatched on opus (security-critical). Reviewer asked to check Subject kind restriction (Action only?), order of checks, batch path.
Task 2: security review (opus) found CRITICAL: exception did not restrict Subject; fixed in 92cb7c0 (waiver_applies requires Subject::Action tool "tap_at" + kind "no_text"; red tests, mutation-verified).
Ruling: the user chose policy B on 2026-10-04 (only privacy and money are gated; tap_at needs no ticket). The waiver half of dct-brain therefore has no consumer: branch feat/ticket-waiver is PARKED, not merged to main (unused permission-related code is attack surface with no benefit). Task 3 (waiver_for_tap) not done. Cost if wrong: re-dispatching Task 3 and a final review later; the spec and branch are kept.

# Final whole-branch review: public live listing

Worktree: /Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live, branch feat/public-live-listing.
Range: 8e69a54..HEAD (29 files, ~3.8k lines). Spec: docs/superpowers/specs/2026-09-13-public-live-listing-design.md. Plan: docs/superpowers/plans/2026-09-13-public-live-listing.md (a draft — code that deviates for good reason is fine; the ledger records rulings).
Ledger: .superpowers/sdd/2026-09-13-public-live-listing/progress.md. Deferred minors from per-task reviews: final-review-deferred.txt (same dir) — decide for each: fix now / leave (say why).

Each task was already reviewed in isolation. Your job is what per-task reviews cannot see:
1. Cross-component contract: dct-link constants/grant ↔ dct-srv routes & auth ordering ↔ daemon (src/live.rs, src/daemon.rs, src/proto.rs) ↔ TUI (src/ui/*) ↔ classroom server.mjs/admin.html ↔ public.html/live.html. Field names, status codes, header names, empty-token handling, title length limits (60 chars — enforced where?), reserved id "public".
2. Security: can anyone without a publish key make a room public, list non-public rooms, read a private room tokenless, or use a grant for anything but PUT/DELETE /live/{id}/public? Can a grant for room A act on room B? Is the key ever sent to the student container, logged, drawn, or put in Debug output? XSS in public.html / live.html / admin.html (titles are attacker-controlled free text).
3. Lifecycle: relay restart, key revoke, takedown, room end, daemon restart, re-publish after stop — does public state converge and never leak a stale public listing? PROTOCOL_VERSION 19 bump consistency (pins in tests/docs).
4. Docs match behavior (deploy-live-relay.md, READMEs).

Operating constraints: you must not stall. Read at most ~100 lines per tool call (use grep -n to find, then sed -n ranges). Do NOT run `cargo test --workspace` or workspace clippy (the controller ran them: takes ~25 min). You may run focused tests (e.g. `cargo test -p dct-srv live`, `node --test container/classroom/server.test.mjs`) in bounded calls. Read-only: do not modify files, index, HEAD or branches.

Output (final message): 
### Critical / Important / Minor — each with file:line evidence and a concrete failure scenario
### Deferred minors — one line each: fix-now or leave + reason
### Verdict: Ready to merge | Needs fixes
Calibration: Important = a real user- or security-visible defect or a broken contract, not style.

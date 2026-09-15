# Task 11 brief: documentation for the public live listing

Worktree: /Users/lei/work/dc/dc-terminal/.claude/worktrees/public-live (branch feat/public-live-listing). BASE = the HEAD you find at start (record it).
Plan text: docs/superpowers/plans/2026-09-13-public-live-listing.md lines 2920-3010 (read ONLY that window). The plan's prose is a draft, not authoritative — every command, flag, key, and on-screen string you document must be checked against the code (grep it; do not read whole files).

Operating constraints: reads ≤60 lines at a time; no cargo builds or tests are needed (docs only). Commit message in English, NO Co-Authored-By or any AI attribution lines. Never `git stash`.

## Verified facts (controller checked these)
- CLI (crates/dct-srv/src/lib.rs ~824-866): `dct-srv key add <name> --file <f>`, `dct-srv key revoke <name> --file <f>`, `dct-srv key list --file <f>`, `dct-srv takedown <id> --file <f>`, serve: `dct-srv <addr> [--with-link] [--publish-keys <f>]`.
- Relay routes: public page `GET /` (dct_page::public_page), list `GET /live/public` (already under `/live/*`), publish `PUT/DELETE /live/{id}/public`. The phone page moved to `/phone` and exists only with `--with-link`.
- TUI keys: `p` publish/unpublish, `K` change key (i18n LiveChangeKey). Banner strings: grep `live_on_air_public` in src/i18n.rs (en "● LIVE PUBLICLY · {title} · {routes} lane(s) · {viewers} watching", zh "● 正在公开直播 · {title} · {routes} 路 · {viewers} 人在看"). Quote them the way the README quotes the existing private banner (README.md:680, README.zh-CN.md:569 — match that style/sample numbers).
- Classroom: env `CLASSROOM_LIVE_PUBLISH_KEY_FILE`; heal every 30s (grep server.mjs to confirm).

## Edits
1. docs/deploy-live-relay.md
   - The doc ALREADY has sections 一、二、三 (三 is `LimitNOFILE`). Add the publish-keys section as **「四、公开直播：发布密钥」** after section 三 (before 「边界值一览」), content per plan Step 1.3 (fix the numbering; drop the zero-width chars).
   - Caddy block: add exact-match `handle / { reverse_proxy 127.0.0.1:8787 }` after `handle /live/*` with the plan's comment. Update the numbered notes below the block (line ~91 says `/live/*` is the ONLY reverse_proxy — now there are exactly two, `/live/*` and exact `/`). Update section 一 title and the 「线上现状」 table row (line ~51) consistently.
   - Verification block (~lines 103-117): root `/` must now be 200 (public page); add `/phone` must be 404; `/link/*` still 404.
   - Mention rollback (remove --publish-keys, restart).
2. container/classroom/README.md: paragraph per plan Step 2 after the `CLASSROOM_LIVE_RELAY` paragraph (~line 32). Also mention that `CLASSROOM_LIVE_RELAY` must be set for public lives to work.
3. README.zh-CN.md live section: bullet per plan Step 3 after the 「屏幕上一直写着你在播」 bullet (569). README.md: English bullet after line ~680 bullet. Also README.md ~851 (and the zh equivalent — grep `crates/dct-srv/`) describes the relay as "Only /live/* by default": update to mention the public page at `/` and `--publish-keys`.
4. docs/superpowers/specs/2026-09-13-public-live-listing-design.md: replace the first bullet under 「### 设置页与密钥存放」 per plan Step 4.
5. Commit: `git add docs container/classroom/README.md README.md README.zh-CN.md && git commit -m "docs: deploy, operate and use the public live listing"` (check `git status` first: only these files).

## Report
Write .superpowers/sdd/2026-09-13-public-live-listing/task-11-report.md (≤40 lines): commit SHA, each edit with file:line, every place the plan draft disagreed with the code and what you wrote instead. Final message: status DONE/BLOCKED + SHA, ≤10 lines.

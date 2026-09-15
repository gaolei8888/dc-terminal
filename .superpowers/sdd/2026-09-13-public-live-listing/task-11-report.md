# Task 11 report: documentation for the public live listing

BASE = ba9ffab93f2a05f619d240ad0f9fe6bd2f89a343
Commit = 9abd9d6

## Edits
1. docs/deploy-live-relay.md
   - Added new section "四、公开直播：发布密钥" (line 189) before 边界值一览, content per plan Step 1.3 (zero-width chars dropped, numbering fixed to 四 since sections 一/二/三 already existed).
   - Caddy block (line ~91-93): added exact-match `handle / { reverse_proxy 127.0.0.1:8787 }` after `handle /live/*`, before the 404 fallback; updated notes 1-2 below the block.
   - Verification block (line ~110-129): `/` now checked as 200; added `/phone` must be 404; `/link/*` unchanged 404.
   - Section 一 title (line 57) and 线上现状 table rows (line 52-53) updated to mention `/` alongside `/live/*`.
   - Also updated the "为什么非要一条路径白名单" section (line 28-32): the old text claimed `dct-srv` mounts neither `/link/*` nor `/` by default; code shows `/` (public page) and `GET /live/public` are always mounted regardless of `--with-link`, only `/link/*` and `/phone` are gated. Fixed this beyond the brief's explicit edit list since it directly contradicted the new section 四 in the same file — flagged here as a plan-vs-code disagreement I resolved.
2. container/classroom/README.md (after CLASSROOM_LIVE_RELAY paragraph, line ~32): added CLASSROOM_LIVE_PUBLISH_KEY_FILE paragraph per plan Step 2, plus one added sentence noting CLASSROOM_LIVE_RELAY is a prerequisite (per brief item 3).
3. README.zh-CN.md (line 571, after 屏幕上一直写着你在播 bullet) and README.md (line 682, after The screen keeps saying you are live bullet): added the publish bullet per plan Step 3. Also updated the "只许放行 /live/*" sentences at README.zh-CN.md:579 and README.md:692 to mention `/` and the publish-keys setup.
4. docs/superpowers/specs/2026-09-13-public-live-listing-design.md (line 128-130): replaced first bullet under 「### 设置页与密钥存放」 per plan Step 4.

## Verification against code
All CLI syntax, route paths, i18n strings (`live_on_air_public`), env var name, 30s heal interval, `__live_publish__` secret key, `LivePublishKeyMissing`, and the `K`/`p` bindings were grepped and matched the plan draft exactly — no other disagreements found besides the one noted above.

git status before commit showed only the 5 intended files changed.

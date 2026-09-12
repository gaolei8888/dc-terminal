# Student Works Publishing Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans for the host publisher; independent website, skill, and UI tasks run in parallel under dispatching-parallel-agents.

**Goal:** Claude and Codex prepare working static previews and students submit immutable versions to ai.tzspace.cn.

**Architecture:** A scoped workspace credential permits prepare/status only. The trusted DCT browser requires its existing session for submission. A separate HTTP listener serves validated static files on works.dataclue.cn, with expiring read-capability preview links. ai-mania owns moderation and the currently approved version; the host checks its authenticated status before serving public releases.

**Tech Stack:** Node HTTP/filesystem, Python descriptor-based file collection, Next.js/Prisma, shared Markdown skills.

**Spec:** ../specs/2026-09-11-student-publishing-design.md

## Global Constraints

- Static built output only; no server-side build execution or arbitrary proxy destinations.
- Host-only cookies; student content never runs on the DCT or ai-mania application origin.
- 5000 files, 100 MiB decoded per version, 145 MiB request limit, 1 concurrent preparation.
- Workspace credential cannot submit, moderate, read other workspaces, or administer the website.
- Existing files, agent sessions, manual projects, and website records remain intact.
- Public visibility follows ai-mania approval; updates do not replace approved content prematurely.

## Task 1: Host artifact publisher

Files: `container/classroom/publishing.mjs`, `publishing.test.mjs`, `collect-static.py`, `test_collect_static.py`; integration in `server.mjs`.

- [x] Write HTTP tests for authenticated prepare/status/submit, identity isolation, invalid paths/base64/types/limits, idempotent submission, approval gating, preview expiry and response headers; run red.
- [x] Implement `Publishing(app, {origin, siteOrigin, serviceKey, dir})`, `agent(req,res,url)`, `browser(req,res,w,target,prefix,session)`, `provision(w)`, and independent `server` static listener.
- [x] Validate all files before atomic rename of a private staging directory. Store immutable metadata and workspace binding separately from student-controlled volumes.
- [x] Read browser-selected output via Python openat/O_NOFOLLOW for every component, fstat regular/nlink checks, bounded reads and before/after metadata validation.
- [x] Verify public release access with GET ai-mania status; fail closed on outage, cache at most 15 seconds. Preview access uses a random 256-bit read token with 7-day expiry, no listing or cookies.
- [x] Run publisher HTTP tests and existing manager regression tests.

## Task 2: Website moderation

Files: ai-mania `prisma/schema.prisma`, additive migration, `app/api/dct/projects/route.ts`, publishing service/tests, existing admin project page/actions/component.

- [x] Service-authenticated POST accepts `{workspaceId,projectKey,version,title,owner,summary,previewUrl,assetUrl,files,bytes}` with strict types, origin/path validation and bounded JSON.
- [x] Add unique workspace/project binding and immutable version payload; conflicting version reuse fails 409. New versions stay pending.
- [x] GET with workspaceId/projectKey returns `{projectId,projectUrl,published,currentVersion,versions:[{version,status}]}`.
- [x] Add authenticated preview/approve/reject/rollback/unpublish controls and transactional gallery updates; guard legacy toggles from bypassing version review.
- [x] Verify isolated database idempotency, moderation, rollback and existing manual projects; typecheck/build.

## Task 3: Shared agent skill

Files: `container/skills/ai-pub/SKILL.md`, `container/bin/dct-publish`, entrypoint/Dockerfile hooks and CLI tests.

- [x] CLI prepare collects a static directory safely, derives projectKey=SHA256(namespace-relative path), reads only its scoped credential, calls host prepare and prints returned links.
- [x] CLI status checks an existing version; ambiguous prepare failure is not silently retried.
- [x] Install identical skill under Claude `~/.claude/skills` and Codex `~/.agents/skills`; validate frontmatter and behavioral CLI tests.
- [x] Explain relative asset bases, static-only limits, browser-openable URLs and explicit UI submission.

## Task 4: Student UI

Files: `container/upload-panel.html`, `container/tests/publishing-ui.cjs`.

- [x] Add simple 发布作品 panel with output folder/title, prepare preview, open preview and 确认提交.
- [x] `?publish=<version>` loads a prepared version; never auto-submit. Use current workspace prefix and existing themes.
- [x] Verify browser prepare/submit/error/deep-link/XSS behavior.

## Task 5: Deployment and actual use

- [x] Verify DNS works.dataclue.cn; provision HTTPS only when resolved to this host.
- [x] Back up website DB with SQLite backup API and application/config, apply additive migration, deploy website and verify service authentication.
- [x] Deploy host publisher on loopback-only static port, restrict Caddy by hostname, install matching server-only publishing credential and scoped workspace credentials.
- [x] Install skill/CLI in active containers without restarting their agents and include install in future image/startup.
- [x] Verify both skill copies, CLI actual preview, authenticated student confirmation, website moderation/version rollback and static assets with browser tests. Remove only synthetic test records/artifacts.
- [x] Prepare the user's existing cafe output if available; return its real preview/confirmation URL without submitting public content on their behalf.

## Delivery verification

Deployed 2026-09-11. DNS works.dataclue.cn resolves to64.83.14.82; HTTPS verified. Both skills installed identically in the active shared workspace; new image includes startup hooks. Cafe preview/confirmation verified without submitting user content. Live synthetic student/admin flow passed approval, pending update preservation, rollback and unpublish; synthetic website project and workspace input files removed. Tiny internal test version artifacts remain unlisted for audit and can be pruned using PUBLISHING.md. Existing agent container start time unchanged.

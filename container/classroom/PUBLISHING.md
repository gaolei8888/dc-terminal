# Student publishing

Claude and Codex use the same `ai-pub` skill and `dct-publish` command.
The command prepares static output and returns an actual preview plus a student
confirmation link. Only the authenticated student page submits to ai-mania.
ai-mania administrators approve/reject versions, roll back, or unpublish.

## Runtime configuration

`CLASSROOM_PUBLISH_CONFIG` points to a root-readable JSON file:

```json
{"origin":"https://works.dataclue.cn","siteOrigin":"https://ai.tzspace.cn","serviceKey":"SERVER_ONLY_VALUE"}
```

The website needs the matching `DCT_PUBLISH_SERVICE_KEY` and `DCT_WORKS_ORIGIN`.
Never place this key in the workspace. Each workspace instead receives a separate
prepare/status-only credential in `~/.dct/publish.json`; disabling or resetting a
workspace invalidates that credential. A normal workspace page visit provisions it.

The manager listens on loopback 17700; the independent static listener uses
`CLASSROOM_WORKS_PORT` (default 17701). Caddy routes **only** works.dataclue.cn to
17701. The works origin must differ from both authenticated application origins.
Production uses `MemoryMax=1536M`: a maximum 100 MiB preparation was measured at
518 MiB RSS before accounting for other manager activity. The older 256 MiB unit
limit is insufficient when enabling this feature.

Files are limited to 5000 and 100 MiB per version; manifests and workspace version
counts have separate quotas. Each workspace may keep at most 30 versions and
512 MiB of artifact contents. Global artifact limit is 4 GiB. Upload bodies spool
to private disk, at most two concurrently and one per workspace; preparing a
version is serialized. The upload deadline is 120 seconds.

Preview links grant read access to anyone holding the URL for seven days. They
are unlisted capabilities, not account-only pages. Public release URLs require
the website's current approved version and published flag; permission results
are cached for at most 15 seconds, and website outages fail closed. Stopping a
student container does not remove artifacts. HTML uses a separate origin, CSP
sandbox, no cookies, no referrer, and no connection access to application origins.

## Installation

The image includes Python3, `/usr/local/bin/dct-publish`, and the canonical skill
under `/usr/local/share/dct-skills/ai-pub`. The entrypoint links it into
both `~/.claude/skills` and `~/.agents/skills`. Existing containers need the runtime
and skill installed before enabling provisioning; do not restart active agents
just to install these files. New sessions discover the skill automatically;
existing agents can explicitly read its SKILL.md or run `dct-publish`.

The host needs `container/bin/dct-publish` installed beside the classroom directory
at `../bin/dct-publish`; the descriptor-based collector imports this implementation.

## Verification and recovery

```sh
node --test container/classroom/publishing.test.mjs container/classroom/server.test.mjs container/classroom/sso.test.mjs
python3 -m unittest discover -s container/classroom -p test_collect_static.py
python3 -m unittest discover -s container/skills/ai-pub/tests
node container/tests/publishing-ui.cjs
```

2026-09-11: real browser verification passed student prepare/confirm, administrator
approval, pending-version preservation, public assets, rollback and unpublish.
The cafe's three images, WebGL model and authenticated confirmation page were
also checked. The cafe was prepared, not submitted for public approval.

Deployment backups: DCT `/opt/dc-terminal/deployment/before-publishing`; website
`/opt/prod/ai.tzspace.cn/backups/before-dct-publishing` includes an online SQLite
backup and source. Website previous image tag is
`aitzspacecn-ai-mania:before-dct-publishing`. The migration only adds DCT tables.
Preserve `/var/lib/dcw-classroom/publishing`, manager state, and server-only config
when replacing the application. To prune artifacts, first verify they are not
current/approved, stop the manager (not student containers), remove only selected
version directories, and restart; do not delete student work volumes.

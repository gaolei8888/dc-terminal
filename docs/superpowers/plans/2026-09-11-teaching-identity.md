# Teaching identity integration implementation plan

**Goal:** Students and authorized teachers enter DCT from their existing teaching applications.
**Architecture:** Browser-bound, one-time ticket handoff; issuer permission callbacks; DCT workspace ownership keyed by tenant + user ID. Separate issuer keys, fail closed, keep existing local administration.
**Spec:** ../specs/2026-09-11-teaching-integration.md
**Stack:** Node built-ins, Grails controllers/services/GSP, existing GORM users and class membership.

- [ ] Add `container/classroom/sso.mjs` and `sso.test.mjs`: issuer authentication, browser challenge, expiring single-use tickets, identity binding, permission callback, scoped HTTP and WebSocket access. Tests must deny replay, wrong browser, expired ticket, wrong issuer role, wrong tenant, unauthorized roster target and fail-closed callback.
- [ ] Update manager `server.mjs` and `admin.html`: filtered roster and operations, per-request scope verification, visible class labels, hide local-account management for federated teachers. Re-run existing server/admin browser tests.
- [ ] In dc_classroom add DctBridgeController/DctBridgeService with tests, enabled-only dashboard/nav entry, existing student entitlement checks and permission callback. Run targeted specs and full required test/build checks.
- [ ] In dc_classeditor add equivalent teacher bridge with strict roster lookup and platform-vs-school administrator scopes. Add management menu entry, test tenant/teacher/class boundaries and build.
- [ ] Verify cross-language issuer protocol against local DCT; back up production artifacts/config/data; deploy without replacing active student containers. Verify existing links and authenticated student/teacher redirects using real roles when credentials available.
- [ ] Record remaining DNS/previews/publishing/backup work explicitly; do not claim full integration until every phase is verified.

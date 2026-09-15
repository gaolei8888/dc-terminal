# Fix wave (node) report

Commit: 659262c "fix(classroom): heal public-live through relay restarts and room churn"

## I1 — relay restart must not permanently unpublish
Added `probeLanes(url, id)` using the viewer token from `status.url`'s `#t=` fragment
against `GET /live/{id}/lanes`. In `healPublic`, a PUT 401 now only clears+errors the
record if the probe confirms the room is present (200); a probe 401/throw means "not
registered yet" — record and any existing error state are left alone, retried next tick.
403 unchanged (terminal). Two dedicated tests added; the existing revocation test's fake
relay now models a `registered` room set so its true-401 assertions still hold.

## I2 — a publish is for one room
`publishLive` now stores `{title, id}` (id = room id `relayPublic` used). `healPublic`
compares the record's id against the room's current id each tick: mismatch → clear
silently (store.mutate re-checks both title and id), no error, no re-PUT — that
publish's live has ended. Legacy records with no `id` adopt the current room's id on
first heal rather than being treated as stale. Two tests added (room-changed clears
without PUT; legacy record adopts id).

## T9 — live-stop clears livePublicError
`delete w.livePublicError` added alongside `delete w.livePublic` in the live-stop
branch. Assertion added in the existing public-live test.

## Tests
`node --test server.test.mjs`: 14/14 pass (10 pre-existing + 4 new).

No contradictions found between the brief and the code.

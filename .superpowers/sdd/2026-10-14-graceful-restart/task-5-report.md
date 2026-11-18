# Task 5 report

Commit a9dc316. Full cargo test green (lib 1844), clippy 1.99.0 -D warnings clean. Mutation check: disabling the tick gate makes tick_keeps_queue_while_draining_and_delivers_after fail.

## Callers of bridge::submit
- bridge.rs SessionWriter::type_into for SessionManager (phone/Telegram deliver) -> GATED.
- mesh/deliver.rs LocalInbox::type_into (mesh immediate delivery + tick) -> GATED (Err), but see mesh changes below so it queues instead of failing.
- NEW daemon.rs Request::Submit (voice) -> GATED.
- Not via submit, UNGATED (user's own): Request::Input -> send_input (ui/grid, attach, pick, ui/mod, web/routes), Request::Key. Web phone page (web/routes.rs) also uses Input/Key: left ungated (it is the user's own remote view, not an automated source) - flag if you disagree.

## Voice choice
Voice used two Request::Input calls, so it bypassed submit. Added `Request::Submit{id,text}` (daemon runs bridge::submit: write+Enter in one call, refused with ErrorCode::RestartPending while draining). Voice SocketDaemon::type_into now sends Submit; RestartPending -> "dct 正在准备重启,稍后再发" shown as "没送进去：..." by voice. UI Input untouched. Protocol: new Request variant added, PROTOCOL_VERSION NOT bumped (Task 6 does 24->25); the request-shape pin test string was updated with {"Submit":...} after Input but still says version 24 - Task 6 must update only the version number (and may need to re-check the pin).

## User-facing text path
- Phone: bridge deliver's Err branch used a generic "这句话没能敲进...". Added `SessionWriter::draining()` (default false; SessionManager returns is_draining); on failure while draining the reply (and Delivered::Failed msg) is exactly RESTART_PENDING_MSG. Test: the_phone_reply_says_dct_is_preparing_to_restart.
- ErrorCode::RestartPending added with i18n zh/en (incl. exhaustive test list).

## Deviations from plan
- tick: draining check placed BEFORE take_queued (plan's snippet sat after jobs were taken, which would drop them).
- Extra: mesh decide path (deliver.rs ~:464) would TypeNow an incoming message to an Idle session, then fail via gated submit; now it queues when inbox.draining(). Test added.
- submit_unchecked has no caller yet except tests (dead-code OK since pub(crate) used in tests only? clippy clean).

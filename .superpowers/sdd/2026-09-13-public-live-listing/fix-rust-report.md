# Fix wave (rust) report

## I3 — paste into the live panel input — done
`src/ui/mod.rs`: added `View::Live { input: Some(input_field), .. }` arm to the
paste handler, delegating to new `paste_into_live_input()`. Key input reuses
`clean_secret`; Title strips control chars (incl. `\n`). Note: brief said
`LiveInput::Title{..}` but it's actually a tuple variant `Title(String)`
(`src/ui/view.rs:91`) — used the real shape. Brief also assumed the keypress
path (`edit_live_input` in `src/ui/live.rs`) enforces a length cap; it does
not (no cap found) — so paste doesn't impose an artificial one either, to
stay consistent with the keypress path's actual behavior.
Tests: `pasting_a_key_fills_the_live_key_buf`,
`pasting_into_the_live_title_strips_newlines` (in `src/ui/mod.rs` tests).

## M1 — daemon marks a valid key "stuck" after a relay restart race — done
`src/live.rs` pusher loop: split the PUT-failure match so `401` alone probes
`fetch_lanes` with the viewer token before deciding. Lanes also 401 → room is
really gone → clear `started`, do not set `public_stuck`. Lanes `Ok` → real
key rejection → `public_stuck` as before. Lanes errors otherwise (network) →
treated as a plain retryable failure (backoff), not a verdict either way.
`403`/`413` are unaffected (still stuck immediately).
Extended `FakeState.forget_room` in the test harness to also 401 `PUT
.../public` (it previously only 401'd frame-push/lanes), since that's what
makes the race reproducible.
Test: `a_put_401_during_a_relay_restart_is_not_mistaken_for_a_real_key_rejection`.

## M3 — daemon refuses an empty title — done
`src/daemon.rs::live_publish`: after trim+truncate, if the title is empty,
return `Response::Error(ErrorCode::BadRequest("标题不能为空"))` without
calling `live.publish`. Reused existing `BadRequest(String)` — it's already
used for other ad hoc validation failures (`daemon.rs:747,1213`, `link.rs`),
no new ErrorCode variant, no protocol bump.
Test: `live_publish_refuses_a_title_that_is_empty_after_trimming` (also
asserts existing public state is untouched).

## T2 — `dct-srv key add --file X` creates a key named "--file" — done
`crates/dct-srv/src/lib.rs::parse_cli`: `name()` now filters out values
starting with `--` before accepting `args.get(2)`, falling through to the
existing "缺少名字" error.
Test: `key_add_without_a_name_does_not_treat_the_file_flag_as_the_name`
(covers `key add` and `key revoke`).

## T4 — misplaced doc comment — done
`crates/dct-srv/src/lib.rs`: moved the doc comment describing the
`/live/{id}/lanes` response from above `struct PublicTitle` to above
`struct LiveLanesResponse` (the item it actually describes). No behavior
change. No `dct-link` match for `LiveLanesResponse`/`PublicTitle` — that pair
only exists in `dct-srv`; the mirrored client type in `src/live.rs`
(`LanesBody`/`PublicTitleBody`) was already correctly documented.

## Verification run
- `cargo test --lib ui::` — 539 passed
- `cargo test --lib live::` — 87 passed
- `cargo test --lib daemon::` — 43 passed
- `cargo test -p dct-srv` — 72 + 3 passed (1 ignored, pre-existing)
- `cargo clippy --lib --tests -- -D warnings` — clean
- `cargo clippy -p dct-srv --all-targets -- -D warnings` — clean
- `git diff --check` on touched files — clean

## Commits
See `git log` on this worktree, commits touching only:
`src/ui/mod.rs`, `src/live.rs`, `src/daemon.rs`, `crates/dct-srv/src/lib.rs`.

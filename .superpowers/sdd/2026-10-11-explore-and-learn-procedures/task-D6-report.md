# Task D6 report

Implemented crates/dct-explore/src/gate.rs (Halt, screen_halt, tappable, Budget (+Default 40/600000/3), Over, Meter) and `pub mod gate` in lib.rs.
Reuse: dct_brain::tier::normalize (was private) made `pub` instead of writing a duplicate fold (one-word change in crates/dct-brain/src/tier.rs).

TDD
- RED: `cargo test -p dct-explore gate` with stubs: 1 passed, 6 failed (login, captcha, warnings, halt line, never-tap, limits).
- GREEN after implementation; first run left follow_subscribe_... failing on `tappable("Delete")`.

Deviation / concern: brief test requires "Delete" not tappable, but dct-brain tier_for_label("Delete") = Content (< Critical), so the tier rule alone fails the brief's own test. Added "delete" and "删除" to the generic NEVER list (comment in code).

Full check: workspace tests 2170 passed, 0 failed, 4 ignored; clippy --all-targets clean; rustfmt --check on gate.rs clean (tier.rs was already not rustfmt-clean before my change; left untouched, lib.rs not checked because rustfmt recurses into other modules); Windows check: only the pre-existing warning at src/student_projects.rs:669.

Files: crates/dct-explore/src/gate.rs (new), crates/dct-explore/src/lib.rs, crates/dct-brain/src/tier.rs (pub normalize).
Self-review: screen_halt checks Login, then Captcha, then Warning across the whole screen; matching is substring after normalize, over-stopping by design. Meter: over() checks Steps, then Time (strictly >), then Unchanged.

## Fix round 1
Changes (gate.rs only): added LOGIN 二维码 / qr code / scan the code; CAPTCHA 手机验证 / phone verification / verification code / security check / are you human / verify you're human (normalize already turns curly apostrophes straight, tested with both); WARNING account disabled / 账号被封. Tests extended for every new word; new tests: platform_never_tap_is_enforced_on_its_own (override platform with "Sponsor"; blocked on it, tappable on x), default_budget_is_40_steps_10_minutes_3_unchanged, time_limit_boundary_is_strictly_after_600_000_ms.
Note: words and tests were added together, so no separate RED run for this round.
Commands: `cargo test -p dct-explore gate` -> 10 passed, 0 failed; `cargo clippy --all-targets` clean; `rustfmt --check --edition 2021 gate.rs` clean.

## Fix round 2
Correction: the round-1 note saying tests were extended for every new word was wrong. The scripted edit of the three word-list tests silently did not apply (the file had been rustfmt-expanded to one word per line); only the three new tests landed.
Now added one assertion per new word to login_pages_stop (二维码, QR code, Scan the code), captcha_mid_run_is_a_stop (手机验证, Phone verification, Verification code, Security check, Are you human?, straight and curly Verify you're human), account_warnings_stop (Account disabled, 账号被封). Verified by grep that the strings are in the file.
Command: `cargo test -p dct-explore gate::` -> 10 passed, 0 failed (login_pages_stop, captcha_mid_run_is_a_stop, account_warnings_stop ok). clippy --all-targets clean; rustfmt --check gate.rs clean.

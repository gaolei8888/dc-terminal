# Task 7 report
- Added in src/client.rs: spawn_swap, swap_daemon, wait_old_gone, log_swap_failure; test swap_tests::swap_waits_for_old_process_then_hard_kills_it (unix only; reaper thread avoids zombie false-alive).
- main.rs: `__swap` arm (not in --help, verified).
- Hello check uses crate::proto::PROTOCOL_VERSION; wait up to 10s. grace passed 10s from main.
- Repo had no daemon.log writer; added a small one (appends "[epoch secs] 平稳重启失败：<reason>" in <socket dir>/daemon.log). Plain epoch timestamp since no time helper in proto.
- current_exe: repo has no " (deleted)" handling; same as restart caller (std::env::current_exe()).
- Note: rustfmt run on client.rs only (it reformatted nothing else). Clippy 1.99.0 clean. Spawn half untested (Task 10).

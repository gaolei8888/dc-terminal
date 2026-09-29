# Task 9 report — local end-to-end and go-live notes

Status: DONE_WITH_CONCERNS

Worktree: /Users/lei/work/dc/dc-terminal-mesh (BASE 35923b7)

Commits:
- 71c9242 test(mesh): local end-to-end over a real relay; deployment notes
- b078195 test(mesh): the production gateway's probe token verifies under dct-mesh

Not pushed, not merged. The pre-existing modification to `.superpowers/sdd/.gitignore` in the worktree was left alone and not committed.

## 1. tests/mesh_e2e.rs (#[ignore])

`cargo test --test mesh_e2e -- --ignored --nocapture` passes, about 1.9 s once built. I ran it 8 times: 6 passed and 2 failed on a race in the test itself (fixed, see below). All 5 runs after the fix passed.

How it works:
- **Relay (real)**: `cargo build -p dct-srv` first. It then starts the `dct-srv` binary as `dct-srv 127.0.0.1:0 --with-link --relay-keys <tmp>/issuer.pub`, and the key pair comes from `dct-srv token keygen --out <tmp>`. The test captures the relay's stdout and stderr.
- **Gateway (stand-in)**: a hand-written `TcpListener` that serves `POST /admin/api/relay/token`. It reads the `endpoint` from the body and signs with `dct-srv token mint` using a 7-day TTL, the same as dc_llm's `TOKEN_TTL_SECONDS`. dct reaches it through the real `MeshLogin` path (`http_transport` and the `pair_origin` of a `dc` profile whose `base_url` points at the stand-in).
- **Two computers**: `common::start_daemon()` twice. Each has its own temp home: socket, secrets, profiles, `mesh/name`, and `config.toml` with `[mesh] relay = <relay>`. These all follow the socket, so the two share nothing. I used the same in-process `dct::daemon::run` that the other integration tests use, not two `DCT_HOME` subprocesses.
- **Agent (stand-in)**: a `is_agent = true` profile, `[posix_tool("sh"), "-c", "echo E2E-READY; exec cat"]`, with `idle_pattern = "E2E-READY"`. On Windows, `sh` and `cat` come from Git for Windows, as in the other tests.
- **Flow and assertions**:
  - login on A and B. The stand-in gateway saw exactly one call per machine, with `Bearer <that machine's api_key>` and its own endpoint.
  - B sends `MeshJoin`, retried until A answers.
  - A's `MeshStatus.pending` shows B, **with the same 6-digit code B sees**.
  - B sends `MeshConfirmInviter(A)`, then A sends `MeshApprove(endpoint, code, yes)`. B's roster then contains A.
  - B creates an agent session in a git repo.
  - A's `MeshPeers` shows B online with 1 session.
  - A sends `MeshSend("电脑B/#id")` and gets back Delivered or Queued.
  - B's screen contains `[来自 电脑A/终端 的留言 #xxxx]` with a 4-character short id, followed by the body.
  - B's `MeshView.messages[id] == 1`.
  - The relay's full output contains neither the body nor the `e2e-canary` string.
- **Race fixed**: the first version asserted the ✉ counter right after the text appeared on screen. The counter is recorded only after the whole submit, Enter included, returns, so the assertion now polls.
- **Windows**: written to be portable (no unix-only APIs, backslash-escaped TOML paths, `EXE_SUFFIX`). It has **not been run on Windows**.

## 2. Cross-language probe token

I found the probe token and the production **public** key in `progress.md`, in the note from the dc-llm session. No private key was read.

The new unit test `relay_token::tests::a_probe_token_signed_by_the_production_gateway_key_verifies` (dct-mesh, not ignored) checks two things:
- `verify` accepts the token under the production public key `BNEgy8UH…Ze0=` and returns `account="usr_01PROBE"`, `endpoint="c-0011223344556677889a"`, `exp=4102444800`.
- The same token is rejected under a different key.

The test passes, so the Python gateway and dct-mesh agree on the token format. dct-mesh: 90 passed.

## 3. README

- **Language**: README.md is English and the Chinese README is README.zh-CN.md. So the Chinese 「多电脑」 section, with its 「部署」 subsection, went into **README.zh-CN.md**, and an English equivalent ("Several computers" / "Deploying") went into README.md. Both "What's new" lists link to the new section.
- **Content**: usage (login, join, approve, peers, send, marker format, relay config), a short known-limits list (from the rulings in progress.md), and the 4-step go-live checklist. It also tells readers how to run the local e2e.
- **Flag name**: README gives the gateway's real name `DC_ADMIN_RELAY_TOKENS_ENABLED`, notes that the design doc writes it as `DC_RELAY_TOKENS_ENABLED`, and names `DC_ADMIN_RELAY_SIGNING_KEY` and `scripts.relay_signing_key show`. I confirmed these in `dc_llm/deploy/production/docker-compose.yml` and commit d05aff1.
- Nothing was deployed.

## 4. Raw LF into a real claude/codex CLI

Skipped. Both CLIs are installed (`~/.local/bin/claude`, `codex` under nvm). Checking would mean starting an authenticated agent session and typing text that could be sent as a real prompt on the user's account. That is not a trivial or side-effect-free check, so it remains unverified.

With the stand-in agent (`cat` in canonical mode), the two lines arrive as two lines, as expected. That says nothing about the claude or codex input boxes.

## Concerns

1. **The brief's relay command line is wrong.** `dct-srv serve --with-link --relay-keys … --addr 127.0.0.1:<port>` would try to bind an address called `serve`, because the parser takes the first non-flag argument as the address, has no `serve` subcommand and no `--addr`. README documents the syntax that works: `dct-srv 127.0.0.1:<port> --with-link --relay-keys /etc/dct-srv/gateway.pub`. The e2e test uses the same form.
2. **Renewal loop when the TTL is under a day.** If a token's TTL is under 1 day, dct renews before every poll: `needs_renewal` stays true and `last_failure` is set only on failures. My first run with a 1-hour test TTL showed 2 gateway calls per login right away. The gateway's real TTL is 7 days, so production is not affected. But if the gateway TTL were ever cut to 1 day or less, every connected machine would call the gateway continuously. A later fix could be a minimum renewal interval after a success too.
3. **What `--with-link` mounts.** It also mounts `/phone`, as well as `/link/*`. README tells the operator to proxy only `/dct-relay/link/*` with the prefix stripped.
4. **The live relay on dataclue.cn.** The public live listing already runs a `dct-srv` there (see memory). Whether the mesh relay reuses that process or runs as a second instance on another port is an operator decision. Restarting the live one with new flags would interrupt live sessions. The checklist does not decide this.
5. **The relay-log check is weak.** `dct-srv` hardly logs at all; its whole output is the one startup line. Confidentiality really rests on the sealing (Tasks 2–3). The e2e check only guards against a future log line that prints payloads.
6. Windows has not been run (see section 1). The LF behaviour in real agent CLIs is still unverified (see section 4).
7. **The spec has no 「已知限制」 section yet**, though the Task 6 ruling said to add one at finish. README now has a known-limits list, but the spec itself is unchanged.

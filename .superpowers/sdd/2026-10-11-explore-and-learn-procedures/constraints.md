## Global Constraints

- 每一步都先 Jeff，`answer_confidence` < 门槛（起始 **0.8**，配置 `[judge] threshold`）或照它选的做了画面没变 / 绕回了见过的画面 → 大模型；大模型说卡住、读不懂、照它选的做了还是不对、或同一画面已试 **2** 次 → 带编号图问用户。
- 每个回答记 `by: jeff | model | user`；大模型和用户纠正 Jeff 的题存进 `~/.dct/explore/answers.jsonl`（只存，不微调）。dco 的 `answer_run.by` 只有 `model | user`，Jeff 的回答发 `model`，dct 自己的记录里写 `jeff`。
- 第 1 步必须是 `open_url`，网址取自平台表，域名写死；模型不能决定去哪个网站。
- 登录页、验证码、二维码、手机验证、账号异常一类画面：**停**，交给用户；这些画面的字不发给 Jeff、不发给大模型。dco 回 `private_screen` 也一样停。
- 关注、订阅、Boost / Promote（付费推广）、平台表的禁点按钮一律不进选项；`dct_brain::tier::tier_for_label(label) >= Tier::Critical` 的也不进选项（dco 本来也会拒）。
- 摸索最多 **40** 步、最长 **10 分钟**；连续 **3** 步画面没变就停。
- 生成的流程名 `promote-<平台>-<种类>`（种类：有文件参数是 `video`，否则 `post`）；第 2 步自动插入登录闸 `verify_text`；完成标志不能含参数占位符，也不能含本次填进去的字。
- 草稿进 dcv 一律 `--source import:agent`、`--learned-from run:<编号>`；拒收最多改 **3** 轮；**agent 写的东西永远不给自己放行**：用户确认 → `dct procedure approve`（Touch ID）→ `dcv approve --sha`。
- 选择题选项 **2–20** 个（judge 设计；Jeff 自己能到 26，我们按 20）。
- dco 永远不调模型、不做判断、不存流程。
- 不写任何平台专用代码；平台差异只在平台表里。
- 写到磁盘的记录（轨迹、answers.jsonl、日志）里，参数值一律换回 `{参数名}`。
- 提交信息用英文。dco 和 dct 仓库不加任何 AI 署名行（dco 的 CLAUDE.md；dct 近来的提交也没有）；dcv 照它自己近来的提交习惯（带 `Co-Authored-By`）。
- cargo 在 `~/.cargo/bin`，不在默认 PATH：命令一律写 `~/.cargo/bin/cargo`。
- dct 仓库的工作区现在有别人没提交的改动（`src/daemon.rs`、`src/pty.rs`、`src/session.rs`、`src/ui/*`、`tests/concurrency.rs`）：执行前用 `git worktree add ../dc-terminal-explore -b explore-learn` 开一个干净的工作树，不碰那些文件。dco、dcv 同理各开一个分支。

## Review Focus

1. **同一个字的两个按钮**（X 左栏 `Post` 和发帖框 `Post`）：摸索时必须点 Jeff 选的那个，而且重放时不能点到另一个。→ Task D10 加测试 `two_post_buttons_are_answered_with_the_one_jeff_chose`；Task D14 加测试 `replay_help_on_duplicate_text_goes_through_the_ladder`。
2. **只偶尔出现的弹窗被记成固定步骤**（TikTok 的 `Got it`、`Continue to post?`）：下次没弹，重放卡在这一步。期望：拒绝类按钮变成弹窗规则草稿，不进步骤；其余的照实记，但确认卡片上标出来。→ Task D11 加测试 `a_dismiss_tap_becomes_a_popup_draft_not_a_step` 和 `a_non_dismiss_popup_step_is_flagged_for_the_user`。
3. **后一个动作把前面填的字清掉了**（X：先打字后加图）：发布前画面上必须看得到要发的字，否则发布按钮不出现。→ Task D7 加测试 `publish_is_hidden_until_every_text_param_shows`；Task D10 加测试 `text_wiped_by_attach_is_typed_again_before_publish`。
4. **半路冒出来的登录 / 验证码 / 账号异常**（不只在开头）：立即停，画面的字一个都不发给 Jeff 或大模型。→ Task D6 加测试 `captcha_mid_run_is_a_stop`；Task D10 加测试 `a_captcha_in_the_middle_stops_without_asking_any_model`。
5. **开着好几个 Chrome 窗口 / 窗口半路被关**：只按 `window_id` 认窗口；不止一个候选时要用户指定，不猜。→ Task D16 加测试 `several_browser_windows_need_an_explicit_window`；Task D10 加测试 `a_closed_window_stops_the_run`。


Controller rulings binding every task:
- Ruling R1: Full check per task = `cargo test --workspace -- --test-threads=1` + `cargo clippy --all-targets` (no warnings) + rustfmt clean on every file the task creates or touches (`rustfmt --check --edition 2021 <files>`); do NOT reformat the 55 pre-existing unformatted files — baseline `cargo fmt --check` already fails there (rustfmt drift) and reformatting would bury the diff — cost if wrong: one later mechanical `cargo fmt` commit.
- Ruling R2: D14 ReplayInput gains `platform: &'a Platform` so `preview` can compute publish_step — the plan's publish_step needs the platform's publish words and replay has no other source — cost if wrong: one extra field.
- Ruling R3: Branch name `explore-and-learn` (dispatcher's), not plan's `explore-learn` — dispatcher instruction wins — cost: none.
- Ruling R4: The repo tracks .superpowers/sdd briefs/reports/progress (its .gitignore says so; sdd-workspace rewrote it to `*`, restored with git checkout). Implementers do not commit sdd files; controller commits the ledger/briefs/reports in one docs commit at the end — cost if wrong: one commit to drop.
- Ruling R5: Commits carry no AI attribution / Co-Authored-By (user rule overrides the harness reminder).
- Ruling R6: Every task also runs `cargo check --workspace --target x86_64-pc-windows-msvc` and must add no errors/warnings (baseline: one pre-existing warning src/student_projects.rs:669).

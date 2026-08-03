### Task 7: 入口组装与守护进程自动拉起

**Files:**
- Modify: `src/main.rs`
- Create: `README.md`

**Interfaces:**
- Consumes: `daemon::run`、`client::Client`、`ui::run`、`proto::socket_path`
- Produces: 可执行的 `dct` 与 `dct daemon`

- [ ] **Step 1: 写失败的测试**

`tests/cli.rs`：

```rust
use std::process::Command;

#[test]
fn daemon_subcommand_is_recognized() {
    // --help 必须提到 daemon 子命令
    let out = Command::new(env!("CARGO_BIN_EXE_dct")).arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("daemon"), "帮助里应当有 daemon: {text}");
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let out = Command::new(env!("CARGO_BIN_EXE_dct")).arg("bogus").output().unwrap();
    assert!(!out.status.success());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test cli`
Expected: FAIL，`--help` 输出里没有 `daemon`。

- [ ] **Step 3: 实现 main**

`src/main.rs`：

```rust
use anyhow::Result;
use std::time::Duration;

use dct::client::Client;
use dct::proto::socket_path;

const HELP: &str = "\
dct —— vibe coding 终端

用法：
  dct           打开会话看板（守护进程没在跑就自动拉起）
  dct daemon    只跑守护进程，不开界面
  dct --help    看这段
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        None => run_ui(),
        Some("daemon") => dct::daemon::run(&socket_path()),
        Some("--help") | Some("-h") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => {
            eprintln!("不认识的命令：{other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn run_ui() -> Result<()> {
    let sock = socket_path();
    if Client::connect(&sock).is_err() {
        // 拉起守护进程后等它把 socket 建好
        std::process::Command::new(std::env::current_exe()?).arg("daemon").spawn()?;
        for _ in 0..50 {
            if Client::connect(&sock).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let client = Client::connect(&sock)?;
    dct::ui::run(client, std::env::current_dir()?)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --test cli`
Expected: 2 个测试 PASS。

再跑全量：`cargo test -- --test-threads=1`。

- [ ] **Step 5: 手动验证并校准 claude 的 idle 正则**

这一步必须真跑，不能只靠测试：

```bash
cargo build --release
cd <一个 git 仓库>
/Users/lei/work/dc/dc-terminal/target/release/dct
```

依次确认：

1. 按 `n`，选 `claude`，看板出现一个会话
2. 会话目录含 `.dct-worktrees`（**不在主工作树**）
3. `Enter` 进去，能看到 Claude Code 界面，打字有反应，全程不弹权限确认
4. 让它改一个文件，`Esc` 回看板按 `d`，能看到文件名和 +N -M
5. 按 `u`，回到改动之前
6. 按 `s`，状态变「已停止」

第 3 步如果状态一直显示「干活中」不变成「空闲」，说明 `profiles/claude.toml` 的 `idle_pattern` 和实际界面对不上。这时候：在会话里按 `Enter` 进屏幕视图，把底部那行提示的原文抄下来，改 `idle_pattern` 让它匹配，重新 `cargo build --release` 再试。

- [ ] **Step 6: 写 README**

`README.md`：

```markdown
# dc-terminal

面向 vibe coding 的 agent 终端。开了任务之后人可以离开电脑，在手机上继续操控。

设计文档：`docs/superpowers/specs/2026-08-01-dc-terminal-design.md`

## 现在能做什么

- 会话看板：多个 agent 会话并行，一屏看状态
- 全自动接受权限，agent 不会停下来问
- 每个 agent 会话跑在独立 git worktree，绝不动主工作树
- 每轮前自动检查点，一键回滚
- 守护进程常驻，关掉终端窗口不影响会话

## 还没做

Bridge 与 `ask_human`、手机通道、`dc_llm` 压缩与归类、手机端命令、Codex profile。

## 用法

    cargo build --release
    ./target/release/dct

按键：`n` 新建，`↑↓` 选择，`Enter` 进入，`u` 回滚，`s` 停止，`d` 看改动，`q` 退出。

## 加一个新 agent

在 `profiles/` 下加一个 TOML，不用改代码：

    name = "myagent"
    command = ["myagent", "--yolo"]
    is_agent = true
    idle_pattern = "> $"
```

- [ ] **Step 7: 提交**

```bash
git add src/ tests/ README.md profiles/
git commit -m "feat: dct 入口、守护进程自动拉起与 README"
```

---

## 完成标准

全部任务做完后：

```bash
cargo test -- --test-threads=1   # 全绿
cargo clippy -- -D warnings      # 无警告
cargo fmt --check                # 格式干净
```

再加上 Task 7 Step 5 的六条手动验证全部通过。

## 下一份计划

Bridge（`ask_human` + token 鉴权）、Relay（Telegram 优先 + 主备切换）、`dc_llm` 的压缩与归类、手机端 `/new` `/list` `/diff` `/undo` `/stop`。协议层已经在本计划里立好，手机端就是 `Client` 之外的第二个客户端。

另有一条 spec 要求本计划**没有覆盖**，留到下一份：守护进程重启后列出上次遗留的 worktree 让用户决定删还是接着用。本计划里遗留 worktree 需要手工 `git worktree remove` 清理。会话清理逻辑等到有调用点时再写，现在不预留空方法。

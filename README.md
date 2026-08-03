# dc-terminal

面向 vibe coding 的 agent 终端。开了任务之后人可以离开电脑，在手机上继续操控。

设计文档：`docs/superpowers/specs/2026-08-01-dc-terminal-design.md`

## 现在能做什么

- 会话看板：多个 agent 会话并行，一屏看状态
- 全自动接受权限，agent 不会停下来问
- agent 直接在你的真实项目里干活
- 每轮前自动拍一张隐藏快照，一键撤销；不动你的分支和提交历史
- 守护进程常驻，关掉终端窗口不影响会话

## 还没做

Bridge 与 `ask_human`、手机通道（Telegram / 飞书 / 企业微信 / SMS）、`dc_llm` 的压缩与归类、手机端命令、Codex profile。

## 用法

需要 Rust ≥ 1.80。

```
cargo build --release
./target/release/dct
```

| 命令 | 作用 |
|---|---|
| `dct` | 打开会话看板，守护进程没在跑就自动拉起 |
| `dct daemon` | 只跑守护进程，不开界面 |
| `dct --help` | 用法 |

看板按键：

| 键 | 作用 |
|---|---|
| `n` | 新建会话（选 agent） |
| `p` | 换项目（新会话开在哪个目录） |
| `↑` `↓` | 选择会话 |
| `Enter` | 进入会话屏幕；再按 `F2` 返回看板 |
| `u` | 回滚到上一个检查点 |
| `s` | 停止会话 |
| `d` | 看这个会话改了哪些文件 |
| `q` | 退出看板（守护进程继续跑） |

进入会话屏幕后打字直接送给 agent，`Esc` 也会送给 agent（它靠这个键取消/清空/关弹窗）；回看板用 `F2`。

## 加一个新 agent

在 `profiles/` 下加一个 TOML，不用改代码：

```toml
name = "myagent"
command = ["myagent", "--yolo"]
is_agent = true
idle_pattern = "> $"
```

- `command` 里要带上这个 agent 自己的权限绕过参数，否则它还是会停下来问
- `is_agent = true` 的会话会自动拍快照、支持撤销；`false`（比如普通 shell）不会
- `idle_pattern` 是判断"干完活了"的正则，匹配的是屏幕文本

## 已知限制

- 同一个项目里同时开两个 agent 会互相踩改动。跨项目并行没问题。
- agent 会话绑定一个目录直到结束。换项目就是开新会话。
- 权限全自动接受，所以 agent 也可能写到项目目录之外；那部分改动不在快照覆盖范围内，撤销撤不回来。
- `profiles/claude.toml` 的 `idle_pattern` 需要按实际界面校准；对不上的话会话状态会一直停在"干活中"。

## 开发

```
export PATH="$HOME/.cargo/bin:$PATH"   # 如果 Rust 装在 ~/.cargo 且没改 shell 配置
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

测试会真的建临时 git 仓库、起子进程、绑 Unix socket，所以串行跑更稳。

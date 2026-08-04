# dc-terminal

面向 vibe coding 的 agent 终端。开了任务之后人可以离开电脑，在手机上继续操控。

[English](README.md) · 设计文档：`docs/superpowers/specs/2026-08-01-dc-terminal-design.md`

`dct` 让多个 coding agent 并行干活，每个待在自己的项目目录里，一屏看完。权限全自动接受，agent 不会停下来问你。每轮开始前自动拍一张隐藏快照，一个键就能撤销 agent 刚才干的事——不动你的分支，也不动提交历史。

---

# 给用户

## 安装

需要一个较新的 stable Rust（1.80 或更高），以及一套 C 工具链（macOS 装 Xcode 命令行工具，Linux 装 build-essential 或等价包）——依赖里有一份 TLS 库要在构建时编译原生代码。

```
cargo build --release
./target/release/dct
```

装就这么多。第一次跑 `dct` 会自动拉起后台守护进程；关掉终端窗口不影响正在跑的会话。

## 看板

跑 `dct` 打开看板——一个会话一行，带上每个 agent 此刻在干什么。

| 键 | 作用 |
|---|---|
| `n` | 新建会话，直接用上次那个 agent（不弹菜单） |
| `N` | 新建会话，选 agent |
| `p` | 换项目——下一个会话开在哪个目录 |
| `c` | 管理密钥——改一个已存的，或者删掉它 |
| `↑` `↓` | 在会话之间移动 |
| `Enter` | 进入会话；`F2` 回看板 |
| `u` | 撤销——回滚到上一个检查点 |
| `s` | 停止会话 |
| `d` | 看这个会话改了哪些文件 |
| `q` | 退出看板（守护进程和会话继续跑） |
| `Ctrl+Q` | 退一层。在会话里回看板，在看板上退出。 |

进了会话，你打的每个键都发给 agent，包括 `Esc`——agent 靠它取消操作、关自己的弹窗。`F2` 和 `Ctrl+Q` 是 `dct` 唯一留给自己的两个键。

## agent

按 `N` 会列出全部，不管这台机器上能不能用。用不了的置灰并写明原因，而且选中它是带你去解决问题，不是把你打发走。第一次选完之后，`n` 就记住了它——下次按 `n` 直接进去，不再弹菜单。

| agent | 是什么 | 需要 |
|---|---|---|
| Claude | Anthropic 官方命令行 | 装了 `claude` |
| Codex | OpenAI 官方命令行 | 装了 `codex` |
| OpenCode | 开源，可接多种模型 | 装了 `opencode` |
| Qwen Code | 阿里通义，独立命令行 | 装了 `qwen` |
| Kimi | 月之暗面，套用 Claude 界面 | 装了 `claude` + 一份密钥 |
| GLM | 智谱，套用 Claude 界面 | 装了 `claude` + 一份密钥 |
| DeepSeek | 深度求索，套用 Claude 界面 | 装了 `claude` + 一份密钥 |
| Qwen API | 阿里通义，套用 Claude 界面 | 装了 `claude` + 一份密钥 |
| 命令行 | 普通终端，不带 AI | — |

后面四个不是独立程序。它们跑的是 `claude`，只是把地址换到各家的 Anthropic 兼容端点——所以既要装 `claude`，又要各自的一份密钥。

**没装？** 照样选。`dct` 知道怎么装的话，会开一个会话把安装命令跑起来，你全程看得见。

**没填密钥？** 照样选。会出现一个输入框让你粘贴，旁边带申领页面的地址，按 `Ctrl+O` 直接在浏览器里打开。密钥存盘前先验一下，粘错了当场就告诉你，而不是等你进去以后看一屏英文报错。

密钥存在 `~/.dct/secrets.toml`，只有你自己读得了。它们绝不会写进 profile 文件，所以那些文件可以随便拷贝分享。

**改主意了，或者密钥失效了？** 在看板上按 `c`。这一页只列真正需要密钥的那几个 agent，每一行标着已配还是未配——选中一个按 `Enter` 换掉它，按 `d` 删掉它。密钥只该在这里改，不需要也不支持手动去改 `secrets.toml`。

## 加自己的 agent

往 `~/.dct/profiles/` 丢一个 TOML。不用改代码，也不用重启——`dct` 每次请求都重新读这个目录。`name` 和内置的重名就覆盖它，其它名字就是新增一项。

```toml
name = "myagent"
command = ["myagent", "--yolo"]
is_agent = true
busy_pattern = "esc to interrupt"

[label]
zh = "我的 agent"
en = "My agent"

[note]
zh = "这个擅长干什么"
```

| 字段 | 含义 |
|---|---|
| `command` | 要带上这个 agent 自己的权限绕过参数，否则它还是会停下来问 |
| `is_agent` | `true` 才有快照和撤销；`false`（比如普通 shell）没有 |
| `busy_pattern` | 拿来匹配屏幕文本的正则。匹配上=干活中，没匹配上=空闲 |
| `idle_pattern` | 反过来：匹配上=空闲。两个都写了以 `busy_pattern` 为准 |
| `env` | 给进程的环境变量——比如换一个 base URL |
| `secret` | 声明这个 agent 需要用户提供密钥，以及注到哪个变量里 |
| `install` | 这台机器上没有的话，怎么装 |

能用 `busy_pattern` 就别用 `idle_pattern`。agent 干活时那句「按 esc 中断」是稳定的；空闲时输入框里的占位符，用户一打字就没了。两个都不填也行——看板会老老实实显示 `—`，不编一个状态出来。

你写的文件没出现在菜单里，选择器会告诉你为什么，带行号。

## 已知限制

- 同一个项目里开两个 agent 会互相踩改动。跨项目并行没问题。
- 一个会话从开到关绑定一个目录。换项目就是开新会话。
- 权限全自动接受，所以 agent 也可能写到项目目录之外。那部分改动不在快照范围内，撤销撤不回来。
- `opencode` 和 `qwen` 没有配屏幕匹配——还没人观察过它们的界面，所以这两种会话的状态一直显示 `—`。
- 四家厂商的端点地址是照公开文档写的，**没有用真实账号验证过**。见 `docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md`。

---

# 给开发者

## 大致结构

两个进程，通过 `~/.dct/daemon.sock`（只有属主可访问）收发按行分隔的 JSON：

```
src/ui.rs        界面：视图状态机 + 渲染（ratatui + crossterm）
src/client.rs      |  单条连接，5 秒读超时，一出错就重连
src/daemon.rs    请求分发，一个连接一个线程
src/session.rs   会话生命周期，200ms 一次 tick，从屏幕文本推状态
src/pty.rs       PTY + vt100 屏幕缓冲
src/profile.rs   profile 结构、内置清单、磁盘加载、可用性判定
src/secrets.rs   ~/.dct/secrets.toml，0600，原子替换
src/verify.rs    密钥探测，传输层可注入
src/git.rs       隐藏快照
src/projects.rs  最近项目、上次用的 agent
src/proto.rs     线上契约
```

守护进程活得比界面久。杀掉终端、过会儿再连回来，会话都还在。

**为什么可用性判定在守护进程侧算。** `codex` 在不在 `PATH` 上，要在真正 spawn 子进程的那个环境里回答。界面侧去查的话，可能报「可用」，然后一开就失败。

**为什么 `create()` 期间不持有任何锁。** 开会话要起 PTY、还要 shell 出去跑 git。握着共享锁干这些会把其它客户端全拖住——`src/session.rs` 里有详细版，还有一个回归测试盯着。

**为什么协议传的是已经取好语言的字符串。** `ProfileEntry.label` 这些字段是 `String`，在守护进程侧就按当前语言选好了，不是 `LocalizedText`。用户可见文案在哪儿组句，只有一个答案。

## 构建与测试

```
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

测试会真的建 git 仓库、起子进程、绑 Unix socket，所以串行跑更稳。没有任何测试打真网络，也没有任何测试碰你真实的 `~/.dct`——数据文件的路径都是从 socket 路径推出来的，而测试把 socket 放在临时目录里。

## 约定

- 注释解释**为什么**，不解释是什么。这里的注释密度是刻意的，照着写。
- 每一条用户可见文案都是写给没有编程背景的人看的。不用黑话，不给栈追踪，不露原始的操作系统报错。错误要说出下一步该干什么。
- **不用 emoji 当图标。**
- 界面的按键分支里不要用 `continue`——它会跳过循环末尾清理陈旧消息的那一步，这个坑本仓库已经踩过并修过一次（`e0ba1ec`）。

## 还没做

`ask_human` 与 Bridge；手机通道（Telegram / 飞书 / 企业微信 / SMS）；走 `dc_llm` 的上下文压缩与归类；手机端命令；中文以外的界面语言（profile 结构已经是分语言的，界面文案还不是）。

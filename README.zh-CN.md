<div align="center">

# dct

### 别守着 agent。

coding agent 要你守着，是因为它每隔几分钟就问一句「这样可以吗」。dct 把这些问题全部
自动答应——**底气是每一轮开始之前，它给你的项目拍一张隐藏快照。** 搞砸了按 `u` 退回去。

没人守着，一个人就能同时开好几个：一块看板、一个九宫格，谁在干活谁卡住一眼看得出。
终端关了它们照样跑，出了门用手机看，上课时发一条链接让一屋子人只读旁观。

![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-b7410e?style=flat-square)
![macOS · Linux · Windows](https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows-005f87?style=flat-square)
![version 0.2.24](https://img.shields.io/badge/version-0.2.24-444?style=flat-square)

[English](README.md) · 设计记录在 [`docs/superpowers/specs/`](docs/superpowers/specs/)

新会话先读 [`docs/HANDOFF.md`](docs/HANDOFF.md)。

</div>

```
dct sessions────────────────────────────────────────────────────────
  ┃ 1 ▾ ai-mania          ~/work            claude×1
▶ ┃    3  idle    rewrite the cra…
    2 ▾ dc-terminal       ~/work/dc         claude×1 codex×1
       1  working fix the login b…
       2  idle    port the picker…

────────────────────────────────────────────────────────────────────
q quit         ai-mania         Enter open  n new  Tab project  ? …
```

按项目分组。左边那条竖线标着你在哪个项目里——`n` 就开在那儿。

---

**目录：** [dct 是什么](#dct-是什么) · [哪个版本有什么](#哪个版本有什么) ·
[装上它](#装上它) · [日常使用](#它从来不问这样可以吗) ·
[命令](#命令) · [`dct status --json`](#dct-status---json) ·
[替别的程序做决定](#替别的程序做决定) ·
[`[llm]` 连接](#llm-连接) · [`dct ask`](#dct-ask只读的聊天入口) ·
[语音](#语音dct-voice) · [配置参考](#配置参考) ·
[文件](#dct-下的文件) · [安全与隐私](#安全与隐私) ·
[排错](#排错) · [开发](#开发)

## dct 是什么

dct 是一个终端会话守护进程，前面套了一层终端界面。在代码里：

| 部分 | 位置 | 做什么 |
|---|---|---|
| pty | `src/pty.rs` | 每个会话一个伪终端，带一块 vt100 屏幕缓冲 |
| session | `src/session.rs`、`src/git.rs` | 会话生命周期；每 200 ms 读一次屏幕，判断是在干活 / 空闲 / 已停止 / 出错；隐藏的 git 快照 |
| daemon | `src/daemon.rs`、`src/proto.rs` | 拥有全部会话。界面和每一条 `dct …` 命令都是它的客户端，走本机 socket（`~/.dct/daemon.sock`，一行一个 JSON 对象） |
| ui | `src/ui/` | 看板、格子视图和整屏的会话视图 |

**边界。** 这条线是项目负责人划的，没有任何代码在强制执行。
dct 是人和模型之间的入口：它开会话、让会话活着、把会话展示出来，并把人说的话带给模型、
把模型的追问带回来。它**不带任何业务或应用相关的内容**。**dcv** 管规则和决定；
**dco** 做具体操作（看屏幕、做输入、输出和界面动作）。dct 去调用它们；它们的逻辑不在 dct 里。

- **dcv** 是一个单独的程序，存着用户确认过的东西（规则、流程）。dct 读写它：
  `dct do` 把流程草稿写进去，批准也经由它。
- **dco** 是同一台机器上的另一个单独的程序。dct 通过本机 socket 跟它说话，用于 `dct voice`、
  `dct game`、`dct do`、`dct dco-rules`、看板上的 `dco` 标签，以及 `dct status --json` 里的
  `dco` 块。按 `dct --help` 的说法，需要它的命令只支持 Mac。没有 dco 的话，这些命令不能用，
  其余一切照常。

这棵树里有些命令**跟这条边界冲突**：`dct do` 和 `dct campaign` 带着一张社交平台表，
`dct game` 带着一份 Candy Crush 的配置。因为它们在代码里，所以下面照样写了。
这份 README 没有说它们以后会去哪儿。

---

## 哪个版本有什么

这里有两件事容易混：这个分支的代码做什么，和已发布的 `origin/main` 上有什么。
这份 README 描述的是分支 `dct-ask`，位于 `2c194da`。下面的对比是拿 `git ls-tree origin/main`
跟那个提交比，用的是本地的 `origin/main` 引用（`542c53e`，版本 0.2.24，socket 协议 24）；
写这份文档时没有执行 fetch。

| | `origin/main` | 分支 `dct-ask` |
|---|---|---|
| 看板、格子、attach、install、手机页、Telegram、直播、多电脑、`dct gate`、`dct game`、`dct dco-rules`、`dct llm check` | 有 | 有 |
| 切换会话的键：`F3` = 下一个可用；`F8` = 上一个，是靠得住的键；`Shift+F3` 用户报告在他的终端里**不起作用**；`Ctrl+F3` 被 macOS 抢走 | 有 | 有 |
| `dct voice`（基本转发） | 有 | 有 |
| `[llm]` 没有默认的 `provider` | 没有（默认是 `claude`） | 有 |
| `dct judge`、`[judge]`、`dct judge up / down / status`、`[judge.serve]` | 没有 | 有 |
| `dct answer`、危险闸、模型否决、`dct train record` | 没有 | 有 |
| 看板上的 `dco` 标签和 `dct status --json` 里的 `dco` 块 | 没有 | 有 |
| `dct campaign`、`dct do`、平台表 | 没有 | 有 |
| `scripts/jeff-exam`、`tests/fixtures/jeff-exam`、`tests/fixtures/jeff-dev` | 没有 | 有 |
| **`dct ask`、`[ask]`、socket 协议 25** | 没有 | **有，只在这个分支上**（`integration` 里也没有） |
| **`dct voice` 读 dco 的语音路由、不带转写文字的 `voice.log`、`voice.toml` 的 `log_text`** | 没有 | **有，只在这个分支上** |

从 `[llm]` 到 `scripts/jeff-exam` 之间的几行，本地分支 `integration`（`14326d6`）上也有；
它们没有一条在 `origin/main` 上。自己核对：`git log --oneline origin/main..HEAD`。因为这里的
socket 协议是 25、`origin/main` 是 24，用其中一个版本起的守护进程，对另一个版本来说就是
「过期」的：见[排错](#排错)。

---

## 新东西

只在这个分支上有（见[哪个版本有什么](#哪个版本有什么)）：

- **[`dct ask`](#dct-ask只读的聊天入口)**：stdin 进一行 JSON，stdout 出一行一个 JSON
  事件，给 dco 的对话功能用。你运行 `dct ask --enable` 之前一直是关的；记忆只在守护进程的内存里；
  关掉连接就是取消。
- **[语音不再记你说了什么](#语音dct-voice)**：`voice.log` 默认只留次数和决定；`voice.toml` 里写
  `log_text = true` 才把文字记回来。`dct voice` 还会读 dco 的语音路由，语音走聊天时保持安静。
- **[`dct answer`](#dct-answer一个问题一行-json)**：进一道选择题，出一个 JSON 答案：
  先 Jeff，再 `[llm]` 大模型，最后「要你来」。基于规则的危险闸不管是谁选的，都拦钱和永久删除；
  还可以再加一道可选的模型否决。
- **[dct 替你跑 Jeff](#运行本机-judge)**：`dct judge up | down | status` 按
  `[judge.serve]` 启动和停止本机 judge 服务。
- **[看板上和 `dct status --json` 里的 dco](#dct-status---json)**。
- **[`dct train record`](#dct-train-record存下别人给的答案)**。
- **[发到自媒体平台（Mac，早期）](#发到自媒体平台mac早期)**：`dct do` 经 dco
  发一条批过的文字。还没在真屏幕上试过。

`origin/main` 上也有：

- **[Telegram 会告诉你干完了](#telegram-会告诉你干完了)**：会话停了、出错了、没了，
  都会发一条消息过来；直接回复那条，你说的话就进那个会话。
- **[重启之后会话接得回来](#会话活得比终端久)**：后台服务重新起来时，dct 会问要不要把
  上次开着的会话接回来，能续上的 agent 接着原来那段对话。
- **[出错了，一句话说清](#出错了一句话说清)**：配了 `[llm]` 的话，会话失败时底栏用
  人话说是哪儿坏了。`dct llm check` 能验这条连接通不通。
- **[选项目能搜了](#选项目)**：打字就过滤；翻文件夹有「上一级」那一行；还能新建项目、
  直接粘路径。
- **[不用再按 `g`](#它从来不问这样可以吗)**：不是 git 仓库的文件夹自己变成仓库；
  电脑上没有 git，就帮你装。
- **[多电脑](#多电脑)**：几台电脑上的 agent 互相留言，中转看不到内容。
- **[直播能公开到固定地址](#直播给学生看)**：直播面板里按 `p`，就上了中转的公开列表。
- **[浏览器里的 dct](#浏览器里的-dct)**：给什么都装不了的机器用——一个容器、
  一条不用密码的链接、一个上传面板，外加一个管课堂的后台。
- **[上一个会话的键](#看板)**：`F8` 跳到上一个会话，
  `F3` 跳到下一个（`Shift+F3` / `Ctrl+F3` 靠不住，见下）。
- **[所有命令一张表](#命令)**。
- 看板标题栏和设置页都写着现在跑的是哪一版。

---

## 跟「tmux 里开个 agent」差在哪

让进程活着这一件事，`tmux` 早就做到了，dct 不比它强。差别在另外四件：

**不问权限，因为退得回去。** 每轮对话前一张隐藏的 git 快照，`u` 退回上一张，`d` 看这个
会话到底改了哪些文件。**没有这一条，下面三条都不成立**——你还是得坐在那儿一个个点「允许」，
那样开几个 agent 只会让你更忙。

**状态是读出来的，不是猜的。** 每 200ms 从每个会话的画面上认它在干什么：干活中、空闲、
已停、出错。九个 agent 摆在看板上，哪个卡住了不用一个个切进去看。认不出来的时候写「—」，
不猜——`shell` 会话以前就是被猜成「干活中」的。

**会话属于守护进程，不属于终端。** 关掉终端、断掉 SSH、合上盖子，它们都还在跑。回来敲一下
`dct`，画面停在你离开的地方。

**十种 agent 一个入口。** `dct install claude` 连它要的 Node 一起装好，密钥统一放在
`~/.dct/secrets.toml`——不用记每一家把自己的环境变量叫什么。

代价写在[会踩到的坑](#会踩到的坑)里，**决定用之前先读那一节**，尤其是「权限全自动接受」
这件事在项目目录之外意味着什么。

---

## 装上它

**macOS · Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.sh | sh
```

**Windows** —— 原生，不用 WSL。在 PowerShell 里：

```
irm https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.ps1 | iex
```

这条命令认 Windows 10 和 11，它们自带的 PowerShell 就够用。更老的系统（Win7、
Win8.1）上，安装脚本会先说一句人话再停下，而不是装到一半糊你一屏红字。

装下来的是一个几 MB 的可执行文件。**不需要 Rust，不需要编译器，不需要等编译。**
装完新开一个终端窗口，进到任何一个文件夹里敲 `dct`。

`dct` 每轮对话前会给你的项目拍一张隐藏快照，那是靠 `git` 做的——没有它，撤销就是死的，
而撤销正是 `dct` 敢让 agent 关掉所有权限确认的全部理由。所以 Windows 上如果这台电脑
还没有 git，安装脚本会顺手装一份便携版（45 MB，解压即用，整个躺在 `dct` 自己的目录
下面，不写注册表也不碰系统里已有的东西）。macOS 和 Linux 上 git 通常已经有了，
真没有的话脚本会告诉你该敲哪一条命令。

<details>
<summary>这台电脑够不够</summary>

<br>

**推荐**——统一发机器就按这一列，「十个 agent，一个入口」那件事真按十个开：

| | |
|---|---|
| 系统 | Windows 11 / macOS 14 / Ubuntu 24.04 |
| CPU | 8 核 |
| 内存 | **16 GB** |
| 磁盘 | SSD，空出 20 GB |

**最低**——这是「跑得起来」的下限，**不是给训练营的建议**：同时开两三个
agent 就满了，第四个开始换页。

| | |
|---|---|
| 系统 | Windows 10 1809（内部版本 17763）/ macOS 12 / Ubuntu 22.04 |
| CPU | 双核，x86_64 或 Apple Silicon |
| 内存 | **8 GB** |
| 磁盘 | 空出 5 GB |

内存那一栏是**实测的，不是估的**。本机同时开五个 claude 会话时：每个
`claude.exe` 常驻 250~400 MB（均值 301 MB），守护进程自己 23 MB，每个会话另有
一个十来 MB 的壳进程。算下来 **一个 agent 约 320 MB**。8 GB 的机器扣掉系统和
浏览器，两三个是老实数字；看板上摆满得有 16 GB。

**CPU 不是瓶颈，别照着它买机器。** dct 自己几乎不耗 CPU，agent 大半时间在等
接口回话。真正吃 CPU 的是 agent 让你跑的那些编译和测试——所以核数该按「你这个
项目编译要多久」来定，跟开几个 agent 关系不大。

磁盘的大头也不是 dct（可执行文件 5.5 MB），是它下面那一层。实测这台机器：
Node 运行时 95 MB，`claude` 那个 npm 包自己 416 MB，Windows 上没有 git 的话再加
一份 45 MB 的便携版。装完约 600 MB，剩下的留给项目、git 快照和对话记录——
后者是会长的，本机 `~/.claude` 用一周涨到 185 MB。

**下面几条是硬的，不是「建议」：**

- **Windows 10 1809（内部版本 17763）起。** dct 的伪终端走 ConPTY，那是 1809
  才有的系统 API，代码里没有退回 winpty 的路。而**安装脚本现在只查 PowerShell
  版本**（要 5.0 以上），所以 1607、1703 那种老 Win10 装得上、起不来。买之前
  先 `winver` 看一眼内部版本号。
- **只有 64 位。** 发布的包是 x86_64 和 Apple Silicon 两种，没有 32 位。
- **Windows on ARM 没有原生包**（Surface Pro X 这类）。走系统的 x64 兼容层
  理论上能跑，没测过，别拿它当教室的统一机型。
- **Linux 只发 x86_64，而且 glibc 要 2.35 以上**——包是在 Ubuntu 22.04 上编的。
  Ubuntu 20.04 起不来，报的还是一句跟 dct 毫无关系的动态链接错误。树莓派这类
  ARM 板子没有现成的包。
- **macOS 更老的没测过。** Apple Silicon 天然是 11 起；Intel 那份是在 macOS 14
  的 SDK 上交叉编出来的，构建里没有钉最低版本，所以 12 以下算未知，不算「不行」。
- **git 是必需的，不是可选的。** 每轮对话前的快照靠它，而撤销正是 dct 敢让
  agent 关掉权限确认的全部理由。

**跑模型的那台不算在这张表里。** 上面说的是「开着 dct 指挥 agent」的机器。
模型在网关那头，学生这台**不需要显卡**。

</details>

<details>
<summary>教室的网连不上 GitHub 怎么办</summary>

<br>

把 release 里那几个包和 `SHA256SUMS` 原样放到任何一个学生下得到的地方，然后告诉他们
先设一个环境变量。**学生那条安装命令一个字都不用改**，校验和照样会验。

```sh
export DCT_RELEASE_BASE=https://你的地址
curl -fsSL https://你的地址/install.sh | sh
```

```
$env:DCT_RELEASE_BASE = 'https://你的地址'
irm https://你的地址/install.ps1 | iex
```

**建这个镜像用 `scripts/mirror-sync.sh`**，别手抄：

```sh
./scripts/mirror-sync.sh --out ./mirror     # 拉 latest，验一遍，摆好
./scripts/mirror-sync.sh --tag v0.2.14      # 要某个具体版本
```

它把四个平台包、`SHA256SUMS`、外加 `install.sh` / `install.ps1` 放进同一个目录。
`install.sh` 自己也必须在镜像里——学生连 `raw.githubusercontent.com` 都连不上，
不然第一条命令就卡在那儿了。

校验和在镜像端**也验一遍**，对不上就拒绝上线。学生那端当然也验，但那道检查发生在
几十台机器上、在课堂中间、在你没法调试的时候；镜像端多验这一遍，是把同一个错误提前
到一台机器、一个人、一个能重跑的时刻。

发新版就把同一条命令再跑一遍、把目录再传一次。**产物名字里不带版本号**是 release.yml
特意的设计，所以「发新版」在镜像端等于「覆盖同名文件」，学生那条命令永远不变。

<br>

**放哪儿？** 任何一个能 HTTP GET 到的静态目录都行——对象存储、学校自己的 nginx、
甚至一个 Gitee 仓库。`install.sh` 对下载源没有任何 GitHub 特有的假设，它拼的就是
`$base/<文件名>` 和 `$base/SHA256SUMS`，两个 `curl`。

用 Gitee 的话，**走 raw 路径，不要走「发行版」**：

```sh
export DCT_RELEASE_BASE=https://gitee.com/<你>/<仓库>/raw/main/dist
curl -fsSL https://gitee.com/<你>/<仓库>/raw/main/dist/install.sh | sh
```

Gitee 的发行版附件地址绑定具体 tag，**没有 `latest` 的等价物**（`/releases/latest/download/<名字>`
在 Gitee 上返回的是通用 API 的 JSON 404，根本没这条路由），而整套设计的立足点正是
「固定名字 + latest」。raw 路径没这个问题：它就是一个普通的静态目录。

Windows 上那份便携 git 也一样，用 `DCT_MINGIT_URL` 换地址。

装完之后还有一次下载：`dct` 给自己准备 Node 运行时的时候。那两样国内本来就有现成的
镜像，路径结构跟官方一模一样，所以只要设这两个环境变量：

```sh
export DCT_NODE_BASE=https://npmmirror.com/mirrors/node
export DCT_NPM_REGISTRY=https://registry.npmmirror.com
```

分成两个变量而不是一个「中国模式」开关，是因为它们会分别失效——镜像站可能只镜像了
其中一样，那时候你要能只换一半。真下不到的时候，`dct` 会把这两行原样印在屏幕上。

</details>

<details>
<summary>装到别处、装旧的、以及别拿 <code>cp</code> 覆盖安装</summary>

<br>

Unix 上默认装进 `~/.local/bin`，`--dir` 或者 `DCT_INSTALL_DIR` 换地方；
Windows 上默认是 `%LOCALAPPDATA%\Programs\dct`，换用 `-InstallDir`。
`--build` / `-Build` 是不下现成的、从源码编译（要在 clone 好的仓库里）；
`-NoPath`、`-NoGit` 分别是不动 PATH、不自动装那份便携 git。

**别自己拿 `cp` 往装好的那个文件上覆盖。** macOS 上，守护进程还在执行这个文件的时候
原地覆盖它，内核手里那份代码签名就此对不上，下次敲 `dct` 会在 exec 阶段被杀掉，终端里
只留一行 `zsh: killed`——而 `codesign -v` 还会说签名没问题，因为磁盘上那份确实没问题。
安装脚本是先写新文件再 rename 覆盖，新二进制永远落在一个新 inode 上。Windows 上是同一件事
的另一个形状：那儿不让你写一个正在执行的映像，所以脚本先把老的改名挪开，再把新的搬进来。

装完敲 `dct --version` 能看到装的是哪一版。

</details>

<details>
<summary>Windows 工具链（只有从源码编译才需要）</summary>

<br>

走上面那条命令的话这一段可以整段跳过——预编译包不需要任何工具链。

真要自己编：

```
winget install --id Rustlang.Rustup -e
winget install --id BrechtSanders.WinLibs.POSIX.UCRT -e
rustup default stable-x86_64-pc-windows-gnu
git clone https://github.com/gaolei8888/dc-terminal
cd dc-terminal
scripts\install.cmd -Build
```

**不需要 Visual Studio Build Tools。** WinLibs 是一份解压即用的 mingw，装在用户目录里，
没有几个 GB 也不用管理员。rustup 自带的那份 mingw 唯独少一个 `as.exe`，而 `dlltool` 给
`windows-sys` 这类 crate 生成 import 库时要调它；缺了它编译会停在 `dlltool.exe: CreateProcess`
上——那句话跟真实原因对不上号。安装脚本在动手编译之前就会检查这一条，缺什么直接说。

已经有 MSVC Build Tools 的话，`rustup default stable-x86_64-pc-windows-msvc` 也行，
那条路不需要 `as`。两条路都一样：依赖树里没有任何 C 要编——Windows 上 TLS 走系统自带的
schannel，而不是 `rustls`，后者拖着 `ring`，`ring` 要 `lib.exe`。上面那套工具链自始至终
只在汇编和链接 Rust 自己。发布用的包走的是 msvc 那条路。

`scripts\install.cmd` 那个 `.cmd` 存在的唯一理由，是别让 PowerShell 默认的执行策略用
一句跟 `dct` 毫无关系的报错把安装挡在门外；顺带它还负责按 UTF-8 把 `install.ps1` 读进来，
因为那个文件为了能被 `irm | iex` 吃下去不能带 BOM。不想走 `.cmd` 那层就直接用
`scripts\install.ps1`。

想用 WSL 也行：在发行版里跑 `scripts/install.sh`，跟 Linux 上是同一套；全新的 Ubuntu
先跑一次 `scripts/install-wsl-deps.sh`，把 `cc`、`git`、Rust 补上——那些 `install.sh`
自己不装。

</details>

<details>
<summary>从源码构建，以及跑测试</summary>

<br>

```sh
cargo build --release
./target/release/dct
```

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -- --test-threads=1
cargo fmt --check
cargo clippy --all-targets
```

测试会真的建 git 仓库、真的起子进程、真的绑 socket，所以一个一个跑更稳。没有一个测试碰网络，
也没有一个测试碰你真实的 `~/.dct`，所有数据文件的路径都是从 socket 路径推出来的，
而测试把它指向临时目录。

重新编译过 `dct` 之后，如果旧编译版本的守护进程还在跑，下次启动会发现这一点，跟你说清楚
重启会让正在跑的会话全部结束（文件改动都还在，只是 agent 得重新开），问过你才动手。
答 y 就换掉旧的重连上去；直接回车就还用旧的接着跑。

想主动换、又不想开界面，敲 `dct restart`——版本号没变（同一个 commit 改个字符串
又编了一遍）的时候没人会发现旧的在跑，这条命令就是为那种情况准备的。它同样先问一句，
并且把要被断掉的会话列出来给你看；`dct restart -y` 是不问直接换。本来没东西在跑的话
没有旧的要换、也就没什么要问，它直接给你起一个新的——开看板；在脚本里（没有终端）
就只把后台服务拉起来。

</details>

---

## 它从来不问「这样可以吗」

**正因为你随时能反悔。** 每轮开始前 `dct` 给项目拍一张隐藏快照。正是这一点让「把所有权限
确认关掉」变得安全：agent 不会干到一半停下来等你点头，而它闯的祸，一个键就没了。

快照走 git，但走的是你看不见的那一侧。你的分支、你的暂存区、你的 `git log`，全程干净。

| | |
|---|---|
| `u` | 退回上一张快照 |
| `d` | 这个会话到底改了哪些文件 |
| `s` | 停掉它 |

agent 只在 git 仓库里跑——撤销就是从那儿来的。**这件事不用你操心。** 在一个还不是仓库的
文件夹里开 agent，dct 会先在那儿跑一遍 `git init`。电脑上压根没有 git 的话，它会开一个
会话跑 `dct install git`，让你看着装：Windows 上是往 dct 自己的目录里下一份便携 git，
别的系统会告诉你该敲哪一条命令。装完再选一次 agent 就好。

---

## 会话活得比终端久

第一次跑 `dct` 会拉起一个后台守护进程。**这个守护进程才是产品本体。** 关掉终端窗口、
合上电脑、第二天回来，会话都还在原地跑，`dct` 本身只是重新连上去看一眼。

看板上可以同时挂好几个 agent，各自待在自己的项目目录里，互不打扰。

**要是守护进程自己没了**——重启电脑、崩了、`dct restart`——下一次敲 `dct` 会列出上次
开着的会话，问你要不要接回来。每一条后面写着会得到什么：`继续` 是 agent 接着原来那段
对话（Claude，以及借它跑的那四家都能），`新开` 是在同一个文件夹、用同一个 agent 开一个
新会话。直接回车就从空白看板开始。

---

## 命令

光敲 `dct` 就是打开看板，守护进程没在跑就顺手拉起来。其余的：

**会话和守护进程**

| | |
|---|---|
| `dct ps` | 列出后台在跑的会话 |
| `dct status [--json]` | dct 有没有在跑、有几个会话在忙；`--json` 给别的程序用，见 [`dct status --json`](#dct-status---json) |
| `dct stop <n>…` / `--all` | 停掉会话，留时间让它们收尾 |
| `dct kill <n>…` / `--all` | 直接杀掉会话 |
| `dct prune` | 把已停止的会话从列表里清掉 |
| `dct restart` | 换成当前的二进制作为守护进程（所有会话都会结束；`-y` 跳过确认） |
| `dct daemon` | 只跑守护进程，不开看板 |
| `dct install <name>` | 装一个 agent——`claude`、`codex`、`qwen`、`opencode`——或者 `git` |
| `dct llm check` | 真去调一次你配置里的 `[llm]` 连接，告诉你通不通；见 [`[llm]` 连接](#llm-连接) |
| `dct --version` / `dct --help` | 你装的是哪个版本 / 命令列表 |

**替别的程序做决定**（只在这个分支上）

| | |
|---|---|
| `dct judge [file]` / `--probe` | 用 Jev 格式问 `[judge]` 后端（Jeff 的 `jeff-serve`）一道选择题，打印每个选项的概率；`--probe` 发一道固定的题，看它答不答。退出码 0 答了，1 没配置或后端出错，2 题目有问题 |
| `dct judge up` / `down` / `status [--json]` | 启动、停止（只停 dct 自己起的）或查看本机 judge 服务；见[运行本机 judge](#运行本机-judge) |
| `dct answer --json` | 从 stdin 读一道选择题请求，写一行 JSON；见 [`dct answer`](#dct-answer一个问题一行-json)。退出码 0 答了或要你来，1 配置读不了或无效，2 请求有问题 |
| `dct train record --json` | 把别人答过的一道题记成一行训练数据；见 [`dct train record`](#dct-train-record存下别人给的答案)。退出码 0 记下了，1 写不了，2 请求有问题 |

**对话**（除非另有说明，只在这个分支上）

| | |
|---|---|
| `dct ask` | 给 dco 的对话用：stdin 一行 JSON，stdout 一行一个 JSON 事件；见 [`dct ask`](#dct-ask只读的聊天入口) |
| `dct ask --enable` | 读完你的话会发到哪儿之后，输入 `y`，打开 `[ask]` |
| `dct ask --forget <conversation id>` | 忘掉某一段对话的记忆 |
| `dct voice` | 点 dco 章鱼说话；dct 把它当成你亲手敲的，打进你最近用的那个 agent 会话。只支持 Mac，需要 dco。`origin/main` 上也有，但没有路由和日志的改动；见[语音](#语音dct-voice) |
| `dct dco-rules` | 马上刷新 `~/.dct/dco-rules.json`（dc-vault 规则页读的那个文件）；守护进程启动时、dco 换版本时也会刷新。`origin/main` 上也有 |

**多电脑**（`origin/main` 上也有；见[多电脑](#多电脑)）

| | |
|---|---|
| `dct login` | 用 DC 账号登录；第一台登录的电脑创建组 |
| `dct invite` | 在老电脑上：出一个 6 位数的码，10 分钟内有效，只能用一次 |
| `dct join <code> [--name <computer name>]` | 在新电脑上：用那个码加入 |
| `dct peers` | 组里有哪几台电脑、各开着哪些会话 |
| `dct peers remove <computer name>` | 把一台电脑移出组 |
| `dct send <computer>/<session> "<text>"` | 给另一台电脑上的会话留一句话（会话可以写 `#id`）；会话空着就马上敲进去，忙着就排队 |

**应用相关**（只支持 Mac；它们跟[边界](#dct-是什么)冲突）

| | |
|---|---|
| `dct game play [--game candy-crush] [--steps 20] [--dry-run] [--auto-next] [--tries 5] [--ask-model] [--ask-every-step] [--goal "…"] [--goal-number 2]` | 让 dct 玩 iPhone 镜像里显示的一关三消；见[让 AI 玩三消游戏](#让-ai-玩三消游戏mac) |
| `dct game ask-bench <record file>… [--limit 40]` | 拿录下来的局面问大模型，看它比随机好多少（不碰手机） |
| `dct game identify [--game candy-crush]` | 说出屏幕上现在是哪一类游戏（在本机识别，什么都不上传） |
| `dct game scene [--game <name>] [--steps 10] [--dry-run]` | 找东西类游戏：大模型指一个位置，dct 核对后点下去（需要较新的 dco） |
| `dct campaign approve <name> --param <platform>.<param>=<value>…` | 一次批准某个 campaign 给所有平台用的文字；见[发到自媒体平台](#发到自媒体平台mac早期)。只在这个分支上 |
| `dct do "<goal>" --platform <p> --campaign <name> [--window <id>] [--preview] [--json]` | 经 dco 把某个 campaign 批过的文字发到一个平台。只在这个分支上 |

**浏览器版**（`origin/main` 上也有）

| | |
|---|---|
| `dct gate [--bind <address>] [--port <port>] [--upstream <host:port>] [--url <public address>] [--link]` | [浏览器里的 dct](#浏览器里的-dct) 前面的那道门；`--link` 只打印链接 |

`ps`、`stop`、`kill`、`prune` 绝不拉起守护进程：问「有没有东西在跑」，不该把答案从
「没有」变成「有」。`restart` 会拉起一个，因为你要的就是「后台跑着新的」。`status` 和
`dct ask` 也不会拉起守护进程。

`dct --help` 只打印中文，不管 `DCT_LANG` 是什么。这份 README 是它的英文参考。代码里还认
`dct keys`（dct 的两把签名密钥，存在 Mac 的安全隔区里）和 `dct procedure`（批准流程、签发票据）；
这两个都不在 `dct --help` 里，这里也没有说明。

---

## `dct status --json`

给别的程序轮询用（本来是给菜单栏小工具用的）。不管 dct 处在什么状态，它都退出 0；
只有参数写错才退 2。它从不拉起守护进程。

```json
{"schema_version": 1, "daemon": "up", "dct_version": "0.2.24",
 "sessions": {"busy": 1, "idle": 2, "total": 3}}
```

- `daemon` 是 `up`、`down`（没有守护进程应答）或 `stale`（有守护进程在跑，但说的是另一个
  socket 协议，所以 dct 不去读它）。`down` 只带 `schema_version` 和 `daemon`；`stale` 再加
  `dct_version`；两种都没有 `sessions`。
- `sessions.total` 数的是所有还没停的会话，所以 `busy` + `idle` 可能比 `total` 小
  （在等你的、出错的、读不出来的会话只算在 `total` 里）。
- 缺这个键，意思是「这个版本不知道」，不是零。

`schema_version` 仍是 1。**只在这个分支上：** 本机装了 dco（有 `~/.dco/endpoint.json`）时，
`dct status --json` 会给上面三种形状都多加一个可选的 `dco` 块：只读，取自 dco 的 `status` 工具，
不会拉起 dco，也不会拉起守护进程：

```json
"dco": {"state": "waiting_user", "activity": "…", "runs": 1, "attention": ["needs_help"]}
```

`state` 取 `idle`、`running`、`waiting_user`、`paused`、`halted`、`permission_missing`、`screen_locked`、
`down`（装了但连不上）、`unknown`（回的话读不懂、超过整个读取允许的约 0.8 秒，或报了我们不认识的东西）之一。
没有 `dco` 这个键 = 没装 dco。

`state` 把 dco 自己的活动状态和它的待处理列表合在一起看，取最高的：
`halted` > `permission_missing` > `screen_locked` > `paused` > `waiting_user`
（待处理里有 `dco.needs_help`，哪怕 dco 正在 running）> `running` > `idle`。
`waiting_user` 只认 `dco.needs_help`，别的种类不算。不认识的东西（不认识的活动状态或待处理种类、形状奇怪的列表）
绝不会让状态看起来更健康：本来会是 `idle`/`running` 的变成 `unknown`，已有的告警态保持不变。

`attention` 列出认得的种类，去掉 `dco.` 前缀（`halted`、`permission_missing`、`screen_locked`、`paused`、`needs_help`），
去重、最严重的在前、最多 8 项；不认识的种类不列。什么都没读到时没有这个键。`runs` 只是流程个数，不含内容。
`activity` 只取第一条 `dco.needs_help` 的文字，最多 40 个字符。
**只有 `idle`（待处理为空）在真实的 dco 上见过。** 其余全部——包括种类名和优先级——来自 dco 会话读 dco 源码后的转述，**未验证**。

---

## 替别的程序做决定

这一节的所有内容都**只在这个分支上**。它的作用是让另一个程序（dco、某个脚本）能把一道选择题
交给 dct，拿回一个答案，而不用弹出任何窗口。

本机的小模型叫 **Jeff**。它由 `jeff-serve` 提供，`jeff-serve` 通过 HTTP 回答 Jev 格式的选择题，
并给每个选项返回一个概率。

`dct judge`、`dct answer`，以及 `dct do` 里问 Jeff 那一级，要在 `~/.dct/config.toml` 里写
`[judge]`，指向一个开着的 `jeff-serve`：

```toml
[judge]
base_url = "http://127.0.0.1:8765"
# model = "jeff-latest"      # jeff-serve 认的模型名
# api_key_env = "JEFF_KEY"   # 从这个环境变量读密钥；明文 http 只发往本机
# threshold = 0.8            # Jeff 把握低于这个数，dct answer / dct do 就去问 [llm] 的大模型，再不行问你
# timeout_ms = 5000
```

dct 不下载权重。`jeff-serve` 默认要你自己起（也可以让 dct 起，见
[运行本机 judge](#运行本机-judge)）。目前用的是 Jeff v1.2：
`jeff-legacy/Jeff-Qwen3.5-0.8B`，revision `d86f657`（约 1.6 GB）。服务端说它最多接 254 个选项，
但 dct 自己给 `dct judge` 发 2 到 20 个，给 `dct answer` 发 1 到 20 个。

没写 `[judge]`（或者里面没有 `base_url`），`dct answer` 和 `dct do` 就跳过 Jeff；没写 `[llm]`，
也跳过大模型。最后问的永远是你。

### `dct answer`：一个问题，一行 JSON

`dct answer --json` 从 stdin 读一个请求（最多 1 MiB，一个 JSON 对象，不认识的字段会被拒绝），
写出一行 JSON。

```json
{"state": "屏幕上显示什么、目标是什么",
 "options": [{"key": "a", "text": "发布"}, {"key": "b", "text": "永久删除账号"}],
 "allow": ["jeff", "llm", "user"],
 "forbid": ["一个绝不能发给大模型的字符串"]}
```

- `state`：最多 50 000 个字符。
- `options`：1 到 20 项。`key` 不能为空，最多 64 个字符，不能重复，也不能是 `stuck`；
  `text` 最多 2 000 个字符。
- `picture`（可选）：一个路径，最多 4 096 个字符。它永远不会发给 `[llm]` 大模型。
- `allow`（可选，默认三级都开）：哪几级可以答。它只能去掉某几级，关不掉危险闸。
- `forbid`（可选，最多 100 个非空字符串，每个最多 200 个字符）：只要其中任何一个出现在
  `state` 或某个选项的文字里，这个请求就绝不会发给 `[llm]` 大模型（Jeff 在你本机上跑，
  仍然会被问）；此时 `reason` 以 `llm_withheld: forbidden string in the request` 结尾。

它按这个顺序问，跳过没配置或答不了的那一级：

1. **Jeff**（`[judge]`）。只有把握至少达到 `threshold`（默认 0.8）时，这个选择才算数。dct 用返回
   的概率自己重算把握，不用服务端给的数。
2. **`[llm]` 大模型**，只发文字。跟别处一样适用 `[llm]` 的规矩：没有 `[llm]`，就没有这一级。
3. **你。** dct 自己不去问任何人，也不显示任何东西。它回答 `by: "none"`、`reason: "needs_user"`
   并退出 0；怎么去问由调用方决定。如果 `allow` 里去掉了 `"user"`，原因是 `no_answer`。

不管哪一级选的，这个选择在发出去之前还要过[危险闸](#危险闸dct-answer)和模型否决。

答案：

```json
{"key": "a", "by": "jeff", "confidence": 0.97}
{"key": "a", "by": "model", "confidence": null}
{"key": null, "by": "none", "confidence": null, "reason": "needs_user"}
{"key": null, "by": "none", "confidence": 0.991, "reason": "danger", "gated": {"key": "b", "tier": "critical"}}
```

`by` 是 `jeff`、`model` 或 `none`。`confidence` 是 Jeff 的，或者 `null`。大模型对一个没有文字的
图标给出的选择，可能还带一个 `name`。退出码：0 答了或要你来（包括 `danger`），1 配置读不了或
无效（`[judge] gate_tiers` 写坏了也算），2 请求有问题。某一级配置了但答不了（连不上、太慢、
回复读不懂）会被跳过，绝不会让命令失败。

### 运行本机 judge

写上 `[judge.serve]`，dct 就替你启动、停止 judge 服务。「怎么起」全部来自这一段，
dct 里没有内置任何路径或程序名。不写这一段，dct 永远不会启动或停止任何东西。

```toml
[judge]
base_url = "http://127.0.0.1:8796"        # 必须是本机

[judge.serve]
command = ["<judge 服务程序的路径>"]        # argv 数组，不经 shell
env = { PORT = "8796" }                   # 额外的环境变量，只能是字符串
# ready_timeout_secs = 60                 # 等 GET <base_url>/health 的上限
# autostart = false                       # true：连不上时 `dct answer` 自动起
# min_free_percent = 15                   # 空闲内存低于这个百分比就不起
```

| 命令 | 做什么 |
|---|---|
| `dct judge up` | `/health` 已经在答就只说一声、不起；否则起，最多等 `ready_timeout_secs`。退出码 0 就绪，1 没起来 |
| `dct judge down` | **只停 dct 自己起的那个进程**：先 SIGTERM，5 秒后 SIGKILL。退出码 0 已停或没有要停的，1 拒绝 |
| `dct judge status [--json]` | 健不健康、是不是 dct 起的、进程号、已运行时间、内存、日志位置、上次启动的错误。退出码 0 健康，1 不健康 |

`autostart = true` 时，`dct answer` 第一次发现 judge 连不上，会起一次（stderr 一行提示，
最多等 `ready_timeout_secs`），然后重试一次。起不来或超时，`dct answer` 照旧往下走：
下一级，再不行「要你」。`autostart` 不是 `true` 时绝不起任何东西。

安全：

- **只认本机。** `base_url` 不是 `127.0.0.1` / `localhost` / `::1`，dct 拒绝启动和停止。
- **只动自己的进程。** `up` 把进程号、启动时间和该进程的命令行记在 `~/.dct/judge-serve.pid`。
  `down` 先核对这个进程还活着、命令行还对得上，对不上就不发信号。不是 dct 起的服务永远不会被停。
  两个 `dct` 同时要起，只会起一个。
- **内存。** 空闲内存低于 `min_free_percent`（macOS 用 `memory_pressure`，Linux 读
  `/proc/meminfo`）就不起，并说明原因。别的系统读不到内存，这道检查跳过；那里 `down` 也读不到命令行，
  于是什么都不停（Windows 暂不支持）。
- **`command` 和 `env` 里不要放密钥。** 本机任何用户用 `ps` 都能看到。
- **日志：** `~/.dct/judge-serve.log`（权限 0600，每次启动重新写）。
- 还没做：`idle_stop_minutes`（空闲一段时间后自动停）没有实现，写了会被忽略。

### 危险闸（`dct answer`）

不管是谁选的（Jeff、`[llm]` 大模型），`dct answer` 放行之前都会用 `dct-brain` 的档位规则
看一眼被选中那个选项的字。这是按规则拦，从不看把握：Jeff 对「Delete account permanently」
有 0.99 的把握，和只有 0.5 一样被拦。Jeff 仍然看到全部选项，闸只管答案。选项开头的角色前缀
（`按钮 `、`文字 `）先剥掉，和考试脚本判档的方式一致。

**默认只拦 money 和 critical（永久删除、清除、恢复出厂、授权）；普通的删除/移除/发送/发布/保存照常放行；
要连这些也拦，在 `gate_tiers` 里加上 `"content"`。** 档位有：**critical**（不可逆或授权：永久删账号、
抹掉、恢复出厂、Allow……）、**money**（购买、支付……），以及要自己打开的 **content**（发出去、发布、保存、删内容）。
被拦的答案长这样：

```json
{"key": null, "by": "none", "confidence": 0.991, "reason": "danger",
 "gated": {"key": "b", "tier": "critical"}}
```

`reason` 多了一个值 `danger`（和 `needs_user` / `no_answer` 并列），退出码和别的「要你来」一样是 0，
`gated.key` 是被选中的那个 key。JSON 里永远不带选项的字。滚动、等一会儿这类固定动作的字属于 Read 档，照常放行。
请求里的 `allow` 只决定哪几级可以答，关不掉闸，也没有逐次关闭的办法。

```toml
[judge]
# gate_tiers = ["critical", "money"]              # 没写这一项时的默认
# gate_tiers = ["content", "critical", "money"]   # 自己打开：content 档的选择也拦
```

名字不分大小写。`critical` 和 `money` 永远去不掉：少了任何一个，或者写了不认识的档位，
`dct answer` 会说明原因并以 1 退出。只有 `content` 是可选的，不写就不拦。

**模型否决（配了 `[llm]` 就开）。** 规则放行一个选择之后，`dct answer` 会就被选中那个选项的文字
（不带屏幕，不带目标）问 `[llm]` 大模型一个固定的问题：它是不是要花钱、转钱，或者永久删除数据？
大模型只能加一票否决（`gated.source` 是 `"model"`，档位 `money` 或 `critical`）；规则拦下的东西，
它绝不会放行。大模型连不上、超时或出错时，这个选择放行，并往 stderr 写一行；如果它的回答
拿不准或读不懂，这个选择就交给你（`needs_user`，`"veto": "unsure"`）。`allow` 里去掉了 `"llm"`
的请求，或者文字命中 `forbid` 的请求，绝不会发给大模型。关掉它：

```toml
[judge]
model_veto = false   # 这一段不需要写 Jeff 的地址
```

**说在前面：闸只认 dct-brain 的词表，它不认识的词不会被拦；调用方知道有害的选项，仍应自己先筛掉。**
（目前认不出的例子：「Cancel subscription」「Clear all」「Block」「Deactivate」；开了 content 档以后，也会多拦一些无害的，比如
「Save」「Done」「Share」「Sign in」「Upload」。）

### `dct train record`：存下别人给的答案

`dct train record --json` 从 stdin 读一个请求，往 `~/.dct/explore/answers.jsonl`（权限 0600）追加
一行训练数据（版本 2）。它给那些问过别人一个问题的调用方用，好让这个答案以后能训练 Jeff。
别的什么都不写。

```json
{"question_id": "q-17", "by": "user", "key": "b", "outcome": "good",
 "run_outcome": "published",
 "corrects_jeff": false, "jeff_asked": true,
 "shown": [{"key": "a", "text": "…"}, {"key": "b", "text": "…"}],
 "step": 3, "state": "…", "forbid": ["a string that must not be recorded"]}
```

- 必填：`question_id`、`by`（`jeff` | `model` | `user`）、`key`、`outcome`
  （`good` | `bad` | `unknown`）和 `run_outcome`（`published` | `declined` |
  `halted` | `stopped`）。其余可选：`corrects_jeff`、`jeff_asked`、
  `shown`（最多 254 项）、`step`、`state`、`forbid`。
- **调用方必须先把用户自己的值换成 `{name}` 占位符。** `state`、`key` 或 `shown` 的文字里
  只要含有任何一个 `forbid` 字符串，请求就会被拒绝，不存。
- 回复里的 `label` 说明这一行算不算训练标签：用户的答案一定算；大模型的答案只有在
  `outcome` 是 `good` 时才算；Jeff 的答案从不算。
- 回答 `{"recorded": true, "label": true|false}`。退出码 0 记下了，1 文件写不了，
  2 请求有问题。

---

## `[llm]` 连接

`~/.dct/config.toml` 里的 `[llm]` 是 dct 自己用的那条通用大模型连接。**你不写这一段，它就是关着的。**
没有 `[llm]`（或者配置文件解析不了）时，什么都不会发到任何地方；配置写坏了，也不会让 dct 起不来。

用到它的地方：给会话起名、[解释失败](#出错了一句话说清)、`dct answer` 的第二级和它的模型否决、
`dct do`、`dct game play --ask-model`，以及（通过 `[ask]`）`dct ask`。

| 键 | 默认 | 含义 |
|---|---|---|
| `provider` | 无 | 一个 profile 的名字（内置的，或者你在 `~/.dct/profiles/` 里写的）。**没有默认值**：dct 从不替你挑厂商；不写它，这些功能就保持关闭，并给你一行提示 |
| `transport` | `"cli"` | `"cli"` 运行那个 profile 自己的无界面命令；`"http"` 直接调 profile 的 `[api]` 段里的端点 |
| `model` | 无 | `http` 必填；dct 不会替你猜一个 |
| `base_url` | profile 的 `[api].base_url` | 可选的覆盖 |

用 OpenAI 兼容服务器的例子在[出错了，一句话说清](#出错了一句话说清)那一节。
`dct llm check` 真的调一次，告诉你通不通。

- **DC profile。** 内置的 `dc` profile 指向营地网关 `https://dc-llm.tzspace.cn/v1`（OpenAI 的
  传输格式）。配对会把密钥写进 `secrets.toml`，把模型名写进 `pair-models.toml`。ask 的设计文档
  记录说，那个网关上的聊天由一台 Ollama 服务器上的 `qwen3.8:27b` 回答（见其 3.7 节，
  2026-10-16/17 测的，之后没有重新核对）。如果你原来的 `[llm]` 写的是 `provider = "dc"` 加一个
  Claude 模型，请改成 `provider = "qwen"`、`transport = "http"` 和一个你账号有的模型。
  dct 从不改写已有的 `[llm]`。
- **除非你另写，只用你自己的网关。** 代码里没有任何一处会自己往外部顶级大模型发东西，也没有
  这样的开关。`dct ask` 只接受 host 在 `[ask] allowed_hosts`（默认 `dc-llm.tzspace.cn`）里的
  `http` 型 `[llm]`。`[llm]` 本身也允许把厂商的命令行当 provider（`claude`、`codex`……），
  那会跟那家厂商通信；只有你自己这么写了才会发生。
- **凭据。** 你在 dct 里保存的密钥，只用于你保存时对应的那个 provider。从别的程序借来的登录
  （Claude Code、Codex），不管 profile 或 `base_url` 怎么写，都只会发回那个程序自己的厂商域名。
- 配对页的勾选框（「让 AI 解释你看不懂的报错」）只有你保持勾选、并在浏览器里批准之后才会追加
  `[llm]`，而且从不改动已有的 `[llm]`。

---

## `dct ask`：只读的聊天入口

**只在这个分支上（`dct-ask`，连 `integration` 上都没有）。** `dct ask` 让 dco 能跟 `[llm]` 背后的大模型
进行一段语音或文字对话。它只是聊天：回复会被显示或念出来，绝不会被执行。ask 服务看不到你的会话，
不能往会话里打字，也没有任何工具。

**自己动手打开一次：**

1. 先有一个能用的 `[llm]`，`transport = "http"`，端点是 OpenAI 兼容的
   （见 [`[llm]` 连接](#llm-连接)）。
2. 在终端里运行 `dct ask --enable`。它会告诉你你的话会发到哪个 host、发什么、不发什么，
   你不输入 `y` 就什么都不写。它没有 `--yes`，stdin 或 stdout 不是终端时会拒绝（退出码 2）。
   它把下面这段追加到 `config.toml`（不会重写这个文件，已有的 `[ask]` 段，不管开着还是关着，
   都原样不动）：

   ```toml
   [ask]
   enabled = true
   allowed_hosts = ["dc-llm.tzspace.cn"]
   ```

   `dct ask --enable` 的退出码：0 已打开（或本来就开着），1 没打开（你没输入 `y`、已经有 `[ask]`
   段，或者文件写不了），2 不是终端，11 `[llm]` 不能用于 `ask`。

3. 装上这个版本之后，运行一次 `dct restart`。旧版本起的守护进程不认识 `Ask` 请求（协议 25），
   `dct ask` 会回答 `daemon_down`，带 `reason: "protocol"`。`dct restart` 会结束所有在跑的会话，
   所以挑个合适的时候。

`[llm]` 的同意只覆盖终端报错的文字；你说的话需要它自己的开关，这就是 `[ask]` 存在的原因。
`[ask]` 每次请求都重新读，所以打开或关掉都不用重启。

**怎么调用。** 文字走 stdin，绝不走命令行（命令行参数通过 `ps` 对每个用户都可见）：

```sh
echo '{"v":1,"text":"What is the weather like?","source":"voice","conversation_id":"c-7f3a"}' | dct ask
```

| 字段 | 规则 |
|---|---|
| `v` | 可选；如果写了，必须是 `1` |
| `text` | 必填，控制字符换成空格后最多 500 个字符；更长的会被拒绝，不会被截断 |
| `source` | `"voice"` 或 `"typed"`；只是给大模型的提示（语音可能听错，回复会被念出来）。**它绝不算授权** |
| `conversation_id` | `[A-Za-z0-9_-]`，1 到 64 个字符；记忆按 id 分开存 |
| `persona` | 可选，最多 300 个字符，放在固定规则之下，不能覆盖它们 |
| `timeout_ms` | 可选，默认 8000，限制在 1000–30000 之间 |

stdin 最多读 16 KiB。stdout 上一行回一个事件，每个事件都带 `"v": 1`：

```jsonl
{"v":1,"type":"start","conversation_id":"c-7f3a","turn":3}
{"v":1,"type":"sentence","i":0,"text":"It is sunny."}
{"v":1,"type":"end","sentences":1,"truncated":false,"first_ms":1840,"total_ms":2100}
```

每次调用都恰好以一个 `end` 或 `error` 结束（如果你把连接关了，就什么都没有）。被拒绝的请求
（输入有问题、没开启、忙）只有一个 `error`，没有 `start`。如果大模型挂了或超时的时候已经发出了
一些句子，你收到的是带 `"truncated": true` 的 `end`，不是 `error`。`error` 带着 `code`、`stage`、
`retryable`、`after_ms` 和一个固定的 `reason` 词（绝不含你的任何文字）。`dct ask` 不往 stderr 写任何东西。

| 退出码 | `code` | 含义 |
|---|---|---|
| 0 | （`end`） | 完成，包括 `truncated` |
| 2 | `bad_request` | 输入有问题；`reason` 是 `not_json`、`bad_version`、`empty_input`、`too_long`、`bad_source`、`bad_conversation_id`、`persona_too_long`、`usage` 之一 |
| 10 | `daemon_down` | 没有守护进程应答（**不会**替你拉起），或者是旧的守护进程：`reason` 是 `socket`、`closed` 或 `protocol` |
| 11 | `not_enabled` | `reason` 是 `ask_off`、`llm_missing`、`llm_not_http`、`llm_unusable` 或 `host_not_allowed` |
| 12 | `unreachable` | 连不上网关（DNS、被拒绝、TLS、连接超时） |
| 13 | `auth` | 网关回了 401/403；重试没用 |
| 14 | `busy` | 网关回了 429/503，或者已经有另一段对话在进行（`reason` 是 `in_flight`；一次只能一个） |
| 15 | `slow` | 到期限都没有一个句子 |
| 16 | `empty` | 流结束了，却没有任何可说的内容 |
| 17 | `bad_reply` | 回复读不懂 |
| 18 | `cancelled` | 同一段对话里更新的一次 ask 取代了这一次 |

**取消就是关掉连接。** dco 杀掉 `dct ask` 进程；socket 关闭；守护进程马上停止生成，并释放
进行中的那个名额（网关可能还会再生成一会儿，最多 `timeout_ms`）。在 Windows 上，守护进程只能靠
写入失败来察觉，这比较弱；dco 目前只支持 Mac。

**它记住什么。** 每段对话最近 6 轮，最多 3 000 个字符，同时 4 段对话，闲置 30 分钟后丢掉。
它存在守护进程的内存里，**从不写到磁盘上**。`dct ask --forget <conversation id>` 丢掉一段。
重启守护进程会丢掉全部。

**什么会离开这台机器。** 这一轮的文字、最多 6 轮历史、一段固定的系统提示和你可选的 `persona`，
只发给你 `[llm]` 的那个 host。没有屏幕内容，没有文件，没有会话列表。请求里没有工具，
`max_tokens` 是 200，没有 thinking 字段。

**它写什么。** 每次 ask 在 `~/.dct/ask.log`（权限 0600，超过 256 KiB 就整个换掉）里写一行元数据：
`t`、`source`、`conv`（对话 id 的哈希的前 4 个十六进制字符，不是 id 本身）、`chars_in`、
`chars_out`、`first_ms`、`total_ms`、`outcome`。没有你或大模型说的话的文字。被当作 `bad_request`
拒绝的请求不记录。

守护进程里对同一个网关的其他用法（错误解释、会话起名、手机页），在 ask 进行时最多等 20 秒，
然后放弃；`dct answer`、`dct do`、`dct game` 和 `dct llm check` 跑在别的进程里，不用等。

设计、测量数据和未完成清单：[`docs/2026-10-16-dct-ask-design.md`](docs/2026-10-16-dct-ask-design.md)。
没有验证过：网关还在预填充时取消、TLS 关闭时的行为、负载下的行为，以及对着真网关跑一遍
（那份文档的第 16 节说，真网关的冒烟测试没有做）。

---

## 语音：`dct voice`

需要 Mac 上有 dco。`dct voice` 在前台运行（`Ctrl-C` 退出），听 dco 听到的东西。你点了 dco 章鱼之后
说的每一句话，会被打进你最近用的那个 agent 会话，就像你自己敲进去再按了回车一样。
你启动它之前说的话会被跳过。

**语音从来不算授权。** 只有下面这些条件全部满足，一句话才会被发出去：

- dco 说它是由点击章鱼触发的（`octopus_click`，唯一接受的触发方式：一个听起来像你的声音，
  也可能是电视）；
- 把握至少 0.5；
- 不只是一句光秃秃的确认（「好」「嗯」「ok」「yes」……）；
- 目标会话不是在等一个批准提示（语音答不了这些），也不是普通的 shell 会话；
- 距离上一句至少 2 秒；
- dct 宣布要发之后的 2 秒窗口里，你没有说「取消」「算了」「别发」或 "cancel"。
  窗口结束时会再检查一次会话的状态。

发出去的文字前面会加上 `（语音输入，可能有识别错误）`，好让 agent 宽容一点。说「给 X 说…」「对 X 说…」
或「告诉 X…」（X 是某个会话的名字或 `#id`）来指定一个会话；匹配到两个，就不发。

**语音路由**（只在这个分支上）。每句话之前，dct 都从 dco 的 `status` 里读
`converse.voice_route`，每次都重新读。`chat`：dct 什么都不发，并且只说一次（由 dco 自己的对话来
处理这句话，它会调用 `dct ask`）。`agent`，或者旧版 dco 没有 `converse` 块：照旧发送。读不出来的
路由，或者值不认识：照旧发送，并且只说一次；代价是，路由读不出来时，本来要说给聊天的话可能
会落进某个 agent 的终端。

**`~/.dct/voice.toml`**（可选）：

```toml
log_text = false          # true：也把说的话记进 voice.log
[alias]
章鱼 = ["张瑜", "张宇"]   # 左边是对的词 = 右边是识别器常常写成的错词
```

没有这个文件时，别名表用的是内置的那份（给「章鱼」的）。`log_text` 类型写错是个错误，
绝不会悄悄当成「开」或「关」。

**`~/.dct/voice.log`**（权限 0600，一行一个 JSON）默认只存元数据：`t`、`trigger`、
`session`（编号，不是名字）、`chars_raw`、`chars_fixed`、`confidence`、`route`、`action`、
`reason`。`log_text = true` 时还有 `text_raw` 和 `text_fixed`，**除了**因为路由是 `chat` 而被跳过的
句子，那些从不带文字。旧版本写下的行可能含有你说的话；这个版本不会去清理它们。

---

## 十个 agent，一个入口

按 `N` 会把十个全列出来，**包括这台机器上跑不了的**。跑不了的置灰并写清楚为什么，
而且选中它是带你去解决，不是把你打发走：

- 没装的，`dct` 开一个会话把它装上，你看着它装。**这台电脑上没有 Node 也不要紧**——
  那几个 agent 都是 npm 装出来的，所以 `dct` 会先给自己下一份 Node 放在 `~/.dct/runtime`，
  只给它自己用，不进系统 PATH、不碰你可能已经装好的那份
- 没填密钥的，弹输入框，旁边给申领页面的地址，`Ctrl+O` 直接打开
- 密钥存盘之前先拿真端点探一下，少复制了一截当场就知道，不用等十秒钟进到会话里对着满屏英文发懵

| | | 需要 |
|---|---|---|
| DC | 训练营自己的网关，默认运行 Qwen Code | `qwen` + 一份密钥 |
| Claude | Anthropic 官方命令行 | `claude` |
| Codex | OpenAI 官方命令行 | `codex` |
| OpenCode | 开源，能接不少模型 | `opencode` |
| Qwen Code | 阿里通义，独立命令行 | `qwen` |
| Kimi | 月之暗面，套着 Claude 的壳 | `claude` + 一份密钥 |
| GLM | 智谱，同样的路子 | `claude` + 一份密钥 |
| DeepSeek | 同样的路子 | `claude` + 一份密钥 |
| Qwen API | 同样的路子 | `claude` + 一份密钥 |
| 命令行 | 就是个 shell | |

DC 现在默认运行 Qwen Code。旧版自动配对保存的 Qwen 模型会继续使用；自定义
`profiles/dc.toml` 仍优先于内置配置。如果之前的 `[llm]` 使用 `provider = "dc"`
和 Claude 模型，请将它改为 `provider = "qwen"`、`transport = "http"`，并把
`model` 改为账号可用的 OpenAI 兼容模型；已有 `[llm]` 不会自动改写。

后面那四个（Kimi、GLM、DeepSeek、Qwen API）不是独立程序，是把 `claude` 的地址指到
另一个 Anthropic 兼容端点上，所以它们既要那个二进制，又要一份自己的密钥。DC 是同一
个路子的另一侧：它跑的是 `qwen`，指向训练营网关的 OpenAI 兼容接口。换一种方言是因为
网关背后那批模型说的就是这一种，Claude Code 说不了。

**DC 不用去哪儿复制密钥**：选中它、这台机器还没配过密钥，回车直接进配对——浏览器里
弹出训练营网关的确认页，点一下同意，密钥和模型名就自动写回来了，不用打开任何页面复制
粘贴。手动贴一份密钥的老路仍在，配对屏的任意一步按 `p` 都能切过去。配对屏上还有一个
默认勾着的框——「报错看不懂时让 AI 解释」，那行字写的是代价不是好处：打开它意味着终端
上的报错原文会被发给训练营网关。在浏览器里点确认之前按 `l` 就能取消勾选；如果
`~/.dct/config.toml` 里已经写过 `[llm]`，配对会说一声，并且一个字都不改你自己那份配置。它排在第一位就是
为了这件事——对第一天上课的学生来说，这一整张表里只有这一行不需要注册、不需要银行卡、
不需要翻墙。不上课的人当它不存在就行，或者把它指到自己的地址上：它就是一个普通
profile，在 `~/.dct/profiles/` 里放一份自己的 `dc.toml` 就覆盖掉了。

密钥存在 `~/.dct/secrets.toml`，权限 0600。它们绝不会写进 profile 文件，那些文件是要能
随手拷到另一台机器、或者直接发给同事的。

<details>
<summary>加自己的 agent，不用改代码</summary>

<br>

往 `~/.dct/profiles/` 丢个 TOML 就行。不用重新编译，也不用重启，那个目录每次请求都重读一遍。
名字跟内置的撞了就以你的为准，是新名字就加进列表。

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

`command` 里记得带上这个 agent 自己的权限绕过参数，不然它照样停下来问你。
`is_agent = true` 才有快照和撤销，不是真 agent 的东西就别开。

两个 pattern 字段是看板判断忙闲用的。`busy_pattern` 匹配「正在干活」时屏幕上的东西，
`idle_pattern` 反过来。能用前者就用前者：「按 esc 中断」这种提示是稳的，而输入框里的
占位符，用户一打字就没了。两个都不填也行，看板会显示 `—`。这是故意的，编一个假状态
比承认不知道更糟。

另外还有 `env` 写环境变量，`secret` 声明这个 agent 要用户给一份密钥，`install` 写怎么安装。
TOML 写错了选择器会告诉你是哪个文件哪一行。

</details>

<details>
<summary>统一发机器：让菜单少于十项</summary>

<br>

十个选项摆在第一天上课的人面前太多了，而且那里面大半行都是在要求他自己去某个网站注册。
统一给一个班、一个团队装机的话，在 `~/.dct/config.toml` 里点名要哪几个：

```toml
[menu]
agents = ["dc", "shell"]
```

写下的顺序就是显示的顺序。不写这一段——绝大多数人都不写——就一项不裁。名字对不上任何
profile 就跳过；万一一个都对不上，回到完整清单，而不是给用户一个什么都开不了的空菜单。

这一段每次请求都重读，所以改完文件就生效，不用重启，也不会有人为了一次菜单改动
把手上正在跑的会话全断掉。

</details>

---

## 看板

同一个项目的会话聚在一起，组头一行写着这个项目在用哪些 agent（`claude×2 codex×1` 这样）、
有没有出错。

| | |
|---|---|
| `Tab` `Shift+Tab` | 换项目，一步到位 |
| `1`…`9` | 直达第 N 个项目 |
| `n` | 新建会话，用这个项目上次那个 agent |
| `N` | 新建会话，自己选 agent |
| `p` | 把一个新项目摆上看板——并且直接开工 |
| `x` | 把一个还没有会话的项目从看板上拿掉 |
| `←` `→` `空格` | 折叠 / 展开当前项目 |
| `Enter` | 进会话 |
| `g` | 九宫格：所有会话的实时画面摊在一屏上 |
| `c` | 管密钥 |
| `l` | 设置 |
| `?` | 全部按键 |
| `q` | 退出看板，会话继续跑 |

**每个项目各自记着上次用的 agent**：在 A 项目按 `n` 开 claude、在 B 项目按 `n` 开 codex，
底栏在你按之前就写着这一下会开哪个（`n 新建 claude`）。

`p` 是你唯一一次明确说出「我要去那个项目」的地方，所以它接着问你用哪个 agent 并开好会话。
`Tab` 和数字键只是把光标挪过去。

标题栏最右边写着现在跑的版本，设置页也有——来帮你排查的人第一句就会问这个。

装了 dco 的话，版本号左边多一小格只读的标签，写着它在干什么（`dco 空闲`、`dco 等你处理`、`dco 已暂停`、`dco 已急停`、`dco 缺权限`、`dco 屏幕已锁` ……）。
后台每隔几秒刷新一次；没装 dco 就不显示；屏幕窄的时候它比别的内容先让出位置。

### 选项目

按 `p` 出来的是你用过的项目，最近的在上面，顶上一个搜索框：一打字列表就收窄。列表下面钉着
三行，列表再长也不会被挤出屏幕：

- **翻文件夹**——从当前位置往下找。最上面那行是回到上一级，`←` 也是；退上去之后光标停在
  你刚出来的那个文件夹上，不会跳回顶上。「就用这个文件夹」那一行选的是你现在站着的地方。
- **新建项目**——名字预先填好你在搜索框里打的字，因为找不到项目的时候，正是最想新建一个的时候。
- **输入路径**——手里已经有一条路径，直接粘进来。

<details>
<summary>底栏、九宫格，以及会话里给你留了什么</summary>

<br>

底栏只有一行，装不下的键不会跟着窗口宽度忽隐忽现——挤不进去的那些全在 `?` 后面那一屏，
而 `? …` 这扇门永远在行尾。那一屏只列**此刻真的按得动**的键：只有一个项目时不写 `Tab`，
组里还有会话时不写 `x`。

底栏中段是当前项目，从底栏自己的配色里反白出来，所以它不会被当成又一个键名。

格子只读——方向键移动焦点，`F3` 跟 `→` 效果一样（下一格，停掉的会话也算），`F8`（还有 `Shift+F3` / `Ctrl+F3`，但 `Shift+F3` 用户报告在他的终端里不起作用，`Ctrl+F3` 被 macOS 抢走，见下）跟 `←` 一样，`Enter` 放大进
焦点那一格，`g` 回列表，`Tab`/`1`…`9`/`n`/`N`/`p`/`x`/`c`/`l`/`s`/`u`/`d`/`q` 在格子里的
效果跟在看板上完全一样。只有两处不同：折叠是列表独有的，因为九宫格的左右键是移动焦点；
数字键在这里屏幕上看不到号码——格子不像组头那样印着序号，但按下去照样是「第 N 个项目」。

`i` 是九宫格独有的那个键，也是看板上没有对应物的一件事：它在焦点那一格上开一个单行回复框，
让你不用离开总览就回 agent 一句。打完按 `Enter` 送出去。框还空着时直接按 `Enter`，送出去的
就是一个光秃秃的回车——批准一份计划、或者说「接着干」，用的就是这一下。`Ctrl+C` 则是打断它。
框开着的时候键盘整个归框。

格子按项目排好，同一个项目的会话挨在一起，每一格标着自己属于哪个项目。格子里敲的键一个字
都不会送到 agent 那边。停掉的会话格子里留一张最后的画面，不是空的。会话超过九个就翻页。

进了会话，你敲的每个键都送给 agent，`Esc` 也一样，它们关自己的弹窗要用。`F2`…`F6` 是 `dct`
唯一留下的几个键：`F2` 退回看板，`F3` 直接跳到下一个还在跑的会话，`F8` 跳到上一个（`Shift+F3` 和 `Ctrl+F3` 也一样，但见下），`F4` 切换复制模式，
`F5` 粘贴图片，`F6` 挑配色。底栏左边那句「F2 回看板」一直在，断连、报错、消息再长都挤不掉
它——那是会话里唯一的出口。

上一个/下一个按看板上的顺序（先项目、再 id），到头回绕，跳过已停止的会话。「上一个」没写在底栏
（放不下）；`Alt+F3` 仍然是下一个。

**`F8` 是靠得住的「上一个」键。** `Ctrl+F3` 在到达终端之前就被 macOS（Dock）抢走了。`Shift+F3`
在代码里、也有单元测试覆盖，但**有人报告说它在至少一个真实终端里不起作用，原因还在诊断中**：
别指望它。有的终端还会把功能键的修饰键剥掉，那时 `F3` 就只是下一个。想知道你的终端到底发了什么，
运行 `cat -v` 再按那个键：`^[[1;2R` 是 `Shift+F3`，`^[OR` 或 `^[[13~` 是单按 `F3`，`^[[19~` 是 `F8`。
（以前任何修饰键 + `F3` 都是下一个，现在 `Shift`/`Ctrl` + `F3` 是上一个。）

会话里之前打印过的内容能往回翻，用 `PageUp`/`PageDown`/`End`。`dct` 大概留着最后 2000 行
滚出屏幕的内容，这是个上限，不是承诺一行不少。翻页是一屏减两行，留出两行接上上一屏，
`End` 直接跳回底部。往回看的时候新内容照样在后面涨，画面不会被拽着往下跳，底栏会数有几行
新的在等着。打一个字，或者改一下窗口大小，立刻弹回底部。

一个会话从生到死绑定一个 agent，中途换不了。整段对话都在那个进程里面。想换就按 `N` 另开一个。

</details>

---

## 会话会自己长出名字

同一个项目挂三个 `claude` 会话，以前不管在哪儿写的都是一样的 `3 claude`、`5 claude`、
`7 claude`——按 Enter 之前你唯一能凭的就是那个数字。

现在守护进程自己把这件事解决了：agent 会话第一次干完一轮活，它会把你说的话和屏幕上的内容
交给 `[llm]` 那段配置的模型，问一个短名字。`3 claude` 就变成了 `3 修登录白屏`，
而且只起这一次，一辈子不会再改。名字用的是**你当时打字那句话的语言**，不是界面语言。

它会出现在会话列表、九宫格的格子标题、回复框的收件人这几处。这一版没有手动改名的入口。

---

## 出错了，一句话说清

会话失败时，底栏会用一句你能照着做的话说是哪儿坏了，不用你自己去啃一屏别人的报错。问的
是起名字那个 `[llm]` 模型，而且只送**失败画面的末尾**——错误就在那儿。

**不自己写一段 `[llm]`，这个功能就是关着的。** 它不是一个别人能替你打开的默认值，因为
失败画面的末尾恰恰是 `Invalid API key: sk-…`、`Authorization: Bearer …`、`.env` 内容最常
出现的地方。DC 的配对屏会提出帮你加上这一段，是一个勾好的框，按 `l` 取消；它只往你的
`config.toml` 后面追加、不重写，已经有 `[llm]` 的话就不动。

`[llm]` 必须写明用哪个后端：**`provider` 没有默认值**，dct 不会替你挑一家厂商。不写的话
功能保持关着，只会给你一句该加什么的提示。`provider` 是一个 profile 的名字（内置的，或你
自己放在 `~/.dct/profiles/` 里的）。`transport = "cli"`（默认）会跑那个 profile 自己的无界面
命令。想接任何 OpenAI 兼容的服务，写一个带 `[api]` 段的 profile（这里是
`~/.dct/profiles/gateway.toml`）再直连。下面的地址只是示例，不是真实端点：

```toml
# ~/.dct/profiles/gateway.toml
name    = "gateway"
command = ["true"]                     # 不是交互式 agent，只用到 [api]
[api]
base_url = "http://127.0.0.1:11435"
wire     = "openai"                    # "openai" = /v1/chat/completions
```

```toml
# ~/.dct/config.toml
[llm]
provider  = "gateway"                  # 上面那个 profile 的名字
transport = "http"
model     = "<模型名>"                 # http 必填，dct 不会替你猜
# base_url = "..."                     # 可选；覆盖 profile [api] 里的 base_url
```

`dct llm check` 会真的通过这条连接打一次，告诉你通不通——现在就知道，而不是等下一次出错。

---

## 用手机看

设置页里有一项「用手机看」。打开它，dct 会在终端里画一个二维码；用**同一个
WiFi 下**的手机扫一下，就能看到你的会话、每个会话的实时画面，以及一个能打字
的输入框。

那是守护进程在你自己网络上发的一个网页。**没有任何东西经过服务器**——压根
没有服务器——所以完全不联网也能用，而出了门就用不了。让手机在任何网络下都
够得着，出了门可以用 [Telegram](#telegram-会告诉你干完了)；让这个网页本身在任何网络下都打得开
要走中转，设计写好了、代码写了一部分，但还没接上开关：见
[`docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`](docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md)。

- **第一次打开时系统会问你允不允许**，选「允许·专用网络」，否则手机连不上。
  这句话在你按下那个开关**之前**就写在屏幕上。
- 令牌放在网址的 fragment 里，所以它只进二维码，不进屏幕上那行地址——屏幕会被
  拍照、被投影、被录进屏幕录像，而看过那行字的人就能往你的终端里敲字。
- 手机**不会改终端尺寸**。一个 PTY 只有一个尺寸，两个客户端抢它会让 agent 的
  画面在桌面那边也跟着重排；手机是把整块画布缩到屏幕宽度。
- 页面一转到后台就不再发任何请求，手机揣在兜里不会每秒问你的电脑三次。
- **字号自己适配**：按屏幕算一个刚好放得下整屏画面的字号，`+` `−` 还能再调，调过的记着。
  底栏钉在屏幕下沿，不跟着内容滚。
- **接了键盘就直接打**：按亮右上角那个键盘按钮，iPad 或手机上的实体键盘直通会话，方向键、
  `Ctrl+C`、中文输入法都进得去。`PageUp` `PageDown` `End` 留着翻历史——跟桌面一个规矩。
- **点屏幕上的任意一行**，那行字就进输入框，改一改再发。
- 手机可以自己挑配色（连画面一起），桌面那边不受影响。
- 同一个网络里拿到令牌的人就能往你的会话里敲字。它默认关着，关掉也只要一个键。

## Telegram 会告诉你干完了

手机网页要同一个 Wi-Fi，这个不用——不过它需要 Telegram，而中国大陆不用 VPN 连不上 Telegram。
在 Telegram 里找 BotFather 建一个 bot，然后到设置页打开「手机通知」，把它给你的令牌粘进去。
给这个 bot 随便发一条消息，就配对好了。

- **会话停了、出错了、没了，都会来一条消息**，带着会话名和项目。同时发生好几件，会合成
  一条，不会让你兜里连着震。
- **回复那条消息**，你打的字就进那个会话——不用选、不用猜。配了 `[llm]` 的话，一个等你
  做选择的会话发过来时，选项已经列好了。
- 不是回复的消息，给那个唯一在等的会话。好几个都在等，bot 会反问你是哪个，不猜：敲错
  agent 的代价远大于多问一句。`/ls` 列出会话，`/use <n>` 把之后的话指给其中一个。
- **填完令牌之后第一个给 bot 发消息的人成为主人，其余所有人的消息永远丢弃。** bot 的名字
  是公开的，谁都能给它发消息，这一条就是「我的手机」和「陌生人往我终端里敲字」之间的那条线。
  主人会存下来，所以重启不会重新打开配对；设置里的「重新配对」才会。
- 离开这台电脑的只有 dct 已经替你写好的话——会话名、项目、选项。原始画面从来不发。

## 直播给学生看

上课时想让一屋子人看你的终端，按 `L` 进直播面板：挑几个会话「上架」，拿到一条
链接和一个二维码，发出去。学生用浏览器打开**只能看，不能操作**。

要用它得有一个中转，而 **dct 不带任何默认中转**：启动 dct 之前把 `DCT_RELAY`
设成你自己跑的中转地址（就是本仓库里的 `dct-srv`，部署见
[`docs/deploy-live-relay.md`](docs/deploy-live-relay.md)）。没设的话开播时会直接
告诉你，而不是给你一条打不开的链接。画面经过哪台机器，由你决定，不是写死在程序里的。

「只能看」不是某处代码判出来的，是这条管子只有一个方向：守护进程每秒两次把
上架那几路的画面推到中转，学生只从中转读。**学生那一侧只有 GET，没有任何一条
路由能把字节送回你的终端。**

- **50–200 人同时看。** 你这台机器的负担跟观众人数无关——1 个学生和 200 个学生，
  推的是同一帧。带宽靠三件事压住：画面不变就不推、gzip、以及一条挂着等变化的
  长连接（安静时几乎不传字节，画面一变全体立刻亮）。
- **发的是文字，不是图片。** 一屏 gzip 完 3–5 KB，学生那边是真正的文字渲染——
  字号能自己调，深色模式跟他自己的设备走。
- **两把钥匙**：学生那把只能读，你那把只能写，而且**从不经过协议线**。知道链接
  的人推不了假画面，也停不掉你的直播。
- **只有上架的那几路上公网**，名字还是你自己起的——会话标题和项目目录一个字都
  不出去。切到别的会话不会把它播出去。
- **屏幕上一直写着你在播**：`● 正在直播 · 2 路 · 7 人在看`，关不掉、压得过别的
  提示。最危险的从来不是链接泄露，是你忘了自己在播。
- **公开到固定地址**：面板里按 `p`，填一个标题，这场直播就出现在中转首页的公开列表上，谁都能看，
  不需要链接。第一次会让你填管理员发的「公开直播密钥」（以后按 `K` 可以换）。再按一次 `p` 取消
  公开，私密链接照常能用。公开期间底栏写的是「● 正在公开直播 · 标题」。
- 改上架不换链接；按 `r` 才是真换一条，旧的当场作废。停播之后学生页显示「这场
  直播结束了」。
- 面板上那块二维码画的是**带令牌的完整链接**——学生要扫它。所以直播面板开着的
  时候别投影。
- 要对公网开的话，先读
  [`docs/deploy-live-relay.md`](docs/deploy-live-relay.md)：反代上**只许放行
  `/live/*` 和公开列表页 `/`**；要开「公开到固定地址」，运营方还得用
  `dct-srv key add`/`--publish-keys` 签发并装上发布密钥。

## 让 AI 玩三消游戏（Mac）

在 iPhone 镜像里打开 Candy Crush 的一关，然后在 dct 的会话里对 AI 说「帮我玩一关 Candy Crush」。
dct 读棋盘、按规则选步、让 dco 划，每步不到一秒；每一步的棋盘、所有能走的步和选择都记在 `~/.dct/games/log/`。
也可以自己运行：`dct game play [--steps 20] [--dry-run]`。棋盘的位置写在 `~/.dct/games/candy-crush.toml`（没有这个文件就用内置的）。

加上 `--auto-next`，一局没过时它会自己点「Try again」「Play」重来；弹窗只关「Close」「Not now」「No thanks」这类安全的；
遇到要花钱、广告、生命用完、通关、不认识的画面就停下，什么都不点（`--tries` 限制重来的次数，默认 5，每次都会用掉一条生命）。
打赢以后不会自动进下一关：下一关的棋盘位置不一样，要等「自动认新棋盘」做好。

## 发到自媒体平台（Mac，早期）

**只在这个分支上，而且它带着应用相关的内容，[边界](#dct-是什么)说 dct 不该持有这些。**

**这里的东西还没在真屏幕上跑过。** 下面说的都是对着假 dco 测的；第一次试点（先 X，再 TikTok）要你在场。

**1. 文字先批一次。**
`dct campaign approve launch --param x.post_text="…" --param tiktok.video=/path/a.mp4`
在终端里把每个平台的文字给你看，你敲 `y`、再按指纹才存（`~/.dct/campaigns/launch.json`）。指纹签的是这些确切的文字：之后手改这个文件（或旧版本批的、没有签名的文件），`dct do` 都会拒绝（退出码 2），要重新批准。各平台的文字和开头那句不能相同。
`dct do` 只发这里批过的东西。

**2. 发。** `dct do "post the launch text" --platform x --campaign launch`，要 dco 开着、平台的浏览器开着。
开着好几个浏览器窗口时它会列出来，你用 `--window <窗口号>` 指定。

- dcv 里有确认过的流程 `promote-<平台>-<post|video>`：交给 dco 重放。dco 找不到按钮时，dct 按下面同样的办法替它答；
  流程本身走不通了，dct 从那一步接着摸索，摸出来的写成新草稿，原来确认过的不动。`--preview` 停在发布那一步之前。
- 没有：dct 自己摸索。打开平台表里写的网址，把画面变成一道选择题，依次问 Jeff（把握过门槛才算）→ `[llm]` 的大模型
  （只发字，同一画面最多 2 次）→ 你（带一张编号图）。
- **遇到这些就停下交给你**：登录、二维码、验证码、手机验证、账号警告；这些画面上的字不发给任何模型。
  永远不点：关注、订阅、推广、升级、删除，动钱或撤不回的，以及平台自己列的禁点词。上限：40 步、10 分钟、连续 3 步画面没变。
- **每次点发布前都先问你。** 平台的发布按钮还没在真屏幕上核实过的（所有国内平台和小红书），每点一下都先问。
  `wechat_mp`（公众号）最后那下发布永远你自己点。
- 发出去以后，dct 把步骤写成流程草稿（连同它关掉的弹窗的规则）交给 dcv，自己绝不批准。dcv 不收，就让 `[llm]` 的大模型改，
  最多 3 轮。然后给你一张卡片，每一行都列出来；你说是 → 按指纹，批准只对卡片上那一版正文有效。
- 退出码：0 发了或试跑停下了；1 出错了（dco、dcv、批准那一步）；2 命令、配置、活动或窗口不对；3 停下了或你说不。
  `--json` 一行一个事件（`ask`、`confirm_publish`、`card`、`note`，最后是带 `exit` 的 `done` / `previewed` / `stopped` /
  `declined` / `error`），回答从标准输入读：`{"key": "e3"}`、`{"stop": true}` 或 `{"verdict": "yes"}`。

**它存下什么**（都是 0600，要发的文字写盘时换成 `{post_text}`，不存原文）：每次的步骤存在
`~/.dct/explore/runs/<编号>/trace.json`，连同给你看过的图（轨迹里也记下这次的结局）；每答一道题在 `~/.dct/explore/answers.jsonl` 留一行，留着以后训练 Jeff。这些行在运行结束时写下，每行记着谁答的（`by`：user / model / jeff）、发给 Jeff 的是哪几个选项（`shown`）、对应轨迹里哪一步（`step`）、这次的结局、这一下是不是确认离目标更近（`outcome`：good / bad / unknown），以及能不能当标准答案（`label`：用户的答案，和 `outcome` 为 good 的大模型答案；Jeff 自己的永远不算）。

**平台表**内置 16 行：x、tiktok、youtube、facebook、instagram、xiaohongshu、weibo、bilibili、zhihu、zhihu_article、douyin、
kuaishou、wechat_channels、toutiao、wechat_mp、baijiahao。没在真屏幕上核对过的字段都列在每行的 `unverified` 里，国内平台几乎全是。
在 `~/.dct/platforms.toml` 里改或加；同名的整行替换内置的。

**还没做的**：真屏幕试点；中间那一级换成更强的模型（等它的地址）；它依赖的 dcv、dco 改动在各自的仓库里做；
要点两下的发布（先「发布」、
再在确认框里点一次）：只有发布后的页面上不再有发布按钮，或者出现了核实过的「发出去了」字样，才算发出去。
X 的「发出去了」字样还没核实，它的首页也一直有「Post」按钮，所以在真机上核实之前，X 的真实运行点完第一下会一直挂着待定。

## 多电脑

家里一台 Mac、公司一台 Windows，各开着几个 agent：让它们互相留言。
一台电脑上的会话可以给另一台电脑上的会话留一句话，那个会话空着就直接敲进去。

```
dct login                       # 老电脑上：用 DC 账号登录
dct invite                      # 老电脑上：出一个 6 位邀请码（看板上按 a 也一样）
dct join 482913                 # 新电脑上：敲那个码（没登录会先自动登录）
dct peers                       # 组里有哪几台、各开着哪些会话
dct send Mac/#3 "run the Windows tests"
```

- **登录**：`dct login` 用配对过的 DC 账号换一张中转令牌，只在这台电脑上存着。
  第一台登录的电脑创建组。
- **加电脑只要一个码**：老电脑的看板上按 `a`（或者运行 `dct invite`），屏幕上出现
  `Invite code 482 913 · valid for 10 minutes`；新电脑上 `dct join 482913`，就进组了——
  老电脑的看板上提示「<name> has joined」。码里的空格、`-` 随便敲，全角数字也认。新电脑必须
  已经配对了**同一个** DC 账号：别的账号的电脑连问都问不到你的电脑。
- **码只能用一次**：用过一次（成不成都算）、过了 10 分钟、或者再按一次 `a` 换了新码，
  旧码就作废。有人拿错码试过，老电脑会告诉你；重新按 `a` 拿个新的。
  守护进程重启之后码也没了。
- **中转猜不出码**：码是一次 PAKE（SPAKE2）的口令，中转看得见来往的每一条消息也什么都
  学不到；冒充一台电脑，每个码只有百万分之一的机会。
- **看到码的人就能进来**（只要他也有你这个 DC 账号）：投屏、截图的时候留意。码就是
  同意，老电脑上不会再问一遍。
- **名字**：默认叫 Mac / Windows / Linux；组里撞名了，由老电脑编号（「Mac 2」），用 `--name`
  起的名字也一样——新电脑会告诉你最后叫什么。想自己起名：
  `dct join 482913 --name work-pc`。
- **留言**：`dct send <computer>/<session> "<text>"`，会话可以写 `#id`。
  对方看到一行标记（`[来自 home-mac/docs 的留言 #a1b2]`），然后是正文。会话空着：马上敲进去。
  忙着：排队，一次一条。
- **中转看不到内容**：留言在你的电脑上就为唯一的收件人加密好；中转只管转发，从不落盘，
  看到的只有密文。
- 中转默认是 `https://dataclue.cn/dct-relay`；要换，在 `~/.dct/config.toml` 里写
  `[mesh] relay = "…"`。

**还做不到的：**

- 对方电脑不在线 → 没送出去（会告诉你）；不会替你存着等它上线；
- 只有 agent 会话能收，普通的终端会话不行；
- 排着队的留言和还有效的邀请码，守护进程一重启就没了；
- 两台老电脑同时各拉一台新电脑进组，名单可能分叉；不在线的电脑会错过名单变化；
- 同账号里的任何一台电脑都可以不停拿错码去试、把你的码烧掉
  （进不来，但烦人）；这一版不防；
- 还不能进入另一台电脑的会话——那是下一步。

### 部署

网关和中转两边都要开起来。**下面是上线清单；照着做就是上线，所以先跟管服务器的人确认。**

1. **网关**（dc_llm）：按设计文档附录实现 `POST /admin/api/relay/token`（已完成）。打开开关——它在网关里的
   实际名字是 **`DC_ADMIN_RELAY_TOKENS_ENABLED`**（设计文档写作
   `DC_RELAY_TOKENS_ENABLED`，是同一个东西）——签名密钥放在
   `DC_ADMIN_RELAY_SIGNING_KEY`。`python -m scripts.relay_signing_key show`
   会打印公钥，一行 base64。
2. **反向代理**：在 dataclue.cn 上加一条 `/dct-relay`，TLS 用现有证书。
   只把 `/dct-relay/link/*`（去掉前缀）转给下面的中转——
   `--with-link` 还会挂一个 `/phone`，这里用不上；保持关闭。
3. **中转**：把公钥放进 `/etc/dct-srv/gateway.pub`（一行一把；
   要轮换，先加新的一行，等它生效，再换网关的私钥），然后运行

   ```
   dct-srv 127.0.0.1:<port> --with-link --relay-keys /etc/dct-srv/gateway.pub
   ```

   地址是**第一个参数**——没有 `serve` 子命令，也没有
   `--addr`。中转仍然只监听本机；公网流量由反向代理转进来。
4. **验收**：两台真电脑（一台 Mac、一台 Windows）走一遍 `dct login` →
   `dct invite` → `dct join <code>` → `dct send`，看留言出现。
5. **验多行留言进真的 agent**（还没验过）：在 Windows 那台上，用 `dct send` 往一个
   Claude Code 会话和一个 Codex 会话各发一条两行的留言。每条都必须作为**一轮**进去——
   标记行和两行正文在一起。要是哪个 agent 在第一个换行就提交了，标记行会单独进去，
   正文变成一条没有标记的输入，agent 会当成用户本人说的话：先停下上线，给那个 agent
   改成用 bracketed paste（`ESC[200~ … ESC[201~`）包住打进去的文字。结果记进台账。

想在一台机器上把整件事试一遍，不用网关，不用公网：
`cargo test --test mesh_e2e -- --ignored --nocapture` 会起一个真的中转和两个
互不相干的守护进程，用 `dct-srv token keygen`/`token mint` 签的测试令牌代替网关，
走完 login → invite → join → peers → send，并检查中转的输出里从不出现留言原文。

## 浏览器里的 dct

上面这些都假定你有一台过得了[那张硬件表](#装上它)的电脑。过不了的人——老笔记本、没有
管理员权限的公司电脑、Chromebook、平板——用 [`container/`](container/README.md)：整个
看板跑在服务器上，他们打开一条链接就行。关掉浏览器，会话照跑，跟关掉终端一样。

- **`dct gate` 是那道门。** 钥匙就在链接里，第一次打开换成一个 30 天的 cookie，随即从
  地址栏抹掉。没带钥匙的请求，不管问的是哪条路径，得到的拒绝都一模一样。它**不加密**，
  所以默认只绑本机，要 `--bind` 才对外，而且对外之前**前面必须架一层 TLS**。
- **终端旁边有一个面板，装终端装不下的东西**：粘贴或上传图片（先预览、可以补一句说明，
  再插进会话），上传文件到项目的 `uploads/` 里让 agent 去读，下载单个文件或者把整个项目
  打成 ZIP。挂在课堂服务后面时还多一个话筒按钮，说的话变成文字填进输入行。结束一次练习不会删掉代码。
- **给一个班用**，[`container/classroom/`](container/classroom/README.md) 是老师的后台：
  按姓名加学生、给每人一条专属链接、只读看某个学生的终端或者进去帮忙、把某个学生的工作区
  拿去直播——愿意的话可以公开。

怎么起、有哪些环境变量、放进容器之后哪些地方会变差，都在
[`container/README.md`](container/README.md) 里。

---

## 配色

在会话里按 `F6`，或者进设置页，打开的是同一张十四档配色表，管标题栏和底栏。方向键当场把
颜色刷在真实的 agent 画面上，`Enter` 留下，`Esc` 换回原来那档，选完存盘，重启还在。

每一档都是一对 256 色索引（底色 + 前景），绝不用会被终端主题改写的 0–15 号具名色；
有一条测试按 WCAG 公式算每一对的对比度，低于 4.5:1 的进不来。`NO_COLOR` 一票否决，
强制退回只画横线那档。

---

## 会踩到的坑

**最要紧的一条：那四家的端点地址是照公开文档抄的，从来没拿真账号试过。** 密钥有可能验证
通过、会话照样起不来。在有人拿真凭据跑通之前，Kimi、GLM、DeepSeek、Qwen API 这四个就当
没验证过。

**DC 在看板上没有状态，也没有「继续」。** 它跑的是 Qwen Code，而那个 TUI 的屏幕特征和
恢复参数都还没人实测过——没实测就不填是这个仓库的规矩，所以 DC 那一格状态永远显示
「—」，会话关掉再开也是全新的一个（界面会明说是重开的）。地址本身是通的，钥匙也拿得
到，缺的只是这两样便利；实测一次就能同时补上。

**权限是全自动接受的，所以 agent 有可能写到项目目录外面去。** 那部分改动不在快照范围内，
撤销撤不回来。

同一个项目里开两个 agent，它们会抢同一批文件。跨项目就没事。

`opencode` 和 `qwen` 在列表里，但这两个一次都没真跑过，所以没给它们配屏幕匹配，
会话状态一直是 `—`。

给会话起名字要靠 `~/.dct/config.toml` 里配好的 `[llm]`，多数人根本没配——这是正常状态，
不是毛病。没配、或者模型答得慢到超时、或者答非所问，起名这件事会自己安静地退下去：
名字退回你说的第一句话，截短了用。全程不报错、不打断会话。

号码只发给前九个组。第十个项目起没有号码，只能用 `Tab` 一个一个翻过去。

<details>
<summary>鼠标、复制，以及粘贴图片</summary>

<br>

`dct` 只在 **agent 自己要鼠标的时候**才接管它。Claude Code 会要（它自己用鼠标滚它那一屏），
codex 和普通命令行不要——只要会话里跑着的东西没自己伸手要鼠标，鼠标就归终端，拖动选中文字、
复制，跟平时完全一样。代价是那些会话里滚轮不再翻 `dct` 的历史，用 `PageUp`/`PageDown`/`End`。

屏幕一行内完整显示的 `http://`、`https://` 地址会标为终端超链接，使用终端自己的
打开链接手势（通常是 Cmd+点击或 Ctrl+点击）。agent 占用鼠标时先按 `F4`。
支持 OSC 8 的浏览器终端也能使用。目前的文字快照无法还原藏在标签后的地址或跨行网址。

在 agent 要鼠标的会话里想复制，按 `F4` 进复制模式：鼠标临时还给终端，底栏会写着现在是这个
状态，复制完再按一次 `F4` 回去。也可以用终端自己的修饰键（iTerm2 是按住 Option），不用退出
会话。`dct` 自己没有复制功能——复制用的是你终端本来那一套。

粘贴图片是反过来的一件事，而且得有自己的键：`F5`。终端这根管子只过字节，图片过不去——
你按终端自己的粘贴键，它去读剪贴板，发现里面是图不是字，于是**一个字节都不发**。dct 连
「你按过粘贴」都不知道，所以这个键不可能是 `Ctrl+V`。`F5` 是让 dct 自己去读剪贴板：
把图片存成临时目录里的一个文件，然后把**那条路径**当成你敲进去的字送给 agent。截图
（Win+Shift+S、Cmd+Ctrl+Shift+4）能用，在资源管理器/访达里拷一个图片文件也能用——后者直接
送它原来的位置，不再复制一份。剪贴板里是文字、或者是空的？底栏说一句，什么都不发。
目前只有 Windows 和 macOS 支持。

</details>

界面有中文和英文两种。按 `l` 切，也可以用 `DCT_LANG=en` 临时压过一次；不设就跟着系统 locale 走。

---

## 配置参考

所有东西都在 `~/.dct/` 下（见[文件](#dct-下的文件)）。配置读不了或解析不了，从不会让 dct 起不来；
dct 退回下面的默认值（所有可选功能都关着），并往 stderr 打一行。

**`~/.dct/config.toml`**

| 键 | 默认 | 作用 |
|---|---|---|
| `[llm]` `provider` | 无（功能关闭） | 提供大模型的 profile 名；绝不替你挑厂商 |
| `[llm]` `transport` | `"cli"` | `"cli"` 或 `"http"` |
| `[llm]` `model` | 无 | `http` 必填 |
| `[llm]` `base_url` | profile 的 `[api].base_url` | 覆盖 |
| `[judge]` `backend` | `"jev"` | 唯一接受的值 |
| `[judge]` `base_url` | 无 | `jeff-serve` 的地址；没有它就从不问 Jeff（只写了 `gate_tiers` / `model_veto` 的 `[judge]` 也可以） |
| `[judge]` `model` | `"jeff-latest"` | `jeff-serve` 认的模型名 |
| `[judge]` `api_key_env` | 无 | 存着密钥的环境变量名；只作为 Bearer 发给 `base_url` 的 host；明文 `http://` 只发往本机 |
| `[judge]` `threshold` | `0.8` | Jeff 的答案只有把握达到或超过这个值才算数 |
| `[judge]` `timeout_ms` | `5000` | 每个请求 |
| `[judge]` `gate_tiers` | `["critical", "money"]` | `dct answer` 拦哪几档；可以加 `"content"`；`critical` 和 `money` 去不掉 |
| `[judge]` `model_veto` | 配了 `[llm]` 就开 | `false` 关掉模型否决 |
| `[judge.serve]` `command` | 无 | 启动 judge 服务的 argv 数组（不经 shell）；没有它，dct 从不启动或停止任何东西 |
| `[judge.serve]` `env` | `{}` | 额外的环境变量，只能是字符串 |
| `[judge.serve]` `ready_timeout_secs` | `60` | 等 `GET <base_url>/health` 的上限 |
| `[judge.serve]` `autostart` | `false` | `true`：连不上时 `dct answer` 自动启动服务一次 |
| `[judge.serve]` `min_free_percent` | `15` | 空闲内存低于这个百分比就不启动 |
| `[ask]` `enabled` | `false` | 只有为 `true` 时 `dct ask` 才工作；`dct ask --enable` 会写它 |
| `[ask]` `allowed_hosts` | `["dc-llm.tzspace.cn"]` | `dct ask` 唯一会发往的 host |
| `[menu]` `agents` | `[]`（全部显示） | 选择器显示哪些 agent profile，按这个顺序 |
| `[mesh]` `relay` | `https://dataclue.cn/dct-relay` | 多电脑用的中转 |

**`~/.dct/voice.toml`**（可选）：`log_text`（默认 `false`），以及一个 `[alias]` 表，
形如 `对的词 = ["识别器写成的词", …]`。

**`~/.dct/profiles/*.toml`**：一个文件一个 agent。dct 读的键：`name`、
`command`、`is_agent`、`pairable`、`idle_pattern`、`busy_pattern`、
`error_pattern`、`env`、`secret`、`install`、`login`、`headless`、`api`、
`label`、`note`、`resume_args`、`backend_only`。见[自己的 agent](#十个-agent一个入口)。

**`~/.dct/platforms.toml`**（可选，给 `dct do` 用）：替换或补充内置平台表的行。
**`~/.dct/games/candy-crush.toml`**（可选）：`dct game` 用的棋盘在屏幕上的位置。

dct 自己用的**环境变量**：`DCT_LANG`（`en` 或 `zh`；优先于保存的选择和系统语言环境）、`NO_COLOR`、
`DCT_RELAY`（直播中转；没有默认值）、`DCT_RELEASE_BASE`、
`DCT_INSTALL_DIR`、`DCT_MINGIT_URL`、`DCT_NODE_BASE`、`DCT_NPM_REGISTRY`（见
[装上它](#装上它)）。

---

## `~/.dct` 下的文件

所有路径都是从守护进程 socket 所在的文件夹推出来的，所以测试能把它们挪到临时目录，
而不碰你自己的。

| 文件 | 是什么 |
|---|---|
| `daemon.sock` | 守护进程的本机 socket（仅所有者可访问） |
| `config.toml` | 上面那份配置；配对和 `dct ask --enable` 只往里追加（从不重写） |
| `secrets.toml` | API 密钥和 `dct gate` 的密钥；**权限 0600**；绝不放进 profile 文件 |
| `settings.json` | 语言、看板画法、底栏颜色 |
| `projects.json` | 最近的项目，以及每个项目上次用的 agent |
| `last-sessions.toml` | 上次开着哪些会话，好让重启后的守护进程提议接回来 |
| `sessions.log` | 每个会话怎么起来的、又是怎么没的 |
| `pair-models.toml` | 配对写下的模型名 |
| `profiles/` | 你自己的 agent profile |
| `runtime/` | dct 自带的 Node 和便携 git，只给 dct 自己用 |
| `ask.log` | 每次 `dct ask` 一行**元数据**（权限 0600）；没有文字 |
| `voice.toml`、`voice.log` | 语音设置；语音日志（权限 0600），默认只有元数据 |
| `judge-serve.pid`、`judge-serve.log` | dct 启动的 judge 服务的 pid 记录和它的日志（权限 0600，每次启动重新写）；`judge-serve.lock` 和 `judge-serve.err` 也写在它们旁边 |
| `dco-rules.json` | 原样写自 `dco rules --json`，给 dc-vault 规则页用 |
| `explore/answers.jsonl` | 来自 `dct do` 和 `dct train record` 的训练数据（权限 0600） |
| `explore/runs/<run>/trace.json` | 每次 `dct do` 运行的步骤，和它给你看过的图片 |
| `campaigns/<name>.json` | 批过的 campaign 文字，连同它们的指纹 |
| `platforms.toml` | 你给 `dct do` 写的平台行 |
| `games/log/<date>.jsonl` | `dct game` 每走一步一行 |

---

## 安全与隐私

- **听到的不算授权。** 除非是点击 dco 章鱼触发的，`dct voice` 什么都不发，从不回答批准提示，
  也从不发一个光秃秃的「yes」。`dct ask` 的 `source` 字段只是给大模型的提示，别无其他。
- **默认不留转写。** `voice.log` 和 `ask.log` 只存次数、耗时和决定，不存话
  （`voice.toml` 的 `log_text = true` 是让 `voice.log` 里出现话的唯一办法）。`dct ask` 的记忆在
  守护进程的内存里，从不写盘。旧版本写进 `voice.log` 的行可能仍然含有话。
- **你不打开，它就是关的。** 没有 `[llm]`，就不调大模型。没有 `[ask] enabled = true`，就没有聊天。
  同意不能由别的程序代给：`dct ask --enable` 和 `dct campaign approve` 不在终端里时会拒绝。
- **只用你自己的网关。** `dct ask` 只发往 `[ask] allowed_hosts`（默认 `dc-llm.tzspace.cn`）里的
  host，走 `http` 传输；把厂商命令行当 `[llm]` provider 用于 `ask` 会被拒绝。从 Claude Code 或
  Codex 借来的登录，只会发给那家厂商自己的域名，绝不会发到别处。
- **失败解释会发什么。** 配了 `[llm]` 时，出错会话的屏幕末尾（最后 2 000 个字符）会发给你的大模型。
  密钥和令牌往往就出现在那里，这就是 `[llm]` 要自己打开的原因。
- **密钥。** `secrets.toml` 权限是 0600，profile 文件从不存密钥。别把密钥放进 `[judge.serve]` 的
  `command` 或 `env`：机器上任何用户都能用 `ps` 读到。`[judge] api_key_env` 写的是变量名，
  从不存密钥本身。
- **危险闸是规则。** 它按词表而不是按把握来拦钱和永久删除，但只认 `dct-brain` 认识的词
  （[局限](#危险闸dct-answer)）。调用方仍应自己筛掉知道有害的选项。
- **撤销有边界。** 快照覆盖项目目录；agent 写到目录之外的东西撤销不了
  （[会踩到的坑](#会踩到的坑)）。
- 手机页、直播链接和浏览器那道门各有自己的令牌规矩，写在各自的章节里。

---

## Jeff 的考试集和开发集

训练和评估 Jeff 只守一条规矩：**考试集从不用来选任何东西。** 这个分支里的文件：

- `tests/fixtures/jeff-exam/exam-v3.jsonl`（364 题）和 `manifest-v3.json`。
  2026-10-14 由负责人**冻结**并留出：绝不能拿它来训练、开发、校准或调阈值；每个候选只在已经选定
  的阈值上评估一次；导出训练数据时，丢掉所有 `dedup_sha256` 等于某道考题的行。改动哪怕一个字节，
  就是一个新的考试版本，需要负责人同意，新旧分数分开报告。`exam-v2.jsonl` 和 `manifest.json`
  留作历史记录。
- `tests/fixtures/jeff-dev/`（`dev-v3.jsonl`、`dev-v4.jsonl`、它们的 manifest 和场景文件）：
  阈值就是在这个集合上**选**的。v3 的 manifest 把 `0.98` 记作在 `dev-v4` 上选出的暂定阈值；
  代码里 `[judge] threshold` 的默认值仍然是 `0.8`。
- `scripts/jeff-exam/`：`build_exam.py`、`check_overlap.py`、`diff_danger.py`、
  `exam_lib.py`、`danger_allowlist.json`，以及 `run_exam.py`——它把每一题都问一遍运行中的
  `jeff-serve`（`python3 -I scripts/jeff-exam/run_exam.py --label
  v1.2 --out DIR [--set exam|exam-v3|dev|dev-v4] [--url …] [--seeds 5]
  [--threshold 0.8]`），而且一个集合跟它的 manifest 对不上时就拒绝运行。

在用的模型是 Jeff v1.2（`jeff-legacy/Jeff-Qwen3.5-0.8B`，revision `d86f657`）。训练设计
（`docs/superpowers/specs/2026-10-11-dct-jeff-auto-training-design.md`）在负责人批准重训出来的
新模型之前，一直保留它。`docs/HANDOFF.md` 说，它写成的时候只做了训练数据的格式
（`dct train record`、`explore/answers.jsonl`）；那份设计的其余部分没有对照代码核对过。

---

## 排错

- **两个版本都显示 `dct 0.2.24`。** 版本号区分不了 `origin/main` 和这个分支。如果 `dct --help` 里
  没有 `dct ask`、`dct answer`、`dct judge` 或 `dct do`，你跑的就是已发布的版本。
- **守护进程「过期」了。** 表现：看板问要不要重启守护进程，`dct status` 说守护进程已过期，
  `dct ask` 以 10 退出并带 `reason: "protocol"`。这个守护进程是用 socket 协议不同的版本起的。
  `dct restart` 把它换掉，并且**结束所有在跑的会话**（文件改动还在，agent 没了）；
  `dct restart -y` 跳过确认。版本号不变地重新编译之后，没有任何东西会注意到区别，
  所以自己运行 `dct restart`。
- **`Shift+F3` 没有跳到上一个会话。** 用 **`F8`**，它是靠得住的那个。`Shift+F3` 已经实现、也有
  单元测试，但有一位用户报告说它在他的终端里没有任何反应。
  `Ctrl+F3` 被 macOS 占了。猜测的原因见下一条，怎么看你的终端发了什么，见[看板](#看板)。
- **已知问题：`Shift+F3`（上一个会话）可能根本不会作为按键事件到达。** *这是读代码得出的诊断，
  还没有在真实终端里确认。* `dct` 通过 crossterm 0.28.1 读输入，它把任何 `CSI … R` 都当成光标位置
  回复而丢掉，而 xterm 风格的终端（以及开了 `xterm-keys`、没开 `extended-keys` 的 tmux）把
  `Shift+F3` 发成 `ESC[1;2R`。`Ctrl+F3`（`ESC[1;5R`）同样被丢掉。单元测试是直接构造 `KeyEvent`
  的，所以覆盖不到解码这一步。变通办法：用 `F8`。想知道你的终端发了什么，运行 `cat -v` 再按那个键
  （出现 `^[[1;2R` 就坐实了）。
  **已在 `fix-shift-f3` 分支修复（内置的 crossterm 把 `ESC[1;2R`..`ESC[1;8R` 当作带修饰键的 F3）；
  装上后还需要在真实终端里确认。**
- **把新的 `dct` 复制到旧的上面之后马上 `zsh: killed`。** 别拿 `cp` 去覆盖已经装好的二进制；
  见[装上它](#装上它)里「别自己拿 `cp` 往装好的那个文件上覆盖」那一段。
- **`dct voice` 说 dco 缺失或太旧。** 启动 dco，或者更新它；这条命令需要 dco 的 `hear` 工具。
- **`dct judge up` 拒绝。** `[judge] base_url` 必须是 `127.0.0.1`、`localhost` 或 `::1`；
  空闲内存必须高于 `min_free_percent`；`down` 只停 dct 自己起的服务。
- **`dct answer` 退出 1。** 配置读不了，或者 `[judge] gate_tiers` 写了不认识的档位、或者
  漏了 `critical` 或 `money`。原因在 stderr。
- **有个 pty 测试失败一次，重跑又过了。** 打开真伪终端的测试，在整套测试并行运行时出现过
  间歇性失败，原因还没查清。一个一个跑：`cargo test -- --test-threads=1`。

---

## 更多文档

- [`docs/HANDOFF.md`](docs/HANDOFF.md)：explore-and-learn 分支做到哪儿了，以及关于本机决策
  模型的研究笔记。在这个仓库开新会话，先读它。
- [`docs/2026-10-16-dct-ask-design.md`](docs/2026-10-16-dct-ask-design.md)：`dct ask`，
  带测量数据和老实的「没做」清单。
- [`docs/2026-10-03-dct-dco-dcv-distributed-agent-runtime.md`](docs/2026-10-03-dct-dco-dcv-distributed-agent-runtime.md)：
  更早的一份架构笔记，讲 dct、dco 和 dcv 的关系。
- [`docs/superpowers/specs/`](docs/superpowers/specs/)：每个功能一份设计（judge、brain、
  控制层、配对、多机、邀请码、语音中转、三消、Jeff 自动训练……），旁边是
  [`docs/superpowers/plans/`](docs/superpowers/plans/)。
- [`docs/deploy-live-relay.md`](docs/deploy-live-relay.md)：把直播中转放到公网上。
- [`container/README.md`](container/README.md)：浏览器里的 dct。
- [`docs/2026-09-04-gateway-findings.md`](docs/2026-09-04-gateway-findings.md)：对网关了解到的东西。

---

<details>
<summary><b>方向</b> —— 还没写的那些</summary>

<br>

放在这儿是为了说清楚上面那些东西是冲着什么去的。

真正想要的不是「在任何地方看终端」，而是**人不在，开发照样往前走**。你只管三件事：
提目标、拍板、验收。中间的理解、写、测、修，不该需要你盯着。

- **agent 主动找你，而不是停在那儿等。** 一个 `ask_human` 工具，agent 一调就阻塞，
  问题送到手机上，你的回答作为工具返回值送回去，它接着干。
- **国内能用的手机通道。** [Telegram 已经做完了](#telegram-会告诉你干完了)，但国内连不上，
  短信在监管上做不成客户端，所以要走一个自己的中转服务（端到端加密，只服务
  dc_classroom 上注册的用户），手机网页当客户端。这部分还没做完，见
  `docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`。
- **只有一套消息格式。** 出站永远是一句话加编号带标签的选项，入站永远是自由文本。
  这套约束来自语音场景：问题要能被念出来，回答是「就第二个吧」而不是 `2`。
- **任务取代会话成为主角。** 你说「修复移动端登录后白屏」，而不是先选 PTY、选 agent、选目录。
- **`dc_llm` 常驻干便宜的活**：判断状态、压缩上下文、归类你的回复、把技术细节写成手机上
  能读的决策卡片。贵的前沿模型只在真正写代码的时候才叫。
- **跑完测试才算完成。** 自动识别技术栈和测试命令，自动跑，失败了让 agent 在限定轮次内
  自己修，通过了才推给你验收。

</details>

## 开发

```sh
cargo build --release          # ./target/release/dct
cargo install --path . --root ~/.local   # 把二进制放到 scripts/install.sh 放的地方
cargo test -- --test-threads=1 # 为什么一个一个跑，见「排错」
cargo clippy --all-targets
cargo fmt --check
```

这个仓库是一个 Cargo workspace；根包就是 `dct` 本身，`crates/` 里有 `dct-brain`、`dct-dco`、
`dct-explore`、`dct-game`、`dct-link`、`dct-mesh`、`dct-page` 和 `dct-srv`。测试会建真的 git 仓库、
起真的进程、绑真的 socket，但没有任何东西碰你自己的 `~/.dct`，也没有任何东西联网。需要真 dcv
或真中转的测试都标了 `#[ignore]`（例如 `cargo test --test mesh_e2e -- --ignored`）。
`cargo fmt --check` 在早于这个分支的文件上就已经失败
（`docs/HANDOFF.md` 记录的是 55 个文件，写于 2026-10-11，之后没有重新核对），
所以别把整个仓库的格式化折进一次功能提交里。

改动涉及守护进程 socket 时，要提升 `src/proto.rs` 里的 `PROTOCOL_VERSION`（有个测试把线上格式
钉在它上面）；这个分支是 25。改了之后，每个在跑的守护进程都是过期的，直到 `dct restart`。

<details>
<summary><b>给改代码的人</b>——源码布局和这儿的规矩</summary>

<br>

两个进程，走 `~/.dct/daemon.sock`（只有属主能访问）收发按行分隔的 JSON。

```
src/ui/mod.rs      事件循环、终端生命周期、按键与渲染的分发
src/ui/view.rs     View 枚举和它的纯函数
src/ui/app.rs      循环的状态，收在一个结构里
src/ui/board.rs    会话列表
src/ui/grid.rs     九宫格：布局数学、裁剪、渲染
src/ui/attach.rs   单个会话，整屏
src/ui/pick.rs     选 agent、选项目
src/ui/secret.rs   密钥相关的几个页面
src/ui/widgets.rs  补空格、截断、状态配色
src/theme.rs       终端背景是深是浅，以及据此选出的弱化文字样式
src/settings.rs    语言、看板画法、配色，存盘
src/client.rs      单条连接，5 秒读超时，一出错就重连
src/daemon.rs      请求分发，一个连接一个线程
src/session.rs     会话生命周期，200ms 一次 tick 从屏幕上读状态
src/pty.rs         PTY 加一个 vt100 屏幕缓冲
src/profile.rs     profile 结构、内置清单、磁盘加载、可用性判定
src/secrets.rs     ~/.dct/secrets.toml
src/verify.rs      密钥探测
src/git.rs         隐藏快照
src/projects.rs    最近项目、上次用的 agent
src/last_sessions.rs  上次开着哪些会话，好让重启后的守护进程问要不要接回来
src/journal.rs     每个会话怎么起来的、又是怎么没的（被叫停，还是自己消失）
src/proto.rs       线上契约
src/config.rs      ~/.dct/config.toml；[llm] 不写就是关着
src/llm/           [llm] 那条连接：走 CLI 还是 HTTP，哪份凭据能发到哪儿
src/judge/         dct judge：用 HTTP 给 jeff-serve 发 Jev 选择题；把握由 dct 自己重算
                   serve.rs 启动和停止本机 judge 服务（dct judge up / down / status）
src/answer.rs      dct answer：问答的几级、请求/答案的 JSON、基于规则的危险闸
src/answer_veto.rs 闸之后的模型否决（它只能加否决）
src/train_record.rs  dct train record：每个回答过的问题一行 v2 训练数据
src/ask/           dct ask：输入检查、流式的一轮、记忆（只在内存）、固定提示、
                   守护进程一侧的服务、ask.log（只有元数据）。从不碰会话
src/voice/         dct voice：各道关卡、路由到某个会话、dco 的语音路由、voice.log
src/dco_rules.rs   dct dco-rules，以及让 ~/.dct/dco-rules.json 保持新鲜的守护进程监视器
src/mesh/          多电脑：login、invite、join、peers、send
src/game/          dct game：通过 dco 玩三消和找东西类游戏
src/keys/, src/procedures.rs  dct keys 和 dct procedure（Touch ID 签名密钥、批准、票据）
src/ui/dco_cell.rs 看板上只读的 dco 标签
src/campaign.rs    dct campaign approve：各平台的文字一次批准
src/explore_cli*   dct do：选窗口、重放或摸索、草稿、卡片、退出码、--json
src/explore_model.rs  dct do 里「大模型」那一级：从清单里挑一个 key，或者改 dcv 不收的草稿
src/learn.rs       草稿以 import:agent 写进 dcv、确认卡片、指纹绑在正文上
src/runtime.rs     dct 自带的 Node 和 git，放在 ~/.dct/runtime
src/pair*.rs       DC 配对：状态机、它的 HTTP、把结果写下来
src/channel/       手机通道；目前只有 Telegram。不认识会话
src/bridge.rs      会话 <-> 通道：谁是主人、哪条回复该进哪个会话
src/live.rs        把上架的画面推给中转（DCT_RELAY）
src/gate.rs        dct gate，浏览器版前面验钥匙的那道门
src/web/           局域网手机端：一个极小的 HTTP 服务和那一页网页
src/link.rs        主动出网连中转、长轮询（还没接上开关，见下）
src/sys/           Unix 和 Windows 不一样的地方全在这儿
crates/dct-brain/  危险闸的档位、执行票据、签名校验；不碰磁盘，不碰网络
crates/dct-mesh/   机器 id、6 位邀请码（SPAKE2）、密钥对、签过名的组名单
crates/dct-game/   三消的走法选择（纯函数）、`dct status --json` 背后的 dco 状态探测
crates/dct-dco/    dco 客户端（unix socket 上的 MCP），dct game 和 dct do 共用
crates/dct-explore/  手、平台表、停下的规矩、选项、Jeff → 大模型 → 你、摸索、轨迹和训练题、草稿、重放
crates/dct-link/   守护进程和中转共用的信封，不含 Request/Response
crates/dct-page/   仅有的两页网页：手机端那一页，和直播观众那一页
crates/dct-srv/    中转服务。默认只挂 /live/* 和公开列表页 /（--with-link 才加上没有鉴权的配对路由，只给本机开发用；
                   --publish-keys 才开放公开直播）。
                   第一期没有鉴权也没有加密，只许绑本机地址
```

动代码之前有三个决定值得先知道。

**可用性判定放在守护进程里算，不放界面。** 因为守护进程的 `PATH` 才是子进程真正会拿到的
那个。在别处问这个问题，你可能兴高采烈地报「可用」，然后一开就失败。

**`create()` 期间不持有任何锁。** 开会话要起 PTY、还要 shell 出去跑 git，握着共享锁干这些，
别的客户端就全在等你。`src/session.rs` 里有长注释，还有一个测试专门量这件事。

**协议上传的字符串已经是用户那门语言了。** `ProfileEntry.label` 是 `String`，
不是 `LocalizedText`。用户可见的文案在哪儿组句，答案只有一个地方，就是守护进程。

### 这儿的规矩

注释解释为什么，不解释是什么。这个仓库的注释密度是刻意的，也确实救过我们不止一次，照着写。

用户看得见的每一句话，都是写给从没编过程的人的。不用黑话，不给栈追踪，不露操作系统的原始
报错。一句没说清下一步该干嘛的错误提示，不算写完。

屏幕上不写按不动的键，按得动的键也不许不写。

不用 emoji 当图标。

按键分支里永远不要 `continue`。它会跳过循环末尾，而那儿正是清理陈旧状态消息的地方。
这个坑我们已经踩过一次：`e0ba1ec`，一句再普通不过的「已切到 X」，把屏幕上唯一告诉用户
怎么退出的那行给盖掉了。

</details>

## 许可

[MIT](LICENSE)

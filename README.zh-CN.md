<div align="center">

# dct

### 关掉笔记本，它们继续干活。

一块 agent 看板。开几个 coding agent 各干各的，关掉终端；回来发现哪个搞砸了，按一个键就退回去。

![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-b7410e?style=flat-square)
![macOS · Linux · Windows](https://img.shields.io/badge/macOS%20·%20Linux%20·%20Windows-005f87?style=flat-square)
![version 0.2.2](https://img.shields.io/badge/version-0.2.2-444?style=flat-square)

[English](README.md) · 设计记录在 [`docs/superpowers/specs/`](docs/superpowers/specs/)

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

## 装上它

**macOS · Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.sh | sh
```

**Windows** —— 原生，不用 WSL。在 PowerShell 里：

```
irm https://raw.githubusercontent.com/gaolei8888/dc-terminal/main/scripts/install.ps1 | iex
```

装下来的是一个几 MB 的可执行文件。**不需要 Rust，不需要编译器，不需要等编译。**
装完新开一个终端窗口，进到任何一个文件夹里敲 `dct`。

`dct` 每轮对话前会给你的项目拍一张隐藏快照，那是靠 `git` 做的——没有它，撤销就是死的，
而撤销正是 `dct` 敢让 agent 关掉所有权限确认的全部理由。所以 Windows 上如果这台电脑
还没有 git，安装脚本会顺手装一份便携版（45 MB，解压即用，整个躺在 `dct` 自己的目录
下面，不写注册表也不碰系统里已有的东西）。macOS 和 Linux 上 git 通常已经有了，
真没有的话脚本会告诉你该敲哪一条命令。

<details>
<summary>教室的网连不上 GitHub 怎么办</summary>

<br>

把 release 里那几个包和 `SHA256SUMS` 原样放到任何一个学生下得到的地方，然后告诉他们
先设一个环境变量。**学生那条安装命令一个字都不用改**，校验和照样会验。

```sh
export DCT_RELEASE_BASE=https://你的地址/dct
curl -fsSL https://你的地址/install.sh | sh
```

```
$env:DCT_RELEASE_BASE = 'https://你的地址/dct'
irm https://你的地址/install.ps1 | iex
```

Windows 上那份便携 git 也一样，用 `DCT_MINGIT_URL` 换地址。

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

agent 只在 git 仓库里跑——撤销就是从那儿来的。你选的项目还不是仓库的话，选 agent 那一屏
在你动手之前就说明白，按 `g` 当场建一个。

---

## 会话活得比终端久

第一次跑 `dct` 会拉起一个后台守护进程。**这个守护进程才是产品本体。** 关掉终端窗口、
合上电脑、第二天回来，会话都还在原地跑，`dct` 本身只是重新连上去看一眼。

看板上可以同时挂好几个 agent，各自待在自己的项目目录里，互不打扰。

---

## 九个 agent，一个入口

按 `N` 会把九个全列出来，**包括这台机器上跑不了的**。跑不了的置灰并写清楚为什么，
而且选中它是带你去解决，不是把你打发走：

- 没装的，`dct` 开一个会话把安装命令跑起来，你看着它装
- 没填密钥的，弹输入框，旁边给申领页面的地址，`Ctrl+O` 直接打开
- 密钥存盘之前先拿真端点探一下，少复制了一截当场就知道，不用等十秒钟进到会话里对着满屏英文发懵

| | | 需要 |
|---|---|---|
| Claude | Anthropic 官方命令行 | `claude` |
| Codex | OpenAI 官方命令行 | `codex` |
| OpenCode | 开源，能接不少模型 | `opencode` |
| Qwen Code | 阿里通义，独立命令行 | `qwen` |
| Kimi | 月之暗面，套着 Claude 的壳 | `claude` + 一份密钥 |
| GLM | 智谱，同样的路子 | `claude` + 一份密钥 |
| DeepSeek | 同样的路子 | `claude` + 一份密钥 |
| Qwen API | 同样的路子 | `claude` + 一份密钥 |
| 命令行 | 就是个 shell | |

后面四个根本不是独立程序，是把 `claude` 的地址指到别家的 Anthropic 兼容端点上，
所以它们既要那个二进制，又要一份自己的密钥。

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

<details>
<summary>底栏、九宫格，以及会话里给你留了什么</summary>

<br>

底栏只有一行，装不下的键不会跟着窗口宽度忽隐忽现——挤不进去的那些全在 `?` 后面那一屏，
而 `? …` 这扇门永远在行尾。那一屏只列**此刻真的按得动**的键：只有一个项目时不写 `Tab`，
组里还有会话时不写 `x`。

底栏中段是当前项目，从底栏自己的配色里反白出来，所以它不会被当成又一个键名。

格子只读——方向键移动焦点，`F3` 跟 `→` 效果一样（下一格，停掉的会话也算），`Enter` 放大进
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
唯一留下的几个键：`F2` 退回看板，`F3` 直接跳到下一个还在跑的会话，`F4` 切换复制模式，
`F5` 粘贴图片，`F6` 挑配色。底栏左边那句「F2 回看板」一直在，断连、报错、消息再长都挤不掉
它——那是会话里唯一的出口。

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

## 用手机看

设置页里有一项「用手机看」。打开它，dct 会在终端里画一个二维码；用**同一个
WiFi 下**的手机扫一下，就能看到你的会话、每个会话的实时画面，以及一个能打字
的输入框。

那是守护进程在你自己网络上发的一个网页。**没有任何东西经过服务器**——压根
没有服务器——所以完全不联网也能用，而出了门就用不了。让手机在任何网络下都
够得着是另一件事，设计写好了但还没做：见
[`docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`](docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md)。

- **第一次打开时系统会问你允不允许**，选「允许·专用网络」，否则手机连不上。
  这句话在你按下那个开关**之前**就写在屏幕上。
- 令牌放在网址的 fragment 里，所以它只进二维码，不进屏幕上那行地址——屏幕会被
  拍照、被投影、被录进屏幕录像，而看过那行字的人就能往你的终端里敲字。
- 手机**不会改终端尺寸**。一个 PTY 只有一个尺寸，两个客户端抢它会让 agent 的
  画面在桌面那边也跟着重排；手机是把整块画布缩到屏幕宽度。
- 页面一转到后台就不再发任何请求，手机揣在兜里不会每秒问你的电脑三次。
- 同一个网络里拿到令牌的人就能往你的会话里敲字。它默认关着，关掉也只要一个键。

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

<details>
<summary><b>方向</b> —— 下面这些一行代码都还没写</summary>

<br>

放在这儿是为了说清楚上面那些东西是冲着什么去的。

真正想要的不是「在任何地方看终端」，而是**人不在，开发照样往前走**。你只管三件事：
提目标、拍板、验收。中间的理解、写、测、修，不该需要你盯着。

- **agent 主动找你，而不是停在那儿等。** 一个 `ask_human` 工具，agent 一调就阻塞，
  问题送到手机上，你的回答作为工具返回值送回去，它接着干。
- **手机通道。** 见 `docs/superpowers/specs/2026-08-23-dc-terminal-srv-design.md`：
  国内没有 Telegram，短信在监管上做不成客户端，所以走一个自己的中转服务
  （端到端加密，只服务 dc_classroom 上注册的用户），手机网页当客户端。
- **只有一套消息格式。** 出站永远是一句话加编号带标签的选项，入站永远是自由文本。
  这套约束来自语音场景：问题要能被念出来，回答是「就第二个吧」而不是 `2`。
- **任务取代会话成为主角。** 你说「修复移动端登录后白屏」，而不是先选 PTY、选 agent、选目录。
- **`dc_llm` 常驻干便宜的活**：判断状态、压缩上下文、归类你的回复、把技术细节写成手机上
  能读的决策卡片。贵的前沿模型只在真正写代码的时候才叫。
- **跑完测试才算完成。** 自动识别技术栈和测试命令，自动跑，失败了让 agent 在限定轮次内
  自己修，通过了才推给你验收。

</details>

<details>
<summary><b>给改代码的人</b></summary>

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
src/proto.rs       线上契约
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

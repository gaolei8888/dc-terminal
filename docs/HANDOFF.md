# 交接：dct「摸索并学会流程」分支的现状与下一步

**新会话先读它。** 写于 2026-10-11，对应分支 `explore-and-learn`（worktree `~/work/dc/dc-terminal-explore`，从 `430d982` 分出）。只在本地提交，没推。

权威文档（这里只给指路，不抄内容）：
- 设计：dc-octo `docs/superpowers/specs/2026-10-11-explore-and-learn-procedures-design.md`
- 计划（D1–D20、V1、O1、O2）：dc-octo `docs/superpowers/plans/2026-10-11-explore-and-learn-procedures.md`
- 执行台账（每个任务的审查结果、裁决 R1–R18、推迟的小问题）：本仓库 `.superpowers/sdd/2026-10-11-explore-and-learn-procedures/progress.md`；各任务报告 `task-*-report.md`、最终修复报告 `final-fix-report.md` 在同一目录
- Jeff 后端和训练题：`docs/superpowers/specs/2026-09-26-dct-judge-design.md` 最后几节
- dct 自己训练 Jeff：`docs/superpowers/specs/2026-10-11-dct-jeff-auto-training-design.md`
- 用户怎么用：`README.md` / `README.zh-CN.md` 的「Post to a social platform / 发到自媒体平台」一节

## 一句话

`dct do` 让 dct 经 dco 把**用户批过的文字**发到一个自媒体平台：dcv 里有用户确认过的流程就让 dco 重放；没有就一步步摸索，发出去以后把步骤写成流程草稿交给 dcv，给用户看卡片、按指纹批准。**还没在真屏幕上跑过**，全部是对着假 dco 测的。

## 现状

### 测试和检查（最新提交 `d356e8d`，台账里记的控制者全量检查）

- `cargo test --workspace -- --test-threads=1`：**2429 通过 / 0 失败 / 5 忽略**。第 5 个忽略的是 `src/procedures.rs` 里对真 dcv 的格式测试，要本机装了带 V1（`learned_from: run:<id>`）的 dcv 才能跑。
- `cargo clippy --all-targets`：没有警告。
- `cargo check --workspace --target x86_64-pc-windows-msvc`：只有一个分支前就有的警告（`src/student_projects.rs` 里没用到的 `path`）。
- 整个仓库的 `cargo fmt --check` 在分支开始前就失败（55 个文件，rustfmt 版本漂移），**不是这个分支造成的**；这个分支新建的文件都是 rustfmt 干净的（裁决 R1）。
- cargo 在 `~/.cargo/bin`，不在默认 PATH 里。

### 做完了什么（都审查通过）

计划里的 D1–D3、D5–D16、D19，用户加的国内平台行，最终审查后的修复，以及之后用户定的三个跟进：A 两段式发布、B 活动文字要指纹、C 训练题第二版。没做的见下面「要用户在场」和「下一步」。

### 按模块看新加了什么

**crates/dct-dco**：从 `dct game` 里抽出来的 dco 客户端（unix socket 上的 MCP），`dct game` 和 `dct do` 共用。只在 unix 上编，Windows 照样能构建。dco 报错时带上它给的运行编号，好取消停在私人画面上的运行。

**crates/dct-explore**（摸索的核心，全部经 `Hands` 接口，对着 `FakeHands` 测）：
- `hands.rs`：`Hands` 接口和 `DcoHands`（真 dco）。记住每个窗口的大小，把求助里的元素框换成窗口里的比例位置（裁决 R7）。
- `platform.rs` + `platforms.toml`：平台表是数据，见下面「平台表」。
- `gate.rs`：停下的规矩（登录、二维码、验证码、手机验证、账号警告）、永远不点的词、上限（40 步、10 分钟、连续 3 个画面没变）。
- `options.rs`：把画面变成一道选择题。参数在画面上出现了才给「发布」选项；先选文件后填字；禁点的过滤掉；发给 Jeff 的最多 20 个。
- `ladder.rs`：Jeff → 大模型 → 用户，见下面「谁来答」。
- `explore.rs` + `explore/`：摸索循环。文件开头的注释是逐条说明（1–11 条），改之前先读。两件要知道的：
  - **画面上的字分「用户自己的值」和「界面的字」**（裁决 R14，`explore/scrub.rs`、`explore/raw.rs`）：用户要发的文字出现在画面上，不算登录墙、不触发禁点词，也不当按钮点；界面的字按原文判断停不停、能不能点，发出去（给模型、写盘）之前把参数值换回 `{名字}`。
  - **两段式发布**（跟进 A，裁决 R17 的修复）：点了发布以后，新画面上**没有能点的发布按钮了**、或者出现了平台表里**核实过的**「发出去了」那行字，才算发出去；否则记成「等确认」，接着走，确认框上点什么都先问用户。
- `trace.rs`：每次运行的轨迹和训练题，见下面「训练题」。
- `draft.rs`：把轨迹变成 dcv 的流程草稿（登录闸放第二步、同一个字段最后一次填的算、关弹窗变成弹窗规则草稿、可能只偶尔出现的步骤加标记；名字用 `-` 不用 `_`，dcv 才收）。
- `replay.rs`：重放和修补。只重放 `executable == true` 的流程；dco 求助时用同一个阶梯答（同一次运行最多答 5 次）；`--preview` 和「只能用户自己点发布」的平台用 `stop_before_step` 停在第一个发布那步前；流程本身走不通 → 从卡住的那步接着摸索，写成新草稿（`draft_of` 原名），原来确认过的不动。

**根目录 src/**：
- `src/judge/`：`dct judge [文件]` 给 `[judge]` 后端发一道 Jev 选择题，打印每个选项的概率；`dct judge --probe` 发一道固定题看通不通。把握由 dct 从概率重算，不信回复里的；选项顺序原样发。
- `src/campaign.rs`：`dct campaign approve <名字> --param <平台>.<参数>=<值>…`。终端里本人看过、敲 `y`、**再按 Touch ID**，指纹签的就是这些确切的文字（跟进 B）。文件被手改过、或旧版本没有签名的，`dct do` 拒绝（退出码 2），要重新批。各平台的文字和开头那句不能相同。
- `src/explore_model.rs`：阶梯中间的「大模型」一级，走现有 `[llm]`。只发元素清单的字（不发图），要求严格 JSON，读不懂就当它说卡住了。同一个实现也负责修被 dcv 拒收的草稿。
- `src/learn.rs`：把草稿写进 dcv、出确认卡片、批准。
- `src/explore_cli.rs` + `src/explore_cli/`：`dct do "<目标>" --platform <平台> --campaign <名字> [--window <窗口号>] [--preview] [--json]`。
  - 退出码：0 发了或试跑停下了；1 出错（dco、dcv、批准）；2 命令 / 配置 / 活动 / 窗口不对；3 停下了或用户说不。
  - `--json`：一行一个事件（`ask`、`confirm_publish`、`card`、`note`，最后是带 `exit` 的 `done` / `previewed` / `stopped` / `declined` / `error`），从标准输入读 `{"key": "e3"}`、`{"stop": true}`、`{"verdict": "yes"}`（给 dc.ai 用）。
  - 开着好几个浏览器窗口时列出来，让用户用 `--window` 指定。

**流程批准的路**：卡片列出 dcv 实际收下的每一行（看不见的字符、控制字符一律拒绝）→ 用户说是 → 先核对 dcv 里的流程和弹窗草稿就是卡片上那一版（正文指纹）→ Touch ID → `dcv approve <库:名字> --sha <指纹>`；流程批准成功后才批准弹窗规则、`confirm_learned` 学到的按钮。任何一步失败，什么都不生效。用户说不：学到的按钮标成拒绝，草稿留在 dcv 里不动。

### dct 怎么和 dco、dcv 说话

**dco**（只做看、做、等、按流程执行；**不调模型、不存流程**）：
- 永远按 `window_id` 指定窗口；`list_windows` 找窗口，`see` 看画面（要编号图时带 marks）。
- 摸索一步一步用 `run_step`（封闭动作词表）；重放用 `run_procedure`，试跑带 `stop_before_step`。
- dco 求助时用 `answer_run` 回：`element_id` + `by`（`user` 或 `model`，没字的图标要带 `name`，关弹窗带 `dismiss`），或 `none`（不在画面上），或 `give_up`。**Jeff 的回答也按 `by: model` 发**（dco 只认这两种）。
- 用户批准流程后，`confirm_learned` 确认求助时学到的按钮。

**dcv**：
- 草稿永远带 `--source import:agent`，`learned_from: run:<运行编号>`；修补的草稿带 `draft_of` 原名。dcv 批准一个 `draft_of` 草稿时会替换掉原来的流程（最终审查核实过）。
- 关过的弹窗写成弹窗规则草稿。
- dcv 拒收就把原因交给大模型改，最多 3 轮。
- 批准只走 `dcv approve --sha`，而且只在用户按了指纹之后。**agent 写的草稿永远不给自己批准。**

### 谁来答（Jeff → 大模型 → 用户）

- Jeff：`answer_confidence` ≥ `[judge] threshold`（默认 0.8）才算数。没配、没答、把握不够、或它选的那下没让画面变 → 往上。
- 大模型（`[llm]`）：同一个画面最多 2 次。
- 用户：永远是最后一级，带一张编号图。
- **危险与否只看规则**（停下的规矩、禁点词、dco 的按钮分档），从不看任何模型的把握。每次点发布前都先问用户。

### 配置

`~/.dct/config.toml`：
- `[judge]`：`base_url`（如 `http://127.0.0.1:8765`）、`model`（默认 `jeff-latest`）、`api_key_env`（有密钥时明文 http 只发本机）、`threshold`（默认 0.8）、`timeout_ms`（默认 5000）。不写就跳过 Jeff。
- `jeff-serve` 要**人手动起**，dct 不起它、不下权重。目前用的权重：Jeff v1.2，`jeff-legacy/Jeff-Qwen3.5-0.8B`，revision `d86f657`，约 1.6 GB，`max_options` 254。
- `[llm]`：现在是 `claude -p`（后备）。换成 qwen3.8-27b 的任务 D20 挂起，等用户给地址、认证方式、能不能看图。不写 `[llm]` 就跳过大模型、草稿也不修。

### 平台表

- 内置 16 行（`crates/dct-explore/src/platforms.toml`）：x、tiktok、youtube、facebook、instagram、xiaohongshu，加国内的 weibo、bilibili、zhihu、zhihu_article、douyin、kuaishou、wechat_channels、toutiao、wechat_mp、baijiahao。
- 每行的 `unverified` 列出没在真屏幕上核对过的字段。国内平台几乎全是（发布词「发布」只是占位），小红书的发布词也没核实；发布词没核实的平台，摸索时每点一下都先问。
- `login_qr`：扫码登录的平台（只是提示，碰到登录 / 二维码画面本来就停下）。`publish_by_user_only`（目前只有 wechat_mp）：dct 永远不点最后的发布，用户自己点。
- 用户可以在 `~/.dct/platforms.toml` 里改或加，同名的整行替换内置的。
- **X 的 `login_gate` 和 `publish_done` 都没核实**。所以真跑 X 时，第一次点 Post 以后会一直是「等确认」（X 发完回首页，左栏还有 Post），要用户说发出去了没有，直到 `publish_done` 在真机上核实、从 `unverified` 里去掉。

### 训练题和轨迹

- 轨迹：`~/.dct/explore/runs/<运行编号>/trace.json`，连同给用户看过的编号图；记着这次的结局。
- 训练题：`~/.dct/explore/answers.jsonl`，每答一道题一行。都是 0600，要发的文字写盘前换成 `{post_text}` 这样的名字。
- 第二版格式（`v: 2`，**运行结束时才写**）：原有的 `by`（jeff / model / user）、`corrects_jeff`，加上 `jeff_asked`、`shown`（发给 Jeff 的选项）、`step`（对应轨迹里哪一步）、`run_outcome`、`outcome`（good / bad / unknown）、`label`。
- 标签规矩（`trace::grade`）：用户的答案永远算；大模型的只有 `outcome` 是 good（这次真发出去了、这一步留在最终路线上）才算；Jeff 自己的永远不算。用户最后说不发的运行，大模型的题都是 unknown、不算标签（裁决 R18）。
- 详细格式见自训练设计第 1 节。

### 留给人看的残余风险（台账里记的，没修）

- 发布后弹出的对话框把原来的发布按钮盖住了，会被当成「已经发出去」（要靠平台表里核实 `publish_done` 来补）。
- 活动文字：把旧的、当时签过名的那份文件拷回来，会被当成有效（撤销需要一个 `~/.dct` 以外的锚点）。
- 修补后，批准记录存在修补草稿的名字下；按原名走 dct 执行票的地方要再批一次（是拒绝那一侧的失败，不会误放行）。
- dct 在运行中途被杀掉，这次的训练题就丢了（题在运行结束才写）。
- 其余推迟的小问题和 R1–R18 每条裁决：见 `progress.md`。

## 要用户在场才能做的（真屏幕）

计划里的两个试点，步骤和完成标准在 dc-octo 的计划文件里：
- **D17 X 发帖**：批活动文字 → 第一次摸索（记下每步谁答的、Jeff 把握）→ 用户点头发出 → 卡片确认按指纹 → 换文字第二次应当零提问重放。顺带核对 X 的 `login_gate` / `publish_done` 真实文字，确认后单独一个提交从 `unverified` 去掉。
- **D18 TikTok 上传视频**：验证先文件后文字、弹窗规则草稿、「可能只偶尔出现」的标记，同样核对平台字段。
- 前提：下面「合并前」的 dcv V1、dco O1/O2 都装好；`jeff-serve` 在跑；Chrome 已登录。

## 合并前要注意

- **dcv 要装带 V1 的版本**（接受 `learned_from: run:<id>`），那个 `#[ignore]` 的真 dcv 测试才能跑；跑过再合。
- **dco 要装带 O1/O2 的版本**（`stop_before_step`、`run_step`），任何真跑之前必须先装。
- **main（`~/work/dc/dc-terminal`）上有别人没提交的改动**（`src/daemon.rs`、`src/pty.rs`、`src/session.rs`、`src/ui/` 下几个文件、`tests/concurrency.rs`）。合并时小心，别覆盖掉。
- `cargo fmt --check` 全仓库失败是分支前就有的，别把一次全仓库格式化混进合并。

## 下一步

1. 真屏幕试点 D17、D18（上面）。
2. **dct 自己训练 Jeff**：设计 `docs/superpowers/specs/2026-10-11-dct-jeff-auto-training-design.md`。任务 1（训练题第二版）已做；其余任务没开始，第 8 节有待用户定的问题。
3. **Jeff 控制变量重跑**：在 dc-octo `docs/experiments` 里进行中。两次 E1 冒烟的数互相冲突，出结果前不当「adapter 比现役好」的依据（见自训练设计第 4 节「现在的基线」）。
4. **D20**：大模型换成 qwen3.8-27b，等用户给地址。

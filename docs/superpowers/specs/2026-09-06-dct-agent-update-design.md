# dct 提示 agent 有新版本、一键更新 —— 设计

**状态：** 已定，可以照着实施。设计在对话里确认过（2026-09-06）。
**起因：** 2026-09-06 用户问「dc-terminal 能不能在 agent 版本旧了的时候自动更新？」，
答案是「今天不能」——`dct install` 只在命令找不到的时候装一次，装完之后
再也不看版本。用户定了范围：**自动检查，一键更新**；不自动替用户升。
随后又补了一条：**「你要告诉别人怎么装」**——功能做完，README 要写清楚怎么装、
怎么更新；界面上遇到 dct 管不了的装法，也要告诉用户自己该怎么办，不能一声不吭。
**关联：**
- `src/cli.rs::run_install` —— 今天的安装路径，这份设计在它上面加一条 `--update`。
- `src/runtime.rs` —— 自带 Node 的来历；里面「钉死版本」那段注释是这份设计
  **不做自动升级**的理由之一。
- `src/daemon.rs::Request::Profiles` —— 「装没装」这个判断在守护进程里做，
  「新不新」也得在同一个地方做，理由一样（见 `profile.rs::command_exists`）。

---

## 问题

看板上十个 agent 里能干活的那几个（`claude`、`codex`、`qwen`、`opencode`）都是
npm 装出来的，而这些 CLI 发版很勤——Claude Code 一周几个版本。学生装完之后
就停在那一版上，直到某天遇到一个新版本才修好的问题，或者一个老版本才有的报错。

今天 dct 对这件事一无所知：

- `dct install <agent>` 看到命令找得到就说「已经装好了」，退出码 0，不看版本。
- 界面上没有任何地方显示 agent 的版本，更没有「有新版本」。
- 这几个 CLI 自己会印「new version available」，Claude Code 甚至会自己尝试升级。
  但那些话出现在**会话里面**，混在 agent 的输出中，而且是英文——对小白来说
  跟别的英文一样看不见。Claude Code 的自升级在 dct 那份私有 Node 里跑不跑得
  通也没人验过。

**目标：** 学生打开选 agent 的列表，就能看见「这个有新版本」，按一个键升上去。

**不做的：**

- **不自动替用户升。** `runtime.rs` 钉死 Node 版本的理由在这里同样成立：一个
  教室里所有人手上应该是同一份，排查「他那台为什么不行」的时候，第一个要排除
  的就是「他的版本跟别人不一样」。自动升级会让每台机器在不同时刻变成不同版本。
- 不做「自动升级」的配置开关。没人要过，先不做。
- 不管 dct 自己的升级——那是另一件事，走 `scripts/install.sh`。
- 不改手机网页端。那一端只看会话，不选 agent。
- 不动 `profiles/*.toml`。包名从已有的 `[install].command` 里读出来。

---

## 一、哪些 agent 归 dct 管

**规矩：命令能追溯到一份 npm 装出来的包，才算「dct 能更新」。**

守护进程把 profile 的 `command[0]`（比如 `claude`）在 PATH 上解析成真实路径，
然后在旁边找 npm 全局安装的 `package.json`：

| 平台 | 可执行文件在 | `package.json` 在 |
|---|---|---|
| Unix | `<prefix>/bin/claude` | `<prefix>/lib/node_modules/<pkg>/package.json` |
| Windows | `<prefix>/claude.cmd` | `<prefix>/node_modules/<pkg>/package.json` |

找到了，读它的 `version` 字段，这就是「装的是几」。找不到，这个 agent **不归
dct 管**：用官方安装器装的 Claude（`~/.local/bin/claude`）、brew 装的、
用户自己编译的，都属于这种。

这条规矩同时覆盖了两种情形：dct 自带的那份 Node（`~/.dct/runtime/node`），
以及机器上本来就有的 npm（`run_install` 看到系统有 npm 就直接用它，装到系统的
全局前缀去）。两种前缀的目录结构一样，同一段代码都认。

Unix 上 `<prefix>/bin/claude` 是指向 `../lib/node_modules/<pkg>/cli.js` 的符号
链接。**先按上表的相对路径找，找不到再顺着符号链接走**——前者简单，后者兜底
`prefix` 被改过的少见布局。

**包名**从 profile 的 `[install].command` 里读：`npm i -g <pkg>` 这条命令里，
`i`/`install` 之后第一个不以 `-` 开头的参数。`@anthropic-ai/claude-code` 这种
带作用域的名字原样保留。`[install]` 不是 npm 的（将来有人写 brew、pip），
或者根本没有 `[install]`，一律不归 dct 管。

**不归 dct 管的 agent，界面怎么说** （用户那句「告诉别人怎么装」）：

- 命令找得到、但追溯不到 npm 包 → 列表上**不显示**版本和更新提示。这不是
  「装错了」，是「dct 不认识这种装法」，没什么好提醒的。
- 只有一种情况要开口：用户在这样的条目上按了 `u`。这时候一句话说清楚：
  「这份 Claude 不是 dct 装的，请用你原来装它的方式更新」。不猜他用的是什么。

---

## 二、守护进程怎么知道有新版本

**在守护进程里查，不在界面里查。** 理由是仓库里已经定过的那条：「装没装」
必须在子进程真正会拿到的那个 PATH 里问（`profile.rs::command_exists` 的注释）。
「装的是几」是同一个问题的延伸，答案也只能来自同一个环境。

### 查什么

对每个归 dct 管的 agent，向 npm 仓库要一个 JSON：

```
GET <registry>/<pkg>/latest
```

`registry` 取 `DCT_NPM_REGISTRY` 环境变量，没设就是 `https://registry.npmjs.org`
——跟 `run_install` 里 `--registry` 用的是同一个变量，国内课堂设一次两边都生效。
回来的 JSON 里 `version` 字段就是「最新是几」。带作用域的包名里那个 `/`
**不转义**：`registry.npmjs.org` 和 `registry.npmmirror.com` 两边对
`/@anthropic-ai/claude-code/latest` 都直接回答（实施时用 curl 验一次，写进注释）。

用 `pair_http::agent()` 那份 ureq agent 的建法——它已经把建连超时调短了。
整个请求 5 秒超时。

### 什么时候查

- 守护进程起来之后 **30 秒**查一次（不在启动路径上，不拖慢第一个连接）。
- 之后每 **24 小时**一次。
- `dct install <agent>` 和 `dct install <agent> --update` 跑完之后，通过 socket
  发一条 `Request::RecheckUpdates`，让守护进程立刻重查这一个——否则装完之后
  提示还挂着，要等一天才消失。

### 结果放哪

放在守护进程内存里，一张 `BTreeMap<profile 名, UpdateInfo>`，跟 `secrets`、
`phone` 一样是 `Arc<Mutex<…>>` 的共享状态。不落盘：守护进程本来就常驻，重启
之后 30 秒内会重新查到。

```rust
pub struct UpdateInfo {
    pub package: String,     // 例：@anthropic-ai/claude-code
    pub installed: String,   // 例：2.1.251
    pub latest: Option<String>, // None = 还没查到 / 查不到
    pub checked_at: Option<SystemTime>,
}
```

### 查不到怎么办

**查不到就是没有提示，不是错误。** 网不通、仓库 5 秒没回、JSON 不认识、
`version` 字段不像版本号——全部记成 `latest: None`，界面上什么都不显示。
这不是把错误吞掉：一个学生在没网的教室里打开 dct，不该看见一行关于 npm 仓库
的红字，那跟他要做的事毫无关系。守护进程的 stderr 里留一行，够排查用。

### 版本比较

按 semver 比 `major.minor.patch` 三段数字；带预发布后缀（`-beta.1`）的一律不
提示更新——`latest` 标签正常情况下不会指向预发布版，真指向了也不该推给学生。
不引入 `semver` crate，三段数字的比较二十行写完，`Cargo.lock` 不用长胖。

---

## 三、界面：选 agent 的列表

`ProfileEntry`（`proto.rs`）加一个字段：

```rust
/// 这个 agent 有没有新版本。`None` = 不归 dct 管，或者还没查到，或者已经是最新。
/// 三种情况界面上都一样：什么都不显示。
#[serde(default)]
pub update: Option<UpdatePrompt>,

pub struct UpdatePrompt {
    pub installed: String,
    pub latest: String,
}
```

守护进程只在 `latest > installed` 的时候填它。「已经是最新」跟「不知道」在
界面上没有区别，所以不往协议里塞第三种状态。

**画法：** 列表里那一行，状态文字后面接一句：

> 有新版本 2.1.251 → 2.1.260，按 u 更新

英文：`Update available 2.1.251 → 2.1.260, press u`。用 `dim` 样式，不抢
「没装」「没密钥」那些真正挡路的状态的眼。

**按键：** `u`。只在选 agent 的列表（`View::PickProfile`）里生效。

- 高亮的条目带 `update` → 开一个 shell 会话跑 `dct install <agent> --update`，
  跟今天「没装 → 回车 → 开窗口跑 `dct install`」一模一样的路（`PickAction::Install`
  那条），也同样 `remember: false`。
- 高亮的条目不带 `update` 但状态是 `Ready` → 一句话：「Claude 已经是最新的了」
  或者「这份 Claude 不是 dct 装的，请用你原来装它的方式更新」。两句话怎么分：
  `ProfileEntry` 再带一个 `managed: bool`（守护进程追溯到 npm 包就是 true）。
- 其他状态（没装、缺依赖、缺密钥）→ 不响应。那些状态各有各的出路，`u` 不该
  抢它们的活。

`pick_action` 是纯函数，`u` 的判断也抽成纯函数 `update_action(e, lang)`，
返回 `Install` / `Blocked(一句话)` / `Nothing`，跟 `pick_action` 并排放着，
一起单测。

底部的按键提示行加 `u 更新`——只在当前高亮条目带 `update` 时显示，
不然一直挂着一个按了没反应的键。

---

## 四、`dct install <agent> --update`

在 `run_install` 里加一个分支。今天的流程是「命令找得到就说装好了、退出」；
`--update` 把这一步换成：

1. **问守护进程要会话列表**（`Request::List`）。这个 profile 有活着的会话 →
   拒绝，退出码 1：「先关掉正在跑的 2 个 Claude 会话再更新」。理由：正在跑的
   node 进程底下换文件，Qwen Code 这种几百个文件的包会在下一次 `require`
   的时候读到新旧混杂的东西。守护进程连不上（没起）→ 没有会话，放行。
2. 追溯 npm 包（第一节那段逻辑，跟守护进程共用 `profile.rs` 里同一个函数）。
   追溯不到 → 退出码 1，印那句「不是 dct 装的」。
3. 跑 `npm i -g <pkg>@latest`，`--registry` 照旧。缺 npm 的话走 `ensure_node`，
   跟安装路径一样。
4. 跑完**再追溯一次**读版本，印「Claude 已更新到 2.1.260」。npm 说成功但版本没变
   （仓库上就是这一版）→ 印「Claude 已经是最新的了」，退出码 0。
5. 发 `Request::RecheckUpdates { profile }` 让守护进程刷新，发不出去不算错。

普通的 `dct install <agent>`（不带 `--update`）装完也发第 5 步那条——今天它
装完就退，守护进程要等一天才知道这个 agent 现在的版本。

`--update` 只认这一个参数；`dct install claude --foo` 是用法错误，跟
`parse_restart_args` 一个脾气。

---

## 五、README

中英两份 README 的「十个 agent，一个入口」一节各加一段，说清楚三件事：

1. 列表上看到「有新版本」怎么办——按 `u`。
2. 不进界面怎么办——`dct install claude --update`。
3. **哪些 agent dct 管不了**：不是 dct（也不是 npm）装的那份，比如官方安装器
   装的 Claude，dct 看不见它的版本、也不会去碰它，更新还是用原来装它的方式。

「会踩到的坑」一节加一条：**更新的时候那个 agent 不能有会话在跑**，dct 会拒绝
并告诉你关掉哪几个。

---

## 测试

纯函数各自单测，不碰网络、不碰真实 PATH：

- `npm_package_of(&InstallSpec)`：`npm i -g @a/b` → `@a/b`；`npm install -g x --foo` → `x`；
  `brew install x` → `None`；空命令 → `None`。
- `installed_version(exe_path)`：临时目录里摆出 Unix 和 Windows 两种布局各一份
  `package.json`，读得出版本；没有 `package.json` → `None`；`version` 字段缺失 → `None`。
- `newer(installed, latest)`：`1.2.3 < 1.2.10`、`1.9.0 < 1.10.0`、相等不提示、
  `latest` 带 `-beta` 不提示、任一边不是三段数字不提示。
- `parse_latest(json)`：正常 JSON 取 `version`；缺字段 / 不是 JSON → `None`。
- `update_action(entry, lang)`：带 `update` → `Install`；`Ready` 且 `managed` →
  「已经是最新」；`Ready` 且不 `managed` → 「不是 dct 装的」；`NotInstalled` → `Nothing`。
- `run_install` 的参数解析：`--update` 认，`--foo` 用法错误。

一条走通的测试：`daemon::run_with_manager` 那套测试夹具里，给 `check_updates`
注入一个假的「取 latest」闭包（返回固定字符串或 `Err`），确认 `Profiles`
响应里 `update` 字段该有的时候有、该没的时候没。**不在测试里发真实 HTTP。**

`curl` 对两个仓库的 `/<scoped pkg>/latest` 各验一次，结果写进 `update.rs` 头注释，
跟 `runtime.rs` 「三个已经验过的事实」一个写法。

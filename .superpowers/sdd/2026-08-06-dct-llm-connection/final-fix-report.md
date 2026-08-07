# 最终整分支 code review —— 一轮修复报告

分支 `feat/llm-connection`，工作区 `.claude/worktrees/llm-connection`。
本轮把 review 的全部 6 条（CRITICAL 1a/1b、IMPORTANT 2、IMPORTANT 3、
FIX NOW 4、FIX NOW 5，外加 clippy 一条）一次做完。

提交（新→旧）：

| SHA | 内容 |
|---|---|
| `ab55b2f` | 1(b) 目的地检查 + 2 i18n + 3 守护进程警告 + clippy |
| `c1ba83c` | 5 设计文档订正 |
| `3349b77` | 4 `extract_text` 挑第一个 text 块 |
| `4da3ee1` | 1(a) 删掉四个 profile 编出来的 `[headless]` |

全套结果：**516 passed / 0 failed**（481 lib + 35 分布在 14 个集成二进制里；
doc-test 0）。修改前的基线是 504（469 + 35）——本轮净增 12 条测试，
一条既有测试都没删、没放宽。

跑过的命令（每一条都以 `export PATH="$HOME/.cargo/bin:$PATH"` 开头）：

```
cargo fmt
cargo build
cargo test
cargo test --no-run
cargo clippy --all-targets
git diff --check
```

`cargo test` 逐个二进制的结果：

```
unittests src/lib.rs      481 passed; 0 failed
unittests src/main.rs       0 passed; 0 failed
tests/cli.rs                9 passed; 0 failed
tests/client_timeout.rs     1 passed; 0 failed
tests/concurrency.rs        1 passed; 0 failed
tests/daemon_detach.rs      1 passed; 0 failed
tests/daemon_roundtrip.rs   3 passed; 0 failed
tests/daemon_upgrade.rs     3 passed; 0 failed
tests/grid_reply.rs         2 passed; 0 failed
tests/profiles_flow.rs      5 passed; 0 failed
tests/projects_flow.rs      3 passed; 0 failed
tests/screen_state.rs       2 passed; 0 failed
tests/signal_restore.rs     2 passed; 0 failed
tests/slow_input.rs         1 passed; 0 failed
tests/socket_perms.rs       1 passed; 0 failed
tests/zombie_reaping.rs     1 passed; 0 failed
Doc-tests dct               0 passed; 0 failed
```

`cargo clippy --all-targets`、`cargo test --no-run`、`git diff --check` 均无
任何 warning / error 输出。

---

## CRITICAL 1(a)：四个 profile 编出来的 `[headless]`

**改了什么。** `profiles/kimi.toml`、`glm.toml`、`deepseek.toml`、
`qwen-api.toml` 各删掉

```toml
[headless]
command = ["claude", "-p"]
```

这四个是 API 密钥形态的厂商，它们的正路是已经写好的 `[api]` + HTTP 直连。

**覆盖测试。** `src/profile.rs::unverified_clis_declare_no_headless_command`
——原来只盖 `opencode` / `qwen`，现在六个一起盖，并且在文档注释和断言消息
里把规矩说清楚了：没有实测过非交互模式就不许写 `[headless]`；这四个尤其
不许，因为它们的 `[headless]` 会让子进程去借另一家的登录态。

**副作用（已处理）。** 删掉之后，`[llm] provider = "kimi"`（transport 默认
`cli`）会落到 `NoHeadlessCommand` 上。那句话原本只说「换成 claude」——对一个
已经买了 kimi 额度、填好密钥的用户等于让他换一家。现在这句话同时给出他那条
正路：「要么换成 claude，要么给『kimi』打开『直连』并填上型号名」。
（`直连` 是本项目对 transport=http 已有的用户措辞，见 `NoApiEndpoint`。）

---

## CRITICAL 1(b)：把这一类关掉，而不是关掉这一个实例

### 检查是怎么工作的

1. **凭据带出处。** `src/llm/creds.rs` 新增

   ```rust
   pub enum BorrowedFrom { ClaudeCli, CodexCli }
   pub type Borrowed = (BorrowedFrom, Credential);
   ```

   OAuth 查询（`cli.rs::oauth_lookup`、`daemon.rs::startup_oauth`）现在返回
   `Option<Borrowed>` 而不是 `Option<Credential>`。

2. **用户自己填的 key 结构上就不在管辖范围内。**
   `resolve::select_credential` 里密钥仓那一路 **直接返回 `Credential`**，
   压根构造不出 `Borrowed`；只有 OAuth 那一路带出处。所以「用户填的 key 不受
   限制」不是一个 `if`，是类型层面的事实——没人能忘记写它。

3. **判目的地主机，不判 profile 名。** `resolve::resolve` 的 HTTP 分支先算
   `base = cfg.llm.base_url.unwrap_or(api.base_url)`，再用
   `creds::host_of(&base)` 抠出主机，然后才去要凭据：

   ```rust
   let host = host_of(&base).ok_or(BadBaseUrl { .. })?;
   let cred = select_credential(name, &host, secrets, oauth)?;
   ```

   `select_credential` 只在 `from.may_reach(host)` 成立时才交出借来的凭据，
   否则返回 `BorrowedCredentialRefused { name, host }`。

4. **`may_reach` 的规则。** ClaudeCli → `anthropic.com` 及其子域；CodexCli →
   `openai.com`、`chatgpt.com` 及其子域（ChatGPT 后端也是 OpenAI 自己在运营，
   codex 的 SSO token 本来就是发给它的）。后缀比较**连着点一起比**
   （`.anthropic.com`），所以 `evil-anthropic.com`、`anthropic.com.example.cn`
   都不算自己人。

5. **看不懂的地址一律不发。** `host_of` 要求有 `scheme://`，剥掉路径/查询/
   锚点、剥掉 `user:pass@`（`https://api.anthropic.com@collector.example/`
   的主机是 `collector.example`，这一手挡的就是它）、剥掉端口、支持 IPv6
   字面量，判不出来返回 `None` → `BadBaseUrl`，一个字节都不发。

6. **CLI transport 这条路（env 变体）。** 同一类问题在这条路上不是长在
   header 上而是长在环境变量上：profile 声明了 `[secret]`，我们却一个都不注入
   时，那个 CLI 会转头读**它自己的**登录态，而 `[env] ANTHROPIC_BASE_URL`
   已经把它指向第三方。现在 `resolve` 的 Cli 分支走
   `headless_env(&p, name, secrets)`：声明了 `[secret]` 就必须有密钥（注到
   `[secret].env` 指定的变量上，跟 `session.rs` 起交互式会话完全一样），
   没有就 `NoCredential`，绝不让子进程自己去凑。

7. **按名字那道关留着。** `oauth_lookup` / `startup_oauth` 仍然只把
   `claude`/`codex` 映射到各自的登录态，其余一律 `None`。它是纵深的第一道，
   不是被替换掉的。

### 覆盖了什么

- 从 Claude Code 借来的 Bearer 发往 moonshot / bigmodel / deepseek /
  dashscope（四个内置 profile 的 `[api]`）。
- 手写 `~/.dct/profiles/claude.toml` 塞一个 `[api]` 指向任意主机。
- `~/.dct/config.toml` 里的 `base_url` 覆盖指向任意主机。
- 从 codex 的 `auth.json` 里读到的凭据（**包括 `OPENAI_API_KEY` 这种 Key**
  ——它同样不是用户填给这个 provider 的，判的是出处不是变体）。
- 无界面子进程借另一家登录态（第 6 点）。
- 地址畸形 / 带 `@` 骗主机名的写法。

### **没有**覆盖什么（诚实的边界）

- **用户自己填进 dct 的 key 不受任何限制。** 这是明确的设计：他把这个 key
  填给了这个 provider，要发到哪里是他的决定。填错地方 dct 不拦。
- **交互式会话（`session.rs::create`）不在此列。** 那条路上注入的一直是用户
  自己填的密钥，从来不碰 Keychain / `auth.json`，本来就不是这一类问题。
- **子进程自己发起的网络请求管不了。** 我们只管「dct 交给它什么」。一个用户
  手写的 profile 如果不声明 `[secret]`、直接用 `[env]` 把 `claude` 指向第三方
  端点，那个 CLI 仍然会拿自己的登录态去打——那是它的进程、它的凭据、它的
  出站连接，dct 在这条路上唯一能做的就是不去主动造出这种 profile（1(a)）
  和不让「声明了要密钥却没有」这种状态跑起来（第 6 点）。
- **不做主机白名单以外的证书/网络层校验。** 判的是「这个凭据允许发给谁」，
  不是「这台机器是不是它自称的那台」。

### 测试

`src/llm/creds.rs`
- `a_borrowed_login_only_reaches_its_own_vendors_hosts` —— 两个出处 × 自己家 /
  别人家 / 骗后缀（`evil-anthropic.com`、`anthropic.com.example.cn`）/ 空串。
- `the_host_is_taken_from_the_url_not_guessed` —— 大小写、端口、IPv6、
  `user@host`、四种畸形地址。

`src/llm/resolve.rs`
- `a_borrowed_login_is_refused_when_the_destination_is_not_its_own` ——
  四个内置厂商 profile 全测。
- `a_hand_written_claude_profile_cannot_aim_the_keychain_token_elsewhere`
  —— 手写 `[api]` + `base_url` 覆盖两个入口。
- `a_key_the_user_typed_in_is_still_allowed_to_that_same_host` —— **同一个
  provider、同一台主机**，借来的被拒、用户自己的 key 照走，而且断言真正被
  带走的是用户那把。
- `an_address_we_cannot_read_gets_no_credential_at_all`
- `a_headless_profile_that_needs_a_key_is_refused_without_one`
- `the_headless_child_gets_the_users_key_in_its_environment`
- `a_headless_profile_without_a_secret_block_is_untouched`
- `http_uses_an_oauth_token_when_there_is_no_key` 改成把目的地覆盖成
  `api.anthropic.com`（原来靠 kimi 的端点通过，那本身就是被修掉的行为）。
- `an_explicit_key_outranks_an_oauth_token_found_elsewhere` 的目的地故意选成
  这份 OAuth 去得了的主机，好让「顺序」仍然是唯一的变量。

**测试一律注入，不读任何真实凭据**：没有测试碰 Keychain、`~/.claude`、
`~/.codex`、真实 `~/.dct`，也没有测试调用 `security`。密钥仓一律用
`tempfile::tempdir()`。

---

## IMPORTANT 2：`describe()` 绕开了 i18n

**改了什么。** 删掉 `resolve::describe`，文案移到
`i18n::msg::llm_problem(lang, &ResolveError)`，跟 `msg::error` /
`msg::warning` 同一套（守护进程报码、界面组句，所以切语言立刻生效）。
`ResolveError` 因此加了 `Serialize`/`Deserialize`——它现在要跟着
`WarningCode::LlmUnavailable` 走 socket。

`llm_check` 加了 `lang: Lang` 参数，`main.rs` 用 `cli_lang()` 传进去，跟
`ps`/`stop`/`kill`/`prune` 完全同一条路。这条命令里剩下的硬编码中文
（「没写 [llm]」「连不上」「通了」「没通」）也一并走了 i18n：
`llm_not_enabled` / `llm_using` / `llm_cannot_connect` / `llm_works` /
`llm_call_failed`。

**路径。** 每一句「去改设置」都带上 `~/.dct/config.toml`
（`i18n::msg::CONFIG_PATH`）。`dct llm check` 更进一步：它印的是**从 socket
真推出来的那个绝对路径**，因为这条命令跑在用户自己的终端里，它知道文件到底
在哪。实跑（`HOME` 指向临时目录，没碰真实 `~/.dct`）：

```
$ HOME=<tmp> DCT_LANG=zh dct llm check
「出错解释」这个功能现在是关着的。
要打开的话，在 <tmp>/.dct/config.toml 里加上这两行：

[llm]
provider = "claude"

加完再跑一次 `dct llm check` 就能验。

$ HOME=<tmp> DCT_LANG=zh dct llm check      # [llm] provider = "kimi"
用的是 kimi，让它自己登录。
连不上：「kimi」还没法自己在后台回答问题。打开设置文件 ~/.dct/config.toml，
要么把这一项换成 claude，要么给「kimi」打开「直连」并填上型号名。

$ HOME=<tmp> DCT_LANG=zh dct llm check      # kimi + 直连 + 型号，没填密钥
用的是 kimi，直接连接。
连不上：「kimi」还没有密钥。在主界面按 c 填一个。
```

**覆盖测试。**
- `resolve::tests::every_reason_explains_itself_in_both_languages_with_a_real_next_step`
  —— 由原来那条中文单语测试升级而来（没有放宽任何断言）：七个变体 × 两种
  语言，禁词表照旧（provider/transport/cli/agent/error，大小写不敏感），
  并且新增「指向改设置的每一条都必须含 `~/.dct/config.toml`」和「被拒那条
  必须点名主机 + 给出按 c」。
- `i18n::tests::every_warning_code_composes_in_both_languages` 补进
  `LlmUnavailable` 的全部七种原因（顺带继承了那条测试原有的守卫：英文里
  不许有汉字、警告不许带换行）。

---

## IMPORTANT 3：守护进程那句警告没人看得见

**选的是「首选方案」，不是兜底方案。** 理由：`client::spawn_daemon` 把
守护进程的 stderr 接到 `/dev/null` 是对的（它和 TUI 共用一个终端，任何一行
都会糊在界面上），所以问题不在那句 `eprintln!`，在于**没有一条通往界面的
路**。而这个仓库已经有那条路——`Response::Profiles { warnings }` +
`ui::join_warnings`，profile 目录读不了、密钥文件坏了都走它。

**改了什么。**
- `proto::WarningCode` 新增 `LlmUnavailable(ResolveError)`（报码不组句，同
  其余各条）。
- `SessionManager` 加 `llm_problem` 槽位 + `set_llm_problem` / `llm_problem`。
  只有用户**确实写了 `[llm]`** 却接不上时才是 `Some`——没写 `[llm]` 是绝大
  多数人的正常状态，那种情况整条路径压根不跑（那条 Critical 隐私边界不动）。
- `install_llm_backend` 成功时清空、失败时记下（stderr 那行留着，`dct daemon`
  前台跑的时候仍然有用）。
- `Request::Profiles` 把它拼在警告列表**末尾**（密钥/profile 那几条是「你现在
  要用的东西坏了」，这条是「一个增强功能没生效」，先后有别）。

**覆盖测试。** `daemon::tests::a_broken_llm_setting_reaches_the_user_instead_of_going_silent`
—— 写一份 `[llm] provider = "根本没有这个"` 的配置，跑
`install_llm_backend`，然后真的发一次 `Request::Profiles`，断言回来的
`warnings` 里有 `LlmUnavailable`，并且 `i18n::msg::warning` 组出来的那句话
点名了写错的那个名字。另外
`a_bare_llm_section_does_install_a_backend` 补了一条「接上了就不该留着一条
抱怨」。

`dct llm check` 依旧是那条精确的诊断命令（它现在印真实路径 + 人话原因），
`--help` 没动：警告已经自己会出现在界面上，再往帮助里塞一段说明反而是让
用户去记一件不需要他记的事。

---

## FIX NOW 4：`extract_text` 在真实 Anthropic 回答上会坏

**改了什么。** `http::extract_text` 的 Anthropic 分支不再取 `content[0]`，
改成扫出**第一个 `type == "text"` 的块**。另加一条窄的兜底：一个带 `type`
的 text 块都没有时，认第一个**带 `text` 字段**的块——有些 Anthropic 兼容的
第三方端点不写 `type`，而 thinking / tool_use 块都没有 `text` 字段，所以这
一步退让不可能把思考过程当成答案端上去。

**覆盖测试。**
- `a_leading_thinking_block_does_not_hide_the_answer` —— thinking 开头、
  tool_use 开头，以及「只有 thinking 没有 text」必须读不出来（绝不猜）。
- `a_block_without_a_type_still_reads_as_text` —— 钉住兜底那一步。
- 既有的 `unreadable_responses_yield_none_for_every_wire`（含
  `{"content":[]}`）和 `extracts_text_from_each_wire_format` 原样保留。

---

## FIX NOW 5：设计文档过期

`docs/superpowers/specs/2026-08-06-dct-llm-connection-design.md` 里
「整段缺失 = 默认用 `claude` profile 的无界面模式」改成「整段缺失 = 这个功能
整个关着」，并把**理由**一起写进去（送 2000 字符终端内容给第三方必须是用户
的一次主动动作），另外明确点名「不要照着旧那句改回去」，并指向
`src/config.rs` 头注释和 `daemon::install_llm_backend`。

---

## 另外：clippy

- `src/llm/creds.rs` 的 `needless_return`（macOS 分支那个 `return
  parse_claude_oauth(...)`）已去掉。
- 顺手删掉 `src/session.rs` 里两处 `let pid = s.pty.process_id();`——上一个
  提交（回收自行退出的 agent）留下的死读取，`cargo clippy` 会报
  `unused_variable`。`process_id()` 是纯 getter，删掉没有行为影响。
  现在 `cargo clippy --all-targets` 零输出。

## 约束核对

- 没加任何 crate 依赖（`Cargo.toml` 未改）。
- 没有 async。
- 没有测试读真实凭据 / 真实 `~/.dct` / Keychain，也没有测试调 `security`。
- 没有删改弱化既有测试（`every_error_explains_itself_...` 是**加强**后改名的
  同一条：从单语变双语，禁词表原样保留，另加两组新断言）。
- 没有用 emoji 当图标。
- 全部 `git add` 都是显式路径。
- 提交信息英文，无 AI/Co-Authored-By 署名。

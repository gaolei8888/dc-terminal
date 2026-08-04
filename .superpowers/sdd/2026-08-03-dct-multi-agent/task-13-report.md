# Task 13 报告：密钥设置页

## 实现内容

- `src/proto.rs`：`ProfileEntry` 加 `has_secret: bool` 字段，daemon 直接从密钥仓查出来的
  事实，不从 `status` 反推（见下面「已配/未配」的决策）。
- `src/daemon.rs`：`Request::Profiles` 处理里，`sec.get(&p.name).is_some()` 现在只查一次，
  同时喂给 `status_of`（原有用途）和新的 `has_secret` 字段，避免中间密钥文件被并发改动导致
  两处结果不一致。
- `src/ui.rs`：
  - `View::Secrets { entries: Vec<ProfileEntry>, state: ListState }`——密钥设置页。
  - `View::EnterSecret` 加 `return_to_settings: bool` 字段；所有构造点（AskSecret 来自选择器 /
    密钥设置页 Enter / verify 成功回填 / Esc/Backspace/Ctrl+O/字符输入的每次重建）都显式带上它，
    没有一处用 `..` 蒙混或猜测。
  - `pub fn secret_rows(entries: &[ProfileEntry]) -> Vec<(String, bool)>`：只列
    `secret.is_some()` 的行，「已配」读 `has_secret`。
  - `back_one_level`：`EnterSecret { return_to_settings: true, .. }` 退回 `Secrets`（空壳，同
    `PickProfile` 的既有约定）；`Secrets` 本身退回 `Board`。
  - `escape_hint` / `idle_help`：`EnterSecret` 按 `return_to_settings` 分岔文案（「回设置」 vs
    「回列表」），`Secrets` 有自己的一套；`Board` 的帮助行加了 `c 密钥`。
  - 看板 `c` 键：拉 `Request::Profiles` 直接进 `Secrets`（失败则留在看板报错，不进一个没法
    显示错误的空页）。
  - `Secrets` 视图按键：`↑↓` 移动、`Enter` 进 `EnterSecret{return_to_settings:true}`、`d` 删除、
    `Esc`/`Ctrl+Q` 回看板。
  - `fn refetch_secrets(client, focus)`：改/删完之后重新拉一份数据，`focus` 给了就把光标定回
    原来那个 profile（同名查 `secret_rows` 里的下标），不给就落第一行。
  - `draw()` 里 `View::Secrets` 的渲染：布局照抄任务简报的建议（label 列 14 宽 + 已配/未配，
    绿/暗灰）。
  - 循环收尾处新增 `needs_secrets_refetch`，对称于既有的 `needs_profile_refetch`，把
    `Esc`/`Ctrl+Q` 留下的空壳 `Secrets` 补上数据；这个分支拉取失败时退回看板并把原因放进
    `message`（`Secrets` 没有 `warning` 字段，见下面的决策）。

## 测试

TDD：先写简报里的三个测试，跑 `cargo test --lib ui` 确认因为 `secret_rows`/`View::Secrets`
不存在而编译失败（RED），再补实现让它们编译通过（GREEN）。

在此基础上补的测试（简报本身偏薄，这几条是我加的）：

- `secret_view_from_settings_escapes_back_to_settings_not_the_picker` / `..._escape_hint_...` /
  `..._idle_help_...`：`return_to_settings` 是这次任务真正的新接口，光测「从选择器进来」那条
  老路径不够，必须补「从设置页进来」这条新路径的 `back_one_level`/`escape_hint`/`idle_help`
  三件套，否则改错了字段值也测不出来。
- `secrets_page_help_lists_its_own_keys`：`idle_help(&View::Secrets)` 真的提了 `Enter 改`/`d 删`。
- `secrets_view_renders_without_panicking_when_nothing_needs_a_key`：空列表（没有 profile 需要
  密钥）不panic、标题正常画出来——简报没提这个边界，但我在「收尾自查」清单里被明确要求想过它。
- `secrets_view_renders_configured_and_unconfigured_rows`：渲染层真的把「已配」「未配」两种状态
  画出来，不是只测数据层的 `secret_rows`。

### TDD Evidence

**RED**（把 `proto.rs`/`daemon.rs`/`ui.rs` 临时还原到本任务开工前的状态，只贴简报里的三个测试
和一个最简 `with_secret` 辅助，跑）：

```
$ cargo test --lib ui
error[E0425]: cannot find function `secret_rows` in this scope
    --> src/ui.rs:3073:20
     |
3073 |         let rows = secret_rows(&entries);
     |                    ^^^^^^^^^^^ not found in this scope

error[E0599]: no variant named `Secrets` found for enum `ui::View`
    --> src/ui.rs:3082:34
     |
  71 | enum View {
     | --------- variant `Secrets` not found here
...
3082 |             back_one_level(View::Secrets {
     |                                  ^^^^^^^ variant not found in `ui::View`

error: could not compile `dct` (lib test) due to 2 previous errors
```

符合预期：这两个符号是本任务要产出的接口，还没实现之前测试连编译都过不了。

**GREEN**（还原完整实现后）：

```
$ cargo test --lib
test result: ok. 158 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.98s
```

```
$ cargo fmt && cargo test && cargo clippy --all-targets -- -D warnings && git diff --check
（全部无输出/全绿，见下方完整跑一遍的记录）
```

全量 `cargo test`（含集成测试 `tests/*.rs`）158 + 若干集成测试全部通过；`cargo clippy
--all-targets -- -D warnings` 无警告；`git diff --check` 无行尾空白。

## 手动运行

用临时 `HOME=/tmp/dct-manual-home`（scratchpad 路径对 unix socket 来说太长，`SUN_LEN` 超限，
挪到 `/tmp` 下）起了一个隔离的 `dct` 实例，`secrets.toml` 里预置了 `kimi` 的密钥，
`~/.dct/profiles/testagent.toml` 放了一个不需要 verify 的自定义 profile 方便测「改密钥成功后
回设置页」这条路径（内置四个都有 `[secret.verify]`，拿假密钥测不出「验证通过」的分支）。

- 按 `c`：看板帮助行确实有 `c 密钥`，进设置页只列了 Kimi/GLM/DeepSeek/Qwen API 和自定义的
  测试Agent，`claude`/`codex`/`opencode`/`qwen`/命令行 都没出现——符合设计。Kimi 一开始显示
  「已配」（绿），其余「未配」（暗灰）。
- 选中 Kimi 按 `Enter`：进填密钥页，底栏正确显示 `Ctrl+Q 回设置` / `Esc 返回设置`（不是
  「回列表」）。打一个假密钥回车，`verify` 打了真的网络请求，因为是假密钥被拒，显示「这个密钥
  用不了，可能是复制的时候少了一段」——网络本身是通的，验证链路没问题。按 `Esc` 回设置页，
  Kimi 仍显示「已配」（验证没过就没存盘，原来的密钥没被覆盖）。
- 选中 Kimi 按 `d`：立即删除，行内翻成「未配」，底栏消息「已删除 Kimi 的密钥」。
- 光标移到已经是「未配」的 GLM 按 `d`：不发请求，只提示「这个还没配密钥，没什么可删的」——
  见下面「`d` 要不要确认」的决策。
- 用不需要 verify 的 `测试Agent`：填一个新密钥回车，**第一次手测就在这里抓到一个真 bug**——
  存盘成功后本该立刻回到密钥设置页并显示刷新过的「已配」，实际卡在一屏空列表，直到我按了别的
  键（`Ctrl+Q`）才刷出来。根因和修复见下面「过程中发现的 bug」。修完之后同样的操作：改完直接
  自动刷新出「测试Agent 已配」，光标停在这一行，没有再按任何多余的键。
- `Ctrl+Q`：从设置页直接回看板。

清理：跑完 kill 了 tmux 会话和临时守护进程，删掉了 `/tmp/dct-manual-home`。

## 决策

**已配/未配的真实性**：按简报的警示，没有用 `status != NeedsSecret` 反推——加了
`has_secret: bool` 字段，daemon 直接从 `SecretStore` 查出来喂给它，跟喂给 `status_of` 的是
同一次查询结果，不会出现「状态判定用一份数据、密钥页用另一份数据」的不一致。这条边界原本
用 `NeedsDependency`/`NotInstalled` 才会暴露（CLI 没装时 `status` 报的是这两种，不管密钥填没填），
简报自己的测试也承认只覆盖了 `Ready`/`NeedsSecret` 两种、绕开了这个边界——加字段是唯一不留
这个坑的做法。

**`d` 要不要二次确认**：没加确认步骤，理由：
1. 后果有限且可逆——删掉的只是一份密钥字符串，用户手上大概率还留着申领页面的账号，重新粘贴一次
   就能补回来，不是丢文件/丢提交那种真正回不去的操作。
2. 跟看板上 `u`（回滚）、`s`（停止）的既有先例一致——这两个键在这个项目里也是按下就执行、
   用消息条给反馈，不额外弹二次确认框。密钥页延续同一套交互语言，不给这一页单独发明一套
   「危险操作先问一遍」的新模式。
3. 简报给的接口清单（`View::Secrets { entries, state }`）没有留「确认中」这个状态的位置，
   硬塞一个 `pending_delete` 字段进去是给一个薄简报的任务过度设计。
4. 唯一补的安全网：对着一个「未配」的行按 `d` 是纯提示、不发请求（`Some((_, false))` 分支），
   避免用户对着空行按错了却看见一句空洞的「已删除」而困惑。
5. 承认代价：`d` 在看板上是「看 diff」，在这一页是「删」，确实是同一个键两种性质完全不同的
   动作（简报原话点出的风险）。缓解手段是消息反馈足够醒目、可辨认（「已删除 X 的密钥」），
   而不是靠「反正是无害操作」不当回事。

**简报留白处我的补充**：
- `EnterSecret` 的 Esc/Ctrl+Q 从设置页进来时退到哪：复用了 `PickProfile` 已有的「空壳 + 循环
  收尾统一重拉」惯例（`back_one_level` 给空的 `Secrets{entries:vec![]}`，收尾处
  `needs_secrets_refetch` 补数据），跟原有代码风格保持一致，而不是另起一套。
- `Secrets` 没有 `warning` 字段（跟 `PickProfile` 不一样）：密钥页的错误反馈走 `message`
  （底部状态栏），拉取失败就直接退回看板并把原因放进 `message`，而不是让用户卡在一屏
  「数据是空的但没人告诉他为什么」的死循环里——`PickProfile` 那种「一直留在原地重试」的模式
  在没有 `warning` 字段撑腰的情况下会变成沉默的空转，体验更差。
- 改完/删完之后光标落在哪：`refetch_secrets` 优先把光标定回原来那一行（按名字在新数据里找），
  不是每次都弹回第一行——这是我在写代码时做的选择，简报没提，出发点是用户可能在连续处理好几
  个 profile，每次都跳回顶端很打断节奏。唯一的例外是走「空壳 + 通用重拉」路径的
  Esc/Ctrl+Q——那条路径复用的是既有惯例（永远选第一行），保持跟 `PickProfile` 一致没有另开
  分支。

## 过程中发现的 bug（已修复）

手动运行时抓到一个真实 bug：「验证通过、存盘成功、`return_to_settings=true`」这条路径最初的
实现是套用 `PickProfile` 那套「先甩一个空壳 `View`，指望循环收尾处的通用重拉逻辑把数据补上」的
惯例。但这条惯例成立的前提是「空壳是在这一轮按键处理里产生的，后面紧跟着就会走到收尾代码」——
而 `verify_rx` 的处理在循环**顶部**，跟这一轮有没有按键无关；如果用户这时候没有按任何键，
`event::poll` 超时会直接 `continue` 到下一轮循环开头，跳过收尾的重拉逻辑，空壳会一直空着，
直到用户偶然按下下一个键才被补上。手测的时候真实复现了：改完一个密钥，界面卡在一屏空列表，
直到按了 `Ctrl+Q` 再按 `c` 重新进设置页才刷新出来。

修复：这条分支不再套用「空壳 + 通用重拉」的惯例，改成直接调用 `refetch_secrets(&mut client,
Some(&profile))`，在 verify 成功的那一刻就地把数据拉齐、光标定回刚改的这一行——不依赖任何
后续按键。已用 `d` 删除后的即时刷新（同样调用 `refetch_secrets`，这条本来就在按键处理流程内，
没有这个问题）和手动复测确认修复有效。这也是「简报只覆盖了两条测试路径就点名有边界」这句话
在别的地方的回响：`run()` 本身连不了真 socket、测不了，这类「哪条路径真的会在按键循环外跑」的
时序 bug 只能靠手动运行抓，测试套件目前抓不到——记在这里供后人参考。

## 文件与行数

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/proto.rs`：`ProfileEntry`
  加 `has_secret` 字段
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/daemon.rs`：`has_secret` 赋值
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/ui.rs`：3055 → 3538 行
  （+483）
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/README.md`：按键表加 `c` 行 +
  一段「改/删密钥」说明
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/README.zh-CN.md`：同上，中文

## 自查结果

- `View::Secrets`、`c` 键、行列表、改/删、`return_to_settings`、`escape_hint`、`idle_help`、
  `back_one_level`、两份 README、测试——全部覆盖，见上文逐项说明。
- 没有任何新分支用 `continue`——`Secrets`/`EnterSecret` 的每个按键分支都显式 `view =
  View::XXX { ... }` 重建（同 `View::PickProject` 的写法），走到循环尾部的
  `message_after_transition`。
- 没有 profile 需要密钥时的行为：`secret_rows` 返回空 vec，渲染出一个只有标题「密钥设置」、
  没有任何行的空列表框，`↑↓`/`Enter`/`d` 因为 `state.selected()` 落不到任何有效行而全部是
  no-op，`Esc`/`Ctrl+Q` 正常回看板——测试
  `secrets_view_renders_without_panicking_when_nothing_needs_a_key` 覆盖了渲染这一半。
- 删除之后列表会刷新，光标落在同一个 profile 上（`refetch_secrets` 的 `focus` 参数）；
  上面记录的那个 verify-成功路径的 bug 已经用同样的方式修复并手动复测。
- 密钥本体没有走漏到任何 `Debug`/日志/错误消息：`ProfileEntry.secret` 只装 `SecretPrompt`
  （提示文案 + URL），从来没有真正的密钥值；新加的 `has_secret` 是纯布尔；`message` 里出现的
  只有 profile 的 `label`（人话名字），不是密钥值本身。
- `src/ui.rs` 到 3538 行。这个文件目前承担了「视图状态机 + 渲染 + 一堆纯函数辅助 + 测试」
  四种职责，已经超过 3500 行——**这是一个值得关注的问题**：继续在这个文件里加新视图会让它
  越来越难导航，值得考虑按「状态机/按键处理」「渲染」「纯函数辅助」拆成几个子模块，但拆分
  本身不是这个任务的范围，这里只提出来，不在本任务里做。

## Issues / concerns

- `src/ui.rs` 单文件 3538 行，建议后续任务考虑拆分（见上）。
- 手动测试发现并修复了一个真实的时序 bug（详见「过程中发现的 bug」一节）；这类 bug 现有测试
  套件抓不到，因为 `run()` 本身连真 socket、测不了——如果以后还要在 `run()` 循环顶部（`verify_rx`
  处理那一段）加新的、不依赖按键触发的状态转换，务必留意同样的 `continue`-跳过收尾的陷阱。

---

# 修复报告：code review 两个 Important finding + 一个 minor

commit `20b0f0f` 的 review 提了两个 Important、一个 minor（可选）。以下是修复内容。

## Finding 1：`d` 删密钥要二次确认

**认输**：原来的「跟 `u`/`s` 保持一致、后果有限可逆」的论证站不住——`u`/`s` 作用在能重建的东西
（checkpoint、会话）上，密钥不是，而且 `d` 在看板上恰好是「看 diff」这个无害动作，物理键完全
一样，肌肉记忆会带过来。review 说得对，直接改。

**确认形状**：两段式，按 review 建议的方案实现，武装状态存 profile 名字（不存下标——列表增删
会让下标指错行，名字不会）：

- `View::Secrets` 加 `pending_delete: Option<String>` 字段（`View` 的 `#[derive(Clone)]` 覆盖到
  `Option<String>`，不用额外处理）。
- 第一次按 `d`：不发任何请求，把 `pending_delete` 设成当前选中行的名字。行内该行的「已配」
  文字换成红色加粗的「再按 d 删除，按其他键取消」；底部消息栏再重复一遍同样的话（双保险，行内
  提示万一没注意到，底栏还有）。
- 第二次按 `d`（且 `pending_delete` 记的名字跟当前选中行一致）：真发 `Request::DeleteSecret`。
- **任何其他键**（包括 ↑↓）：清空 `pending_delete`。`↑↓` 单独处理是因为「挪光标」不经过
  `KeyCode::Char('d')` 分支，必须在 Up/Down 分支里显式清；顺手把底部消息栏里可能还挂着的
  「再按一次删除 X」也清掉——不然光标挪到别的行，行内提示已经消失，底栏却还念叨着上一行的名字，
  用户会怀疑刚才那下到底有没有生效。
- `Esc`/`Ctrl+Q`：整个 `View::Secrets` 被扔掉换成 `View::Board`，武装状态自然作废，不用单独处理
  （`back_one_level` 对 `Secrets` 走的是「退一层到看板」的通用兜底分支）。

**判断逻辑抽成纯函数**：`decide_delete_key(target, pending_delete) -> DeleteKeyAction`（放在
`secret_rows` 后面）。真发 `DeleteSecret` 请求那半必须留在 `run()` 里因为要碰 daemon 连接（这个
文件里所有 `client.call` 分支都是这样处理、测不到的），但「这次按 d 该武装还是该确认」这个判断
完全不碰网络，抽出来之后可以直接单测——这是这次唯一新增的、非渲染类的生产代码路径，值得有专门
覆盖，而不是只靠手测兜底。

**新增/改动的构造点**：`View::Secrets` 加字段之后，之前所有构造它的地方都要跟着改——看板 `c`
键、`EnterSecret` 的两处 Esc 空壳、`refetch_secrets` 的两个分支、`back_one_level` 的空壳分支、
渲染函数、以及五处测试构造。逐一过了一遍，`cargo build` 报的每一处「missing field」都补上了
`pending_delete`（大多数是 `None`，只有武装那一支是 `Some(name)`）。

**新增测试**（`src/ui.rs` 的 `mod tests`，靠近文件末尾，标了 `Finding 1（Task 13 code review）`
的分组注释）：

- `decide_delete_key_arms_on_first_press`：没有武装状态、选中已配行 → `Arm`。
- `decide_delete_key_confirms_on_second_press_of_the_same_row`：武装的名字等于选中行 → `Confirm`。
- `moving_the_cursor_must_disarm_pending_delete`：武装的是 `kimi`，但当前选中行已经是 `glm`
  （模拟「挪了光标但没人记得清空 pending_delete」的疏漏场景）→ 断言判成 `Arm("glm")` 而不是
  `Confirm`。这是名字比对这道防线本身的测试：就算 `run()` 里哪天漏改了一条清空分支，这个判断
  函数也不会把新行误判成「确认删除」。
- `decide_delete_key_on_unconfigured_row_just_notifies`：未配的行 → `NotConfigured`（照抄原有
  行为，纳入同一组）。
- `decide_delete_key_with_nothing_selected_is_a_no_op`：没有选中任何行 → `NoSelection`。
- `back_one_level_from_secrets_clears_any_armed_delete`：武装状态下 `back_one_level` 依然落到
  `View::Board`，武装状态随整个视图一起作废。
- `secrets_view_renders_the_armed_delete_prompt_on_its_row`：渲染层测试，武装之后这一行必须画出
  「再按…d…删除」的字样，且**不能**再出现「已配」——这是 finding 原话点名要的「inline prompt on
  that row」，只测判断函数不测渲染的话，这条要求就没有真正落地。

## Finding 2：设计文档「未实测项」表要落回 Task 2 Step 6 的真实结果

`docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md` 的「⚠️ 未实测项」表已更新：

- `codex`：该行原本就写着「已实测（v0.146.0，PTY 抓屏确认 `esc to interrupt`）」——这条本来就是
  准的，没有改动判定，只是确认它没有被漏看。
- `claude`：原文「现状已在用」改成「**已实测**——`claude` 本身已安装，在开发机上日常使用中」，
  把「谁验证的、验证到什么程度」写明白，不再是一句可以有两种理解的短语。
- `opencode` / `qwen`：判定不变（仍是「未实测，本机没装」），但补了一句「也没找到能装的机器；
  pattern 依旧刻意留空，跟文档最初的决定一致」——明确这是「查过、装不了」，不是「没查」。
- 四个 vendor 的 base_url / verify url：判定不变（仍是「未实测」），但加了「这四项是发布前
  最后一个阻塞项，需要各家的真实 API key 才能验」，把它在发布流程里的位置点出来，不让下一个人
  猜这条到底要不要紧。

没有软化任何一条未验证的判定，只是把「谁查过、查到什么程度、卡在哪」写清楚。

## Minor：改密钥成功没有反馈

`src/ui.rs` 里 `verify_rx` 处理分支、`Ok(Response::Ok) if return_to_settings` 那一支，
`refetch_secrets` 之前加了一行 `message = format!("已保存 {label} 的密钥").into();`——现在改密钥
和删密钥这对镜像操作在反馈上对称了。

## 手动运行

用临时 `HOME=/tmp/dct-manual-home2`（跟原报告一样挪到 `/tmp` 下，避开 scratchpad 路径太长导致
`SUN_LEN` 超限的问题），`secrets.toml` 预置 `kimi` 的密钥，`~/.dct/profiles/testagent.toml` 放一个
不需要 verify 的自定义 profile，起了一个隔离的 `dct` 实例（tmux 会话 + `capture-pane` 截屏比对）：

- 按 `c` 进设置页：Kimi「已配」（绿），其余「未配」（暗灰）。
- 光标在 Kimi 上按 `d`：这一行立刻变成红色加粗的「再按 d 删除，按其他键取消」，底部消息栏同步
  显示「再按一次 d 删除 Kimi 的密钥，按其他键取消」。
- 按 `↓`：光标挪到 GLM，Kimi 那一行**变回**「已配」（武装状态确实清掉了），底部消息栏也回到
  `idle_help` 的默认按键提示（不再挂着刚才那句关于 Kimi 的话）。
- `↑` 回到 Kimi，再按 `d`：又变成「再按 d 删除…」。**再按一次 `d`**（这次是真正原地的第二次）：
  这一行翻成「未配」，底部消息栏显示「已删除 Kimi 的密钥」——跟改之前的行为一致，只是现在要
  按两次才会触发。
- 对着刚删完、已经「未配」的 Kimi 再按 `d`：走的是「这个还没配密钥，没什么可删的」分支，不武装、
  不发请求——没有被新的确认逻辑污染。
- 选中「测试Agent」按 `Enter` 填一个新密钥回车：这一行翻成「已配」，底部消息栏显示「已保存
  测试Agent 的密钥」（Minor finding 修的那句话），光标停在这一行没有跳走。
- 从设置页按 `Ctrl+Q`：直接回看板。

清理：`tmux kill-session`、`pkill dct`、删掉了 `/tmp/dct-manual-home2`。

## 验证命令与结果

```
$ ~/.cargo/bin/cargo build
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.94s

$ ~/.cargo/bin/cargo fmt
（无输出）

$ env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test --lib
test result: ok. 165 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.21s

$ env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo test
（含 tests/*.rs 全部集成测试）全部通过

$ env GOCACHE=/tmp/dcwb-go-cache ~/.cargo/bin/cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.27s
（无警告）

$ git diff --check
（无输出，没有行尾空白）
```

原有 158 个 lib 测试 + 本次新增 7 个（5 个 `decide_delete_key` 判断测试 + 1 个
`back_one_level_from_secrets_clears_any_armed_delete` + 1 个渲染测试）= 165，与实测的
`165 passed` 对上。

## 文件

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/ui.rs`：`View::Secrets` 加
  `pending_delete` 字段、`decide_delete_key`/`DeleteKeyAction`、`d` 键改成两段式确认、渲染层加
  武装提示、改密钥成功加确认消息、六个新测试。
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/docs/superpowers/specs/2026-08-03-dct-multi-agent-design.md`：
  「⚠️ 未实测项」表落回真实验证结果。

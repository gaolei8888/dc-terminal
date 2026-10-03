# dct 玩三消：规则选步 + 真去玩（第一轮）

日期：2026-10-02
关联：dct 大脑设计第 9 节（`2026-09-27-dct-brain-design.md`）；dco `read_grid` 设计（dc-octo `docs/superpowers/specs/2026-10-02-read-grid-design.md`）；
dc-octo 实验报告（`docs/experiments/2026-09-30-fast-decision-*.md`）。

## 要解决什么

Candy Crush 现在由会话里的大模型看截图选步，每步 30～40 秒、每步花 token。dco 已经能在 1～5 毫秒内读出棋盘（`read_grid`），
实验里规则打分在三消上 100% 选对。这一轮让 dct 自己读盘、按规则选步、让 dco 划，一步 1 秒以内，并把每一步记下来，
以后拿去和大模型比。

## 用户定下的（2026-10-02）

- 分三块：① 规则选步 ② dct 真去玩 ③ 新游戏 AI 自动校准。**这一轮只做 ①②**，③ 下一轮。
- **规则拿不准时照样走分最高的**，只在读不出棋盘、没有能走的步时停；每一步全记下来，事后和大模型比。
- **入口是「跟 AI 说一句话」**：小白在 dct 的会话里说「帮我玩一关 Candy Crush」，会话里的 agent 去跑。
  实现方式是一张说明卡（SKILL.md），**Claude Code、Codex、千问都必须支持**。`dct mcp` 和 dc.ai 菜单栏按钮下一轮做。
- 判断放在 dct，不放 dco：dco 只看、做、核对、急停（dco 设计的分工，用户 2026-09-30 再次确认）。

## 不在这一轮

AI 自动校准、dcv 的游戏资料（`dcv games`）、开关卡和道具检查、弹窗处理、问大模型、`dct mcp`、dc.ai 按钮、小模型。

## 结构

| 位置 | 做什么 | 依赖 |
|---|---|---|
| `crates/dct-game` | 纯算法：棋盘、列出合法交换、模拟消除和下落、打分 | 无 IO，只有 serde |
| `src/dco_client.rs` | 连本机 dco：读 `~/.dco/endpoint.json` 里的 unix socket，说 MCP（initialize → tools/call），一条长连接 | — |
| `src/game/profile.rs` | 棋盘配置：内置 1712 关那套，`~/.dct/games/<游戏>.toml` 可覆盖 | — |
| `src/game/play.rs` | 读盘 → 选步 → 划 → 等画面停下 → 下一步；停下的条件 | 通过一个 trait 用 dco，测试换成假的 |
| `src/game/log.rs` | 每步一行 JSON | — |
| `src/game/skill.rs` | 把说明卡装进三个 agent 的 skills 目录 | — |
| `src/cli.rs` | `dct game play` | — |

## ① 规则选步（`crates/dct-game`）

**输入**：`read_grid` 的一次返回（rows、cols、cells、odd、classes）。类别编号只在这一次返回里用来比相等，不跨次使用。

**合法交换**：相邻两格（横或竖）交换后，任一方向连成 ≥3 颗同类，算合法。两格都是 odd（两颗特殊糖）时，交换也算合法。
**只有一格的类别**（比如彩色炸弹，`read_grid` 永远不会把它标 odd）不知道是什么，不参与连线，也不当作可交换的糖。

**模拟**：交换 → 找出所有 ≥3 的横竖连线 → 记下会出的特殊糖（直线 4 颗 = 条纹，L/T 形 = 包装，直线 5 颗 = 彩色炸弹；
特殊糖留在交换落点那格）→ 清掉 → 上面的糖往下掉，顶上补进来的未知、当空格 → 再找连线，重复到没有为止。
被清掉的格里有 odd 的，算「引爆了特殊糖」（不知道是哪种，统一算一种）。

**特征**：`cleared`（第一下消几颗）、`cascade`（连锁消几颗）、`striped` / `wrapped` / `bomb`（做出几颗）、`triggered`（引爆几颗）、
`special_swap`（两颗特殊糖互换）、`lowest_row`（消掉的最低一行）。

**打分**（常量，第一轮拍的，写明出处，以后按记录调）：

```
score = cleared + 0.5·cascade + 6·striped + 8·wrapped + 15·bomb + 5·triggered + 20·special_swap + lowest_row / rows
```

同分时按固定顺序挑：最低的行优先，再左边的列优先，再横向交换优先。同一盘每次选得一样。

## ② 真去玩（`dct game play`）

```
dct game play [--game candy-crush] [--steps 20] [--dry-run]
```

- `--steps`：最多走几步，默认 20，范围 1～200。agent 的命令工具有超时（几分钟），一次 20 步左右能在超时前结束；用户要接着玩，agent 再跑一次。
- `--dry-run`：只读盘、选步、打印，不划。

**一步**：
1. `read_grid`（用配置里的窗口、范围、行列、参数）。
2. 选步。
3. `swipe`：从一格中心划到相邻格中心（窗口比例坐标：`x = region.x + (c + 0.5) / cols · region.w`，y 同理），时长用 dco 默认。
4. 等画面停下：先等 150 ms，再每 100 ms 读一次；连续两次读到的 cells 和 odd 完全一样，且和划之前不一样，算停下。
   3 秒内一直和划之前一样 → 这一步没生效（记 `no_change`），连续两次没生效就停；8 秒还在变 → 停（「画面一直在动」）。

「一步 1 秒以内」量的是 1～3 段（读、选、划），不含等游戏动画。

**停下的条件**（每条都用一句人话说清楚为什么停）：
- 读不出棋盘（`not_a_grid`），或者类别数和这次开始时不一样——多半是这关结束了、弹了窗；
- 没有能走的步；
- 走满 `--steps`；
- dco 回 `halted` / `paused` / `screen_locked`，或者 dco 没在运行；
- 用户按 Ctrl-C（agent 那边就是用户让它停）。

**输出**：每步一行，大白话，例：

```
第 3 步：第 7 行第 2 列 ↔ 第 7 行第 3 列，消 4 颗，做出条纹糖（读 2 ms，选 0 ms，划 262 ms）
停了：没有能走的步了。这次走了 12 步，记录在 ~/.dct/games/log/2026-10-02.jsonl
```

不出现类别编号、分数公式这类东西；行列从 1 数、从上往下。

## 棋盘配置

内置一份 `candy-crush`（1712 关的版面）：

```toml
window = { app = "iPhone Mirroring" }
region = { x = 0.2372, y = 0.3156, w = 0.5257, h = 0.4622 }
rows = 9
cols = 5
inset = 0.6
class_de = 24
top_div = 5
odd_share = 0.15   # 0.15 时包装糖也标 odd、不误标普通糖（dco 实测）
```

`~/.dct/games/candy-crush.toml` 存在就用它。配置的指纹 = 配置文本的 sha256，记进每一步。
**已知限制**：棋盘位置每关不同，换关要改配置；棋盘里有空洞、障碍的关卡这一轮不处理（读出来会多出类别或被当成糖）。这些由第三块自动校准解决。

## 记录

`~/.dct/games/log/<日期>.jsonl`，每步一行：

```json
{"schema":1,"time_ms":…,"game":"candy-crush","profile_sha256":"…",
 "observation_id":"obs-…","observed_at_ms":…,"frame_age_ms":…,
 "rows":9,"cols":5,"cells":[[…]],"odd":[[…]],"classes":[{"id":0,"rgb":[…],"count":…}],
 "candidates":[{"a":[6,1],"b":[6,2],"score":…,"features":{…}}],
 "chosen":0,"dry_run":false,
 "timing_ms":{"read":…,"choose":…,"swipe":…,"settle":…},
 "outcome":"moved|no_change|stopped","after_observation_id":"obs-…"}
```

全部合法交换都记，不只记选中的——以后拿去和大模型比、训练，需要知道当时有哪些选择。
停下的原因单独写一行 `{"schema":1,"stop":"…","steps":…}`。

## 说明卡（三个 agent 都要）

一份 `SKILL.md`（编进 dct 二进制），装到：

- `~/.claude/skills/dct-game/SKILL.md`
- `~/.codex/skills/dct-game/SKILL.md`
- `~/.qwen/skills/dct-game/SKILL.md`

三家都认这个格式（Codex 0.160、Qwen Code 0.24.6 本机已核对有 skills 目录支持）。**什么时候装**：守护进程启动时，以及 `dct install <agent>` 之后；
只在那个 agent 的目录（`~/.claude` 等）存在时装；内容一样就不写；文件里带 dct 的标记行，没有标记的同名文件（用户自己写的）不覆盖。

卡片内容要点：
- 什么时候用：用户要玩 Candy Crush / 三消，iPhone 镜像里已经开着一关；
- 怎么做：跑 `dct game play`，把每一步的输出转述给用户；跑完一次（20 步）问用户要不要继续；
- 停：用户说停就中断命令；命令自己停下时，把停的原因原话告诉用户，不要自己去点屏幕补救；
- 不要做：不要自己调 dco 去点关卡按钮、道具、付款（这一轮不开关卡，用户自己开）。

## 出错

| 情况 | 说给用户的话 |
|---|---|
| dco 没在运行 / socket 连不上 | 「dco 没在运行，先打开 dco 再试」 |
| 窗口不在 | 「没找到 iPhone 镜像的窗口，先打开它、进到一关里」 |
| 没有屏幕录制权限 | 照 dco 的原话 |
| halted / paused | 「dco 急停 / 暂停了，这次停下」 |
| 配置文件写错 | 「~/.dct/games/candy-crush.toml 第 N 行：…」 |

## 测试

- **算法（`crates/dct-game`，cargo test）**：手写小棋盘覆盖：横竖三连、四连出条纹、L/T 出包装、五连出炸弹、两颗特殊糖互换、
  连锁下落、只有一格的类别不参与、没有合法步、同分的固定顺序。
- **真实棋盘**：用 dco 的 `tests/fixtures/candy-1712*.png` 在 dco 那边跑一次 `read_grid`，把返回的 JSON 存进 dct 的 `tests/fixtures/`，
  人工核对合法交换的全集，钉进测试。
- **玩的循环**：假 dco（trait）按脚本返回棋盘序列，测：正常走、没生效两次就停、一直在变就停、类别数变了就停、halted 就停、
  `--dry-run` 不调 swipe、记录的每行字段齐全。
- **说明卡安装**：临时 HOME 下测：目录不存在不装、内容一样不写、用户自己的同名文件不覆盖、三家都装上。
- 变异测试照老规矩跑在算法和停下条件上。

## 验收（真机）

1. iPhone 镜像里开一关 1712 版面的关卡，`dct game play --dry-run` 打印的选择看着合理。
2. `dct game play` 真走 20 步：每步读 + 选 + 划 p95 < 1 秒；记录文件每行完整。
3. 在 dct 里分别开 Claude Code、Codex、千问的会话，各说一句「帮我玩一关 Candy Crush」，三家都去跑 `dct game play` 并转述进度。
4. 玩的时候 dco 急停，命令在下一步前停下，说清原因。

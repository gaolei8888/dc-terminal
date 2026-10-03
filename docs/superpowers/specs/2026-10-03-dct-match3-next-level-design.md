# dct 玩三消：失败后自己重来、安全地关弹窗、生命用完就停（第二轮 · 第一步）

日期：2026-10-03
前提：第一轮已合并（`docs/superpowers/specs/2026-10-02-dct-match3-play-design.md`，`dct game play` 能在一关里读棋盘、选步、划）。
真机验收（2026-10-03，第 1712 关）：28 步全部有效，一步（读+选+划）中位 336 ms、最慢 511 ms，打通了一关；
打通以后游戏跳到「Daily Stamps」画面（只有一个 Play 按钮），dct 看到颜色种类变了就按设计停下，没有乱点。

## 要解决什么

用户要「让 dct 自己点 Play，跳过所有广告，一直玩下去」。拆开看：

1. **开始一局**：点 Play。
2. **关掉弹窗**：只关「安全的」，凡是要花钱、要看广告领奖励的都不碰。
3. **一关接一关**：每一关的棋盘位置、行列数、障碍都不一样，这要等下一步「新游戏自动调参」。**这一步不做。**

用户 2026-10-03 定：**分两步做；生命用完就停下来告诉用户。** 这份设计是第一步。

## 做什么、不做什么

做：
- `dct game play --auto-next`：一局结束后自己继续——失败了就**重来同一关**；弹窗里只点白名单里的字；认出「生命用完」就停。
- 每一个不认识的画面都**停下来、把画面上的字和截图指纹记进记录**，之后按证据补白名单（不凭想象写词表）。

不做：
- **打赢以后进入下一关。** 下一关的版面不一样，用第 1712 关的位置去读会读成一团，划出去的步是乱走的，白白烧生命。所以**打赢就停下**，告诉用户「通关了，下一关要重新认棋盘」，等第二步的自动调参。
- 点任何带价格、金条、钻石、「看广告领奖励」的东西；关只有叉叉图标的广告（dco 把没有文字的按钮一律当作对外，点不了）。碰到就停下，告诉用户自己处理。
- 不开新能力给 dco：用的还是 `see`（OCR）和 `tap`，dco 自己按按钮上的字定档、拒绝付款类，是第二道保险。

## 画面分类（纯函数，`crates/dct-game/src/screen.rs`）

输入：一次 `see`（OCR）读到的元素（id、文字、位置）。输出下面之一，**按这个顺序判**（前面的优先）：

| 分类 | 怎么认 | dct 做什么 |
|---|---|---|
| `Money` | 任何元素的文字带价格符号（¥ ￥ $ € £ 💎）、数字加 gem/gems/金币/钻石、或含 buy / purchase / pay / checkout / 购买 / 支付 / 充值 / gold / bar(s) 这类词 | **停**，说「出现了要花钱的画面，请你自己处理」 |
| `LivesOut` | 文字里有 lives 或 life，同时有 no more / out of / 0 / ask / get more 之一；或「生命」+「用完」/「不足」 | **停**，说「生命用完了」 |
| `Won` | 已知的通关字样（见下，从证据里补） | **停**，说「通关了，下一关要重新认棋盘」 |
| `Failed` | 已知的失败字样：out of moves / no more moves / try again / level failed / 没有步数了 / 再试一次 | 点那个 Try again / Play 类按钮重来；如果只有叉叉，**停** |
| `Dismiss` | 有一个元素，整条文字（去掉空白、大小写不计）正好是 Close / No thanks / Not now / Later / Maybe later / Skip / Cancel / 关闭 / 以后再说 / 暂不 / 跳过 / 取消 | 点它 |
| `PlayButton` | 有一个元素，整条文字正好是 Play / Start / 开始 / 开始游戏 | 点它 |
| `Board` | 不是上面任何一种，而且 `read_grid` 读得出（至少 3 个「大」类别、总类别不超过 16） | 进入 `play` 一局 |
| `Unknown` | 其它 | **停**，把所有文字和截图指纹记进记录，说「出现了不认识的画面」 |

说明：
- 「整条文字正好是」是故意的：`Play now for 💎 5` 不会被当作 Play。
- 白名单**只放 dco 本来就让点的词**。dco 会拒绝带 buy/pay/price 的，这是第二道保险；dct 先自己判，不靠 dco 兜底。
- `Won` 和 `Failed` 的词一开始只放上面几个我能确定的。**真机上第一次遇到不认识的结算画面就走 `Unknown` 路线**：记录下它的文字，我们看过再补。这是有意的，不是缺陷。
- 点完以后要等画面变了才算点成（轮询 `see`，文字集合变了；最多等 8 秒，不变就记 `no_effect`，连续两次不变就停）。

## 流程

```
dct game play --auto-next [--steps N] [--tries N]
  循环：
    1. see（OCR）→ 分类
    2. Board        → play（第一轮那套，最多 --steps 步）；它停下后回到 1
    3. PlayButton / Dismiss / Failed 的按钮 → tap → 等画面变 → 回到 1
    4. Money / LivesOut / Won / Unknown / 点了没反应两次 → 停，说明原因
    5. 重来次数（点 Play / Try again 的次数）到 --tries（默认 5）→ 停
```

- `--steps`：一局里最多走几步（沿用第一轮，默认 20）。**auto-next 下它是整个命令一共最多走的步数**，不是每局，免得 agent 的命令超时（默认 60，范围 1～200）。
- `--tries`：整个命令最多点几次「开始/重来」，默认 5，范围 1～20。每一次重来都会用掉一条生命，所以有上限。
- 不带 `--auto-next` 时行为和第一轮完全一样（打完一关就停），所以现有的说明卡和用法不变。
- 命令输出一句一句的大白话，例如：「点了 Play，开始新的一局」「关掉了一个弹窗（Not now）」「这一局没过，重来（第 2 次）」，最后一句说清为什么停。

## 记录

沿用 `~/.dct/games/log/<日期>.jsonl`，新增一类行（`"kind": "nav"`，和走棋的行区分）：

```json
{"schema":1,"kind":"nav","run_id":"…","time_ms":…,"observation_id":"obs-…",
 "screen":"dismiss|play|failed|won|lives_out|money|unknown|board",
 "texts":["Daily Stamps","Play"], "tapped":"Play"|null, "outcome":"changed|no_effect|stopped"}
```

`texts` 就是 OCR 读到的全部文字——这是之后补白名单的证据。停下来的最后一行（`stop`）新增原因码：`won`、`lives_out`、`money`、`unknown_screen`、`no_effect`、`tries_done`。

## 对 dco 的要求

没有新要求。用 `see`（`source: "ocr"`）取元素，`tap {snapshot_id, element_id}` 点；`tap` 本来就按文字定档，`Play`、`Close`、`Not now` 这类是「自用」档，不需要执行票；带价格的会被 dco 拒绝。
已核对 dco 源码：`tier_for_label("Play")` 是 `SelfOnly`；没有文字的元素是 `Content`（点不了）；`Continue` 不在 dco 的 tap 词表里，会被当作自用，所以**dct 的白名单里不放 Continue / OK / Collect / Claim**（语义不清，可能连着奖励或广告）。

## 测试

- **分类器（纯函数）**：每一类各有一个真实或手写的 OCR 夹具；顺序优先级（有价格的画面里同时有 Close，必须判 `Money` 不是 `Dismiss`）；整条文字匹配（`Play now for 💎 5` 不是 `PlayButton`）；`Daily Stamps` 的真实 OCR（2026-10-03 取的）判 `PlayButton`；空元素列表判 `Unknown`。
- **循环（假 dco）**：Board → play → Failed → 点重来 → Board；Dismiss 连着几个弹窗；Won 停；LivesOut 停；Money 停且一次 tap 都没发；点了没反应两次停；`--tries` 到上限停；不带 `--auto-next` 时一次 `see`/`tap` 都不发。
- **命令层（假 dco 的集成测试）**：`--auto-next` 的输出和退出码、记录里有 `kind:nav` 行、`texts` 原样记下。
- 变异测试照老规矩跑在分类顺序和停下条件上。

## 验收（真机，需要用户在场）

1. 现在屏幕在「Daily Stamps」：`dct game play --auto-next --steps 5 --tries 1` → 点 Play → 进入新的一关，板子版面和 1712 不同，所以会走 `Board` 读不出/读成怪样；这时**期望的结果是它停下并说清楚**（不是乱走）——第二步的自动调参才解决这个。如果新一关恰好读得出来，也如实记录。
2. 找一个会失败的局面（或让步数走完）验证 `Failed` 路线：看它点的是什么、记录里 `texts` 是什么，把真实字样补进白名单。
3. 记录里每个不认识的画面都有 `texts`。

## 已知的限制

- 只认英文字（用户的 iPhone 是英文界面）；中文词表放了常见的几个，没有真机验证。
- 打赢以后停、不进下一关：这是故意的，等自动调参。
- 「看广告领奖励」「用金条买步数」一律不点，也不替用户点叉叉关广告；需要用户自己处理。
- 点按钮之前不核对「这个按钮是不是用户想要的」：Play 就是 Play。白名单之外的任何字都不点。

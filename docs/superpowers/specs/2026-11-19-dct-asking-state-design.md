# dct：把「弹框等人」认成「等你回答」

2026-11-19 · 小改动 · 已获用户同意

## 为什么

用户要「一眼看出哪些会话在等我处理」。现状是 dct 根本分不清：`dct ps` 报 12 个
「干活中」，其中 11 个其实停在 Claude 的弹框上等人按键。没有可信的状态，后面
的一屏总览（第二步）无从谈起。

## 实测（2026-11-19，本机 11 个真实会话）

11 个全是 Claude 启动时的「信任这个文件夹吗」弹框（`Accessing workspace: …
Quick safety check … ❯ No, exit / Yes, I trust this folder`），最后一行都是：

```
 Enter to confirm · Esc to cancel
```

这些目录在 `~/.claude.json` 里没有信任记录（或为 false），agent 从没真正启动。

成因：claude profile 两条都声明了——`busy_pattern = "esc to interrupt"`、
`idle_pattern = "(?m)^❯|bypass permissions on"`。弹框上两条都不在（`❯` 前面
有空格，`^❯` 匹不上），`classify()` 返回 `None` = 不表态，状态停在建会话时的
Working。这是有意的设计（防卡死时谎称空闲），副作用是弹框等人也被当成干活。

## 做什么

- `Profile` 加可选 `ask_pattern`。`classify()` 顺序：error > ask > busy > idle。
  没声明 `ask_pattern` 的 profile 行为一个字不变。
- claude profile：`ask_pattern = "(?m)^\s*Enter to confirm · Esc to cancel\s*$"`。
  **整行锚定**：agent 的回答里引用这句话（比如本文件被显示在屏幕上）不能把
  会话判成等人；真弹框里这句独占一行。其他 agent 没实测，不加。
- `tick()` 里 `if s.state == Asking { continue; }` 改掉：Asking 也要重新分类，
  否则用户答完弹框它永远停在「等你回答」。这条原本是给 Bridge（`ask_human`）
  留的，Bridge 目前不设 Asking——改的时候确认没有别处在设。
- 看板 / 九宫格：Asking 用最醒目的样式，仅次于 Failed。`dct ps`、
  `dct status --json` 走同一份状态，自动变对。
- 手机推送：Working → Asking 已经和 Working → Idle 走同一条「一轮干完」路径
  （`session.rs` ~2081）；确认推送门（`is_agent`、`first_input_empty`）对
  启动弹框的行为是用户想要的——**刚开的会话停在信任弹框上，要不要推？**
  `first_input_empty` 这道门会把它拦下。实现时按现有门不动，计划里写明。

## 测试

- 用实测两屏（普通信任框、带 219 条预批权限的长信任框）做 `classify()` 输入：
  → Asking。
- 同一屏去掉那一行、只留在正文里引用 → 不是 Asking。
- 状态机：Asking 之后屏幕变成 `esc to interrupt` → Working；变成 `❯` 输入框 → Idle。
- 现有 profile 无 `ask_pattern` 的测试全部不变。

## 不做

- 自动替用户点「信任」——那是安全决定，归用户。
- 一屏总览、已读规则（第二步；已定：看过即不算等你）。
- codex / qwen 等的 `ask_pattern`（没实测）。

## 方案取舍

- **A（采用）profile 加 `ask_pattern`**：跟 busy/idle 同一套机制，按 agent 实测配置。
- B 在 `classify()` 里硬编码弹框文字：省一个字段，但把 claude 的文案写死进
  通用代码，违背 profile 的分工。
- C 「两条都没匹上」直接判 Asking：会把卡死的会话谎报成等人——正是现有注释
  要防的事故。

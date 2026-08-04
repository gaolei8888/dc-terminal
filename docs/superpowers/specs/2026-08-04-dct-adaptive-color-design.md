# dct 自适应配色

## 起因

选 agent 菜单在 Solarized Dark 下渲染成一片空白：只剩标题、三个可用项，和一个悬空的
`▶`。中间那六个不可用的 agent、每一行的说明栏、底部的操作提示，全都消失了。

根因不是布局，是颜色。这些弱化文字用的是 `Color::DarkGray`——ANSI 亮黑（8 号色）。
Solarized 一类主题把 8 号色定义成和背景同一个颜色，于是「压暗」变成了「隐形」。
用户看到的是一个坏掉的界面，而不是一段灰字。

已经落地的临时修复是把 `Color::DarkGray` 全部换成一个常量 `DIM = Color::Indexed(245)`
（256 色表里的固定灰，不经过终端的 16 色映射）。这一步止住了 Solarized 的血，但把
问题挪到了另一头：245 是一个偏亮的灰，在浅色背景的终端上它和白底几乎分不开。
一个写死的灰不可能同时适配深浅两种背景。

这份设计要的是：灰度跟着终端背景走。

## 目标与非目标

**目标**：弱化文字（说明栏、不可用项、操作提示、Stopped/Unknown 状态）在深色背景、
浅色背景、以及任何我们探测不出来的终端上，都保持可读且明显弱于正文。

**非目标**：完整的主题系统、可配置调色板、多套命名配色。dct 的目标用户是零编程经验
的人，不会去调 TOML 里的颜色；把这些做出来只是给自己加维护面。

**只有灰适配。** 状态色和报错边框用的具名 ANSI 色（Red / Cyan / Yellow / Green）
保持原样：终端主题本来就保证这四个色在自己的背景上可读，我们再去重映射等于跟用户
自己的配色打架。会话画面里 agent 输出的颜色（`ScreenColor::Idx` / `Rgb`）更是原样
透传，那是 agent 的输出，不是我们的界面。

## 架构

新模块 `src/theme.rs`，两件事：判断背景是深是浅，据此给出弱化文字的 `Style`。

```rust
pub enum Theme { Dark, Light, Unknown }

impl Theme {
    pub fn dim(self) -> Style;
}
```

三个变体对应三种 `dim()`：

| 变体 | dim() | 理由 |
|---|---|---|
| `Dark` | `fg(Indexed(245))` | 偏亮的灰，压在深底上 |
| `Light` | `fg(Indexed(241))` | 偏暗的灰，压在浅底上 |
| `Unknown` | `Style::default().add_modifier(Modifier::DIM)` | 不指定颜色，让终端自己去暗化默认前景色 |

`Unknown` 这一支是整个设计的安全网，值得说清楚为什么它是安全的：它不写死任何颜色，
所以不可能撞上任何主题的背景色；而不支持 `DIM` 属性的终端会直接忽略它，那种情况下
文字以正常亮度显示——不够弱，但**看得见**。失败方向是「不够暗」，不是「隐形」。
这正好是我们要的：一个探测不出背景的终端，宁可让说明栏显得太亮，也不能让它消失。

### 探测链

按顺序试，任何一步拿到答案就停：

1. **`DCT_THEME=dark|light` 环境变量** —— 永远优先。探测猜错时用户（或我们排查问题时）
   得有一个不需要改代码的出口。其它值和空值一律当作没设。
2. **OSC 11 查询** —— 往 stdout 写 `\x1b]11;?\x07`，读回 `\x1b]11;rgb:RRRR/GGGG/BBBB`
   （终止符可能是 BEL `\x07` 也可能是 ST `\x1b\\`，两种都要认）。算相对亮度
   `0.2126R + 0.7152G + 0.0722B`，> 0.5 判 `Light`，否则 `Dark`。这是唯一真正问了
   终端的一步，绝大多数现代终端（iTerm2、Terminal.app、Alacritty、kitty、WezTerm、
   现代 xterm）都答。
3. **`COLORFGBG` 环境变量** —— 形如 `"15;0"`（前景;背景）。末段是背景色号：0–6 和 8
   判 `Dark`，7 和 9–15 判 `Light`。rxvt / urxvt / konsole 这些不答 OSC 11 的终端设它。
4. **都不行 → `Unknown`**。

亮度阈值取 0.5，深浅背景在实际终端配色里离这个中点都很远：Solarized Dark 的背景
`#002b36` 算出来约 0.14，Solarized Light 的 `#fdf6e3` 约 0.97。不存在需要精调阈值的
边界情形。

用的是**不做 sRGB 反伽马**的简化式——直接在 0–1 的归一化通道值上加权。判深浅只需要
一个把两类背景分得开的标量，不需要真的物理亮度；省掉三次 `powf` 也省掉一处以后会
有人想「优化」的复杂度。

### 接到 ui.rs

`static THEME: OnceLock<Theme>`，在 `run()` 里 `enable_raw_mode()` 之后、
`EnterAlternateScreen` 之前设一次。

这个位置是被两头夹死的，不是随手选的：
- **必须在 `enable_raw_mode()` 之后**：OSC 11 的回复是终端塞进 stdin 的一串字节。
  非 raw 模式下它会被行缓冲（没有换行，读不到）并且被回显到屏幕上（用户会看见一串
  乱码）。
- **必须在 `EnterAlternateScreen` 之前**：万一探测阶段有字节漏到屏幕上，那时候还在
  主屏、还没开始画界面，脏字符会被随后的 alternate screen 切换盖掉；反过来就是把
  乱码糊在已经画好的界面上。

渲染代码调 `theme::dim()`，取代现在的 `DIM` 常量。

### 顺带：`status_color` 改成 `status_style`

`dim()` 返回的是 `Style`（`Unknown` 那一支要的是 `Modifier::DIM`，不是某个颜色，
表达不成 `Color`）。但 `status_color(s) -> Color` 的两个变体 `Stopped`/`Unknown`
恰好要用弱化样式，类型对不上。

不给 `dim()` 再开一个返回 `Color` 的孪生函数——那个函数在 `Unknown` 下无法表达
「用 DIM 修饰符」，只能退回写死一个灰，等于在安全网上开个洞。改成把
`status_color` 换成 `status_style(s) -> Style`：`Stopped`/`Unknown` 返回
`theme::dim()`，其余三个返回 `Style::default().fg(Color::Cyan)` 之类。

它只有一个生产调用点（`Style::default().fg(status_color(s.state))`，本来就在
包一层 `Style`）和一个测试调用点（改成对 `Style` 断言 `assert_ne!`），代价是两处。

**为什么用全局，而不是 `DrawInput` 的一个字段。** 主题是进程级配置，启动后不变，
不是每帧的状态——把它塞进 `DrawInput` 是把一个常量伪装成状态。而且 `DrawInput` 有
26 个构造点（25 个在测试里），加一个必填字段就是 26 处纯噪音的改动。用 `OnceLock`
则一处不用改：测试里没 set 过，读出来就是 `Unknown`，拿到 `DIM` 修饰符，正好是
测试想要的与终端无关的默认值。

## 错误处理

探测的每一种失败都只是「降到链条的下一步」，不产生任何错误：终端没回复、回复超时、
回复格式不对、stdin 不是 tty、十六进制解析不了——全部往下走，最终落到 `Unknown`，
而 `Unknown` 本身就是一个能用的样式。**探测不可能让界面启动失败。**

超时是一个硬性的 150ms 上限。不答 OSC 11 的终端只付一次 150ms 的启动代价，而不是
挂在那里等。150ms 对本地终端的往返（亚毫秒级）绰绰有余，对用户来说也还在「启动」
这个心理窗口里面。

**stdin 竞争**：探测跑在事件循环启动之前，直接读 stdin，此刻没有 crossterm 的事件
读取器在抢同一个 fd。如果读到的字节不是 OSC 回复（用户在界面出来之前就敲了键），
丢弃。最坏情况是第一帧之前丢掉一次按键——比让那些字节漏进 OSC 解析器要好。

## 测试

纯函数拿真单测，覆盖到边界：

- `is_light(r, g, b)`：阈值两侧、纯黑、纯白、Solarized Dark/Light 的实际背景值，
  以及纯红/纯绿/纯蓝（三个通道权重差得远，写错位置会被这一条抓住）
- `parse_osc11`：4 位十六进制（`rgb:0000/0000/0000`）、2 位变体（`rgb:00/00/00`）、
  BEL 终止、ST 终止、垃圾输入、截断的回复、少一个通道
- `parse_colorfgbg`：`"15;0"`、`"0;15"`、`"default;0"`、空串、没有分号的垃圾
- `Theme::dim()`：三个变体给出三个互不相同的 `Style`，且 `Unknown` 那个不带任何
  前景色（这条断言防的是以后有人「顺手」给它补一个写死的灰，把安全网拆了）
- 环境变量优先级：`DCT_THEME` 压过 OSC 回复；非法值被忽略而不是当成 `Dark`

I/O 那部分（发查询、带 deadline 地读）藏在一个窄 trait `ReplyReader` 后面
（`fn read_reply(&mut self, deadline: Duration) -> Vec<u8>`，读不到就返回空，不返回
`Result`——调用方对所有失败的处理都一样，用错误类型区分它们只会诱导出没人需要的
分支）。测试实现喂两种情形：一个预设好的回复，和一个一直沉默的读端。

不做成泛型的 `Read + Write`：超时靠 `poll(2)`，它要的是一个裸 fd，而泛型 `Read`
给不出 fd。一个窄 trait 既保住了可测性，也不用假装 stdin 是任意的读端。

## 落地顺序

1. `src/theme.rs`：`Theme`、`dim()`、三个纯解析函数 + 它们的单测
2. 带 deadline 的 OSC 11 I/O 包装 + 它的两个单测
3. `detect()`：串起环境变量 / OSC / COLORFGBG / Unknown 四步
4. `ui.rs`：`OnceLock`、`run()` 里的 set、把 `DIM` 常量的 10 处引用换成 `theme::dim()`
   （其中两处是 `status_color` 内部，随该函数一起改名成 `status_style`）
5. 在深色和浅色终端上各跑一次，肉眼确认选 agent 菜单九行齐全、说明栏可读

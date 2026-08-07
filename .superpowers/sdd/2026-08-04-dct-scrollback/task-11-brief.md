### Task 11: 两份 README 跟上

**Files:**
- Modify: `README.md:90`、`README.md:149`、`README.md:104-116`
- Modify: `README.zh-CN.md:90`、`README.zh-CN.md:149`、`README.zh-CN.md:104-116`

**Interfaces:**
- Consumes: 前十个任务的成果
- Produces: 无代码

**语气要求：** 这两份 README 刚重写过，就是为了不像 AI 写的。第一人称、
承认缺点、不堆形容词、不用「无缝」「强大」「轻松」这类词。改动要跟周围
一个调子。

- [ ] **Step 1: 删掉「滚不了」那条，换成新的代价**

`README.md:90` 现在是：

```
Scrolling back doesn't work yet, and in iTerm2 it actively garbles the screen.
Scroll to the bottom and it repaints. The underlying reason is that `dct`
currently keeps zero scrollback, so there's nothing to scroll to; that's on the list.
```

换成：

```
Scrolling back works now, but it cost you something: while you're inside a
session, dct grabs the mouse, so your terminal's own click-and-drag text
selection stops working. In iTerm2 you hold Option to get it back; most
terminals have some equivalent. dct has no copy of its own yet. Back on the
board the mouse is yours again.
```

`README.zh-CN.md:90` 换成：

```
往回滚屏能用了，但它是有代价的：进了会话之后 dct 会接管鼠标，终端自己的
拖动选中就失灵了。iTerm2 里按住 Option 能拿回来，别的终端一般也有对应的
修饰键。dct 目前还没有自己的复制功能。退回看板鼠标就还给你了。
```

- [ ] **Step 2: 「还没做的」里删掉滚屏**

两份文件的最后一段（`:149`）里都有 `Scrollback` / `滚屏历史`，删掉这一项，
其余不动。

- [ ] **Step 3: 文件清单补三个新模块**

两份文件的 `src/` 清单（`:104-116`）里，`src/ui.rs` 那一行换成：

```
src/ui/          the TUI — one module per view, plus App and the shared widgets
src/restart.rs   dct restart
```

中文版：

```
src/ui/          界面：一个视图一个模块，外加 App 和公用的小部件
src/restart.rs   dct restart
```

- [ ] **Step 4: 看板键表加一行**

两份文件的键表里加 `dct restart` 说明不合适（那是命令不是键），改为在
「跑起来」那一节末尾加一句：

英文：

```
If you upgrade dct and it tells you the background service is out of date,
run `dct restart`. It lists whatever is still running and asks before killing anything.
```

中文：

```
升级完 dct 之后如果它说后台服务版本对不上，运行 `dct restart`。
它会先把还在跑的会话列出来，问过你再动手。
```

- [ ] **Step 5: 通读一遍**

Run: `git diff README.md README.zh-CN.md`
检查：两份内容对得上；没有 emoji；没有「无缝」「强大」这类词；
中文版读起来不像从英文直译的。

- [ ] **Step 6: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: README 跟上滚屏与 dct restart

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## 自查记录

**规格覆盖**

| 设计文档 | 任务 |
|---|---|
| 0.1 协议握手版本号 | Task 1 |
| 0.1 `dct restart` | Task 2 |
| 0.2 第一步：模块拆分 | Task 3、Task 5 |
| 0.2 第二步：`App` | Task 4 |
| 一、路由规则 | Task 6（`agent_owns`）、Task 10（`wheel_action` / `key_scroll`） |
| 一、第四种情况的提示 | Task 10（`scroll_hint`） |
| 一、坐标换算与编码分工 | Task 9（编码）、Task 10（换算） |
| 一、不转发纯移动 | Task 10（`handle_mouse` 的 `_ => (false, None)`） |
| 二、2000 行 | Task 6（`SCROLLBACK_ROWS`） |
| 二、钉住 | Task 6（`the_view_stays_put_when_new_output_arrives`） |
| 二、`new_lines` | Task 7 |
| 二、滚动区的坑 | Task 6（`a_scroll_region_swallows_the_history`） |
| 三、状态在守护进程 | Task 7 |
| 三、`Scroll` / `Mouse` / `ScrollState` | Task 8 |
| 四、打字与改尺寸归零 | Task 7 |
| 五、键位步长 | Task 10 |
| 五、捕获只在会话里开 | Task 10 |
| 五、代价写进 README | Task 11 |
| 六、全部测试项 | 各任务的 Step 1 |
| 七、明确不做 | 无任务（就是不做） |

无遗漏。

**类型一致性**

- `ScrollView`（pty 层，Task 6）→ `ScrollState`（协议层，Task 7 用 `state_of` 转换，多一个 `new_lines`）。两个名字不同是故意的：`new_lines` 要跨帧记忆，pty 层没有那个上下文。
- `ScrollBy` 在 Task 7 定义（`session.rs`），Task 8 在协议里引用同一个类型，不另建。
- `MouseForward` / `MouseForwardKind` 在 Task 8 定义（`proto.rs`），Task 9、10 引用。
- `PROTOCOL_VERSION` Task 1 建为 1，Task 8 改为 2。Task 1 的测试用的是常量本身，不写死数字，改版本不会让它们变红。

**测试数量推演**：172（起点）→ 175（T1）→ 178（T2）→ 178（T3 纯搬家）→ 180（T4）→ 180（T5 纯搬家）→ 189（T6）→ 194（T7）→ 197（T8）→ 205（T9）→ 217（T10）。每个任务的 Step「跑测试」里都写了预期数字，对不上就说明搬漏了或漏写了。

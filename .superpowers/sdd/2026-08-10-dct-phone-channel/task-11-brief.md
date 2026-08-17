## Task 11: 错误处理收尾与端到端实测

**Files:**
- Modify: `src/bridge.rs`、`src/ui/phone.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: 前面全部
- Produces: `backoff(attempt: u32) -> Duration`、`QUEUE_CAP`

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn backoff_grows_then_stops_growing() {
    assert!(backoff(0) < backoff(1));
    assert!(backoff(1) < backoff(2));
    // 上限 5 分钟：再久用户就会以为功能坏了
    assert_eq!(backoff(99), Duration::from_secs(300));
}

/// 队列满了丢最老的，**绝不阻塞 tick**。
#[test]
fn a_full_queue_drops_instead_of_blocking() {
    let b = Bridge::for_test();
    for i in 0..(QUEUE_CAP + 10) {
        b.enqueue(Event { session: i as u32, kind: EventKind::Stopped, name: "x".into(), project: "p".into() });
    }
    assert_eq!(b.queued(), QUEUE_CAP);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests::backoff -- --test-threads=1`
Expected: `cannot find function backoff`

- [ ] **Step 3: 实现**

指数退避 + 5 分钟上限；`BadToken` 不退避，直接置 `PhoneState::Broken("令牌被撤销了，按 Enter 重填")`；队列有界。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -- --test-threads=1`
Expected: 全绿

- [ ] **Step 5: 变异测试**

把上限 `min(300)` 去掉 —— `backoff_grows_then_stops_growing` 必须失败。把队列改成无界 —— 第二条必须失败。

- [ ] **Step 6: 端到端实测（需要真 bot token）**

**这一步不能跳，也不能用「单测全绿」代替。** 按 dct 的惯例，没跑过的一律记成没跑过。

1. 在 Telegram 找 @BotFather 建一个 bot，拿 token
2. `dct` → `l` → 手机通知 → 填 token → **断言页面显示「等你给 @xxx 发一条消息」并带上真实 bot 名**
3. 给 bot 发一句话 → **断言页面变成「已连上」**
4. 在某个项目里开一个 claude 会话，跟它说一句话，等它干完一轮 → **断言手机收到消息，且消息里没有路径、没有代码块**
5. 长按回复一句话 → **断言 agent 真的收到了，且手机上收到回执**
6. 直接发一句不带回复的话 → 断言按五条规则走
7. 发一个陌生账号给这个 bot 发消息 → **断言什么都没发生**（这条最重要）
8. 杀掉守护进程重启，长按回复步骤 5 那条旧消息 → **断言回「会话已经不在了」，且没有任何东西被敲进任何会话**

把每一条的真实结果写回 spec 的「未验证 / 风险」表。**跑不通的写成跑不通，不要写成待验证。**

- [ ] **Step 7: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add -A
git commit -m "feat: back off, cap the queue, and record what actually ran

A revoked token skips the backoff entirely -- retrying it forever would also
mean never telling the user to go re-enter it.

The end-to-end results go back into the spec's unverified table, including
the ones that failed. A green test suite is not evidence that the telegram
endpoints were ever called."
```

---

## Self-Review

**Spec 覆盖检查：**

| spec 小节 | 任务 |
|---|---|
| 架构：渠道住 daemon | Task 5 |
| 组件与边界 | Task 1, 2, 5 |
| 出站三事件 + 三道门 + 防抖 | Task 6 |
| 出站内容分两档（隐私边界） | Task 9 |
| 消息格式（无路径无 diff 无代码块） | Task 9 |
| 不碰 `Asking` | Task 6（tick 里不新增 `Asking` 的写入；本计划没有任何任务设置它） |
| 路由五条规则 | Task 7 |
| 主动发 `/ls` `/use` | Task 7（规则 2、5）+ Task 8（`NeedUse` 的回话） |
| 安全：只认一个 chat id | Task 5 |
| 回执 | Task 8 |
| journal | Task 8 |
| 智能四项 | Task 9（合并、编号选项）、Task 10（听懂回复、猜路由） |
| 红线：只转格式不造内容 | Task 10 |
| 错误处理表 | Task 11 |
| 重启后旧消息 | Task 7（`Gone`）+ Task 8（不写出去）+ Task 11 步骤 6.8（实测） |
| 界面：设置页改结构 | Task 3 |
| 界面：手机通知页四状态 | Task 4 |
| 令牌存 `SecretStore` 保留名 | Task 4 |
| 测试三条回归套 | Task 5（陌生人）、Task 6（新建会话）、Task 7+8（重启旧消息） |

**无遗漏。**

**类型一致性：** `MsgId`（Task 1）→ Task 2 解析、Task 7 映射键，一致。`Event`/`EventKind`（Task 1）→ Task 6 投递、Task 9 合并，一致。`Route`（Task 7）→ Task 8 `deliver`、Task 10 `narrow` 只作用于 `Ask`，一致。`PhoneState`（Task 4）→ Task 5 写 `Broken`、Task 11 写 `Broken`，一致。

**占位符扫描：** 无 TBD/TODO；每个代码步骤都有可运行的代码或明确的行为约定；Task 2 Step 5 与 Task 4 Step 3 描述的是补全动作，但都给了具体的判定条件和必须存在的测试。

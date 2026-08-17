## Task 7: 入站路由五条规则

**Files:**
- Modify: `src/bridge.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `MsgId`、Task 5 的 `Bridge`
- Produces: `RouteInput`、`Route { To(u32), Ask(Vec<u32>), Gone, NeedUse }`、`route(&RouteInput) -> Route`

- [ ] **Step 1: 写失败测试**

```rust
fn input<'a>(
    reply_to: Option<MsgId>,
    map: &'a HashMap<MsgId, u32>,
    used: Option<u32>,
    replied_since_use: bool,
    waiting: &'a [u32],
) -> RouteInput<'a> {
    RouteInput { reply_to, map, used, replied_since_use, waiting }
}

#[test]
fn a_reply_goes_where_it_replied() {
    let map = HashMap::from([(42, 7)]);
    assert_eq!(route(&input(Some(42), &map, Some(3), false, &[9])), Route::To(7));
}

/// **重启之后旧消息不能敲进任何地方。** 退化成「发给当前会话」正好是
/// 敲错地方的那条路径。
#[test]
fn a_reply_to_a_message_we_no_longer_know_types_nothing() {
    let map = HashMap::new();
    assert_eq!(route(&input(Some(42), &map, Some(3), false, &[9])), Route::Gone);
}

/// `/use` 压过「唯一在等」：用户切过去就是想跟那个会话说话，
/// 此刻另一个会话恰好在等，不能把他的话抢走。
#[test]
fn an_explicit_use_beats_a_waiting_session() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, Some(3), false, &[9])), Route::To(3));
}

/// 但用户一旦长按回复过某条推送，注意力已经转走，`/use` 的指定作废——
/// 否则一次 `/use` 会永久劫持所有不带回复的消息。
#[test]
fn use_expires_once_you_have_replied_to_a_push() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, Some(3), true, &[9])), Route::To(9));
}

#[test]
fn the_only_one_waiting_gets_it() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, None, false, &[9])), Route::To(9));
}

/// 好几个在等就不猜。敲错 agent 的代价比多问一句大得多。
#[test]
fn several_waiting_means_ask_not_guess() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, None, false, &[9, 10])), Route::Ask(vec![9, 10]));
}

#[test]
fn nothing_waiting_and_no_use_asks_for_ls() {
    let map = HashMap::new();
    assert_eq!(route(&input(None, &map, None, false, &[])), Route::NeedUse);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge::tests -- --test-threads=1`
Expected: `cannot find function route`

- [ ] **Step 3: 实现**

**顺序就是 spec 里那五条，不要重排：**

```rust
pub fn route(i: &RouteInput) -> Route {
    // 1. 带回复的：直接定位，永远不反问
    if let Some(m) = i.reply_to {
        return match i.map.get(&m) {
            Some(&s) => Route::To(s),
            // 守护进程重启过，映射没了。**绝不退化成「发给当前会话」**
            None => Route::Gone,
        };
    }
    // 2. 显式 /use 过、且那之后还没回复过任何推送
    if let (Some(u), false) = (i.used, i.replied_since_use) {
        return Route::To(u);
    }
    // 3. 只有一个在等
    if i.waiting.len() == 1 {
        return Route::To(i.waiting[0]);
    }
    // 4. 好几个在等：不猜（模型在这一条介入，见 Task 10）
    if i.waiting.len() > 1 {
        return Route::Ask(i.waiting.to_vec());
    }
    // 5. 没候选也没 /use 过
    Route::NeedUse
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把规则 2 和规则 3 调换顺序 —— `an_explicit_use_beats_a_waiting_session` 必须失败。把 `Route::Gone` 改成 `Route::To(i.used.unwrap_or(0))` —— `a_reply_to_a_message_we_no_longer_know_types_nothing` 必须失败。把 `replied_since_use` 的 `false` 改成 `true` —— `use_expires_once_you_have_replied_to_a_push` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs
git commit -m "feat: decide which agent a reply belongs to, or refuse to

Five rules in a fixed order. Two of them exist because of failure modes
rather than features: an explicit /use has to outrank the one session that
happens to be waiting, or your message gets stolen by it; and a reply to a
message from before a daemon restart types nothing at all, because falling
back to the current session is precisely the path that types into the wrong
terminal."
```

---


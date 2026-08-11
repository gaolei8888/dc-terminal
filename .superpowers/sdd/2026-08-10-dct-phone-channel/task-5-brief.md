## Task 5: bridge 骨架 —— 长轮询、配对、只认一个 chat id

**Files:**
- Create: `src/bridge.rs`
- Modify: `src/daemon.rs`（启动线程）、`src/lib.rs`
- Test: `src/bridge.rs` 内

**Interfaces:**
- Consumes: Task 1 的 `Channel`/`Incoming`、Task 4 的 `PhoneState`
- Produces: `Bridge::new(ch: Arc<dyn Channel>)`、`Bridge::accept(&self, msg: &Incoming) -> Accepted`、`Accepted { Paired(i64), FromOwner, Rejected }`

- [ ] **Step 1: 写失败测试**

**这是整个功能唯一真会伤到用户的地方，测试写在最前面。**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn msg(chat: i64, text: &str) -> Incoming {
        Incoming { text: text.into(), reply_to: None, chat_id: chat }
    }

    /// 第一个发消息的人成为主人。
    #[test]
    fn the_first_person_to_message_becomes_the_owner() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(111, "先跑完")), Accepted::FromOwner);
    }

    /// **bot 用户名是公开可搜的，任何人都能给它发消息，而这个功能会把消息
    /// 敲进用户的终端。** 这条测试破了就等于任何人都能往用户机器上敲字。
    #[test]
    fn a_stranger_is_rejected_even_after_pairing() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "在吗")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(222, "rm -rf /")), Accepted::Rejected);
        assert_eq!(b.accept(&msg(222, "/use 1")), Accepted::Rejected);
        // 主人还是主人，没被挤掉
        assert_eq!(b.accept(&msg(111, "继续")), Accepted::FromOwner);
    }

    /// 陌生人抢在主人之前发消息，就成了主人——这正是为什么配对必须是
    /// 用户填完令牌后的一次显式动作，而不是长期开着的门。
    /// 配对完成后 `accept` 再也不会返回 `Paired`。
    #[test]
    fn pairing_happens_exactly_once() {
        let b = Bridge::for_test();
        assert_eq!(b.accept(&msg(111, "hi")), Accepted::Paired(111));
        assert_eq!(b.accept(&msg(333, "hi")), Accepted::Rejected);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: `cannot find type Bridge`

- [ ] **Step 3: 实现**

```rust
//! 连接层：把守护进程里发生的事送到渠道上，把渠道上来的话敲进会话。
//!
//! **这是唯一有状态的地方**：谁是主人、哪条消息对应哪个会话、当前对着哪个
//! 会话。除此之外它什么都不存。
//!
//! **绝不 panic 到线程外面。** 手机通道死掉是遗憾，会话跟着死是灾难——
//! 同 `journal.rs` 那条「记不下来是记账的事，不该连累会话」。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepted {
    /// 这条消息完成了配对，发信人成为主人。
    Paired(i64),
    FromOwner,
    /// 不是主人发的，**丢弃**。
    Rejected,
}

pub struct Bridge {
    /// 配对之后只认这一个。`None` = 还没配对。
    owner: Mutex<Option<i64>>,
    // …… 消息映射与当前会话见 Task 7
}

impl Bridge {
    pub fn accept(&self, msg: &Incoming) -> Accepted {
        let mut owner = recover(self.owner.lock());
        match *owner {
            None => {
                *owner = Some(msg.chat_id);
                Accepted::Paired(msg.chat_id)
            }
            Some(o) if o == msg.chat_id => Accepted::FromOwner,
            Some(_) => Accepted::Rejected,
        }
    }
}
```

再写轮询线程：`loop { ch.poll(25s) }`，`ChannelError::worth_retrying()` 为真就指数退避（上限 5 分钟），为假就停下并把 `PhoneState::Broken(人话)` 写进状态槽。**整个线程体包在 `catch_unwind` 里**，panic 只让手机通道停掉。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib bridge:: -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `Some(o) if o == msg.chat_id` 的 `==` 改成 `!=` —— `a_stranger_is_rejected_even_after_pairing` 必须失败。把 `None` 分支改成不写 `owner`（每次都返回 `Paired`）—— `pairing_happens_exactly_once` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/bridge.rs src/daemon.rs src/lib.rs
git commit -m "feat: pair with exactly one person and ignore everyone else

A bot username is public and searchable, so anyone can message it -- and
this feature types what it receives into a terminal. The first message
after you enter the token claims ownership; every message from anyone else
is dropped, forever.

That is the one test in this feature that maps directly onto someone else
getting to type on your machine."
```

---


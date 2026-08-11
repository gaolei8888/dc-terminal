## Task 4: 协议、令牌存储、手机通知页

**Files:**
- Modify: `src/proto.rs`、`src/daemon.rs`、`src/secrets.rs`、`src/ui/view.rs`、`src/i18n.rs`
- Create: `src/ui/phone.rs`
- Test: `src/ui/phone.rs` 内 + `src/proto.rs` 内

**Interfaces:**
- Consumes: Task 3 的 `SettingsItem::Phone`
- Produces: `Request::PhoneStatus` / `PhoneSetToken { token }` / `PhoneUnpair` / `PhoneDisable`、`Response::Phone(PhoneStatus)`、`PhoneStatus { state: PhoneState, bot: Option<String>, owner: Option<String> }`、`PhoneState { Off, WaitingForPairing, Paired, Broken(String) }`、`View::Phone { status: PhoneStatus }`、`secrets::PHONE_TOKEN_KEY`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    /// 这一页存在的全部理由是那一行状态。四种取值，**每一种都要带下一步**——
    /// 一个不告诉用户下一步该干什么的错误，按房规就是没写完。
    #[test]
    fn every_state_tells_the_user_what_to_do_next() {
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Paired,
            PhoneState::Broken("token revoked".into()),
        ] {
            let s = status_line(
                &PhoneStatus { state: st.clone(), bot: Some("my_bot".into()), owner: None },
                Lang::Zh,
            );
            assert!(!s.is_empty(), "{st:?} 没有状态文案");
        }
        // 「已连上」是唯一不需要下一步的：它就是终点。其余三种都必须给出路。
        for st in [
            PhoneState::Off,
            PhoneState::WaitingForPairing,
            PhoneState::Broken("token revoked".into()),
        ] {
            let s = next_step(
                &PhoneStatus { state: st.clone(), bot: Some("my_bot".into()), owner: None },
                Lang::Zh,
            );
            assert!(s.is_some(), "{st:?} 没有给出下一步");
        }
        assert!(next_step(
            &PhoneStatus { state: PhoneState::Paired, bot: Some("my_bot".into()), owner: Some("lei".into()) },
            Lang::Zh
        ).is_none());
    }

    /// 等配对时必须把 bot 名字说出来，否则「去给它发条消息」是句没法执行的话。
    #[test]
    fn waiting_names_the_bot() {
        let s = status_line(
            &PhoneStatus { state: PhoneState::WaitingForPairing, bot: Some("my_dct_bot".into()), owner: None },
            Lang::Zh,
        );
        assert!(s.contains("my_dct_bot"), "等配对却没说是哪个 bot：{s}");
    }

    /// 令牌是密钥。**任何一处状态文案都不许把它带出来。**
    #[test]
    fn the_token_never_appears_in_any_status_text() {
        let st = PhoneStatus {
            state: PhoneState::Broken("123456:AAH-SECRET".into()),
            bot: None,
            owner: None,
        };
        let s = format!("{}{}", status_line(&st, Lang::Zh), next_step(&st, Lang::Zh).unwrap_or_default());
        assert!(!s.contains("AAH-SECRET"), "令牌漏进了界面文案：{s}");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::phone -- --test-threads=1`
Expected: `cannot find type PhoneStatus`

- [ ] **Step 3: 实现**

`proto.rs` 加类型（`PhoneState::Broken` 里装的是**已经成文的人话**，不是原始错误）：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhoneState {
    /// 还没填令牌
    Off,
    /// 填了、验过了，在等用户给 bot 发第一条消息
    WaitingForPairing,
    Paired,
    /// 连不上。**装的是人话**，不是原始错误文本——守护进程是唯一决定
    /// 用户看到什么文字的地方（`proto.rs` 顶上那条已有的约定）。
    Broken(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneStatus {
    pub state: PhoneState,
    /// bot 用户名，`getMe` 拿的。等配对那句话要用它。
    pub bot: Option<String>,
    /// 配上的主人，显示用。
    pub owner: Option<String>,
}
```

`secrets.rs` 加保留名字：

```rust
/// 手机通知的令牌存在密钥仓里，用一个 profile 不可能占用的名字。
///
/// **它不会出现在密钥页（`c`）里**，因为那一页遍历的是 profiles 再查
/// `has_secret`（见 `ui/pick.rs`），不是遍历这个文件的键。
/// 将来谁把密钥页改成遍历 `secrets.toml`，这个名字就会作为一个不存在的
/// agent 冒出来——改那里的人请回来看这一句。
pub const PHONE_TOKEN_KEY: &str = "__phone__";
```

`ui/phone.rs` 写 `status_line()` / `next_step()` 两个**纯函数** + `draw()` + `handle_key()`（`Enter` 填令牌、`r` 重新配对、`x` 关掉、`Esc` 回设置页）。

`Request::PhoneSetToken` 的处理走 `getMe` 验证，复用 `EnterSecret` 的 `SecretPhase::Verifying` 反馈。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib -- --test-threads=1`
Expected: PASS

- [ ] **Step 5: 变异测试**

把 `next_step` 里 `Paired` 那条分支改成也返回 `Some(...)`，第一个测试的最后一句断言必须失败。把 `status_line` 里 `WaitingForPairing` 分支的 bot 名字插值去掉，`waiting_names_the_bot` 必须失败。

- [ ] **Step 6: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/proto.rs src/daemon.rs src/secrets.rs src/ui/phone.rs src/ui/view.rs src/i18n.rs
git commit -m "feat: a page for the phone, because pairing is something you watch

Filling in a token is the easy half. The other half is that pairing is
asynchronous -- the daemon sits polling until you message the bot -- and
without somewhere to show that, you type a token and stare at a page that
does nothing.

Four states, each carrying its own next step, and a test that says the token
never reaches any of them."
```

---


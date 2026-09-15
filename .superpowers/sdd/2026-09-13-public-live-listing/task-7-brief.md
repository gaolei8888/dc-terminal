### Task 7: 守护进程 —— 公开意图、推帧线程、自愈

**Files:**
- Modify: `src/live.rs`
- Modify: `src/daemon.rs`

**Interfaces:**
- Consumes: Task 1 `publish_grant`、`push_hash`、`public_path`、`MAX_PUBLIC_TITLE_CHARS`；Task 6 全部。
- Produces:
  - `LiveState::publish(&self, title: String, key: String) -> Option<LiveInfo>`
  - `LiveState::unpublish(&self) -> Option<LiveInfo>`
  - `LiveState::grant(&self) -> Option<String>`
  - 纯函数 `fn public_action(want: Option<&str>, applied: bool, stuck: bool, rev: u64) -> PublicAction`，`enum PublicAction { Nothing, Put, Delete }`
  - 纯函数 `fn seen_public(want: Option<&str>, current: &LivePublic, relay_title: Option<String>) -> (LivePublic, bool /*需要重新 PUT*/)`
  - 守护进程处理 `LivePublish` / `LiveUnpublish` / `LivePublishGrant`

- [ ] **Step 1: 写失败的测试（纯函数与状态槽）**

`src/live.rs` 的 `mod tests` 追加：

```rust
    fn live_with_room() -> LiveState {
        let live = LiveState::new("https://x".to_string());
        live.start(vec![(1, "一".into())]);
        live
    }

    #[test]
    fn publishing_needs_a_room_and_records_pending() {
        let live = LiveState::new("https://x".to_string());
        assert!(live.publish("课".into(), "k".into()).is_none(), "没在播不能公开");
        let live = live_with_room();
        let info = live.publish("课".into(), "k".into()).unwrap();
        assert_eq!(info.public, LivePublic::Pending { title: "课".into() });
        assert_eq!(live.unpublish().unwrap().public, LivePublic::Private);
    }

    #[test]
    fn the_grant_is_the_shared_hmac_of_this_rooms_push_secret() {
        let live = live_with_room();
        let secret = live.push_secret().unwrap();
        let id = live.info().id;
        assert_eq!(
            live.grant().unwrap(),
            dct_link::live::publish_grant(&dct_link::live::push_hash(&secret), &id)
        );
        live.stop();
        assert!(live.grant().is_none());
    }

    /// 发布密钥不许从 `info()` 或 `Debug` 漏出去。
    #[test]
    fn the_publish_key_never_leaves_the_state() {
        let live = live_with_room();
        let info = live.publish("课".into(), "SUPER-SECRET-KEY".into()).unwrap();
        assert!(!serde_json::to_string(&info).unwrap().contains("SUPER-SECRET-KEY"));
        assert!(!format!("{info:?}").contains("SUPER-SECRET-KEY"));
    }

    #[test]
    fn what_to_tell_the_relay_about_publicity() {
        use PublicAction::*;
        assert_eq!(public_action(None, false, false, 0), Nothing, "从没碰过公开");
        assert_eq!(public_action(Some("课"), false, false, 1), Put);
        assert_eq!(public_action(Some("课"), true, false, 1), Nothing, "这一版已经送达");
        assert_eq!(public_action(Some("课"), false, true, 1), Nothing, "被 401/403/413 拒过，不重试");
        assert_eq!(public_action(None, false, false, 2), Delete, "撤销过公开");
        assert_eq!(public_action(None, true, false, 2), Nothing);
    }

    /// 以中转为准，外加唯一一条自愈规则。
    #[test]
    fn reading_publicity_back_from_the_relay() {
        let listed = LivePublic::Listed { title: "课".into() };
        // 本机不想公开：照实显示中转说的（管理台代为公开的就是这种）
        assert_eq!(seen_public(None, &LivePublic::Private, Some("课".into())), (listed.clone(), false));
        assert_eq!(seen_public(None, &listed, None), (LivePublic::Private, false));
        // 本机想公开、中转也说公开
        assert_eq!(seen_public(Some("课"), &LivePublic::Pending { title: "课".into() }, Some("课".into())), (listed.clone(), false));
        // 本机想公开、之前已上列表、中转却说不公开 → 回到 Pending 并要求再 PUT 一次
        assert_eq!(seen_public(Some("课"), &listed, None), (LivePublic::Pending { title: "课".into() }, true));
        // 已经 Failed（被拒）的，中转说不公开是意料之中：保持失败，不重试
        let failed = LivePublic::Failed { title: "课".into(), reason: LiveFailure::Refused(401) };
        assert_eq!(seen_public(Some("课"), &failed, None), (failed.clone(), false));
    }
```

- [ ] **Step 2: 写失败的测试（推帧线程对假中转）**

扩展 `FakeState`：

```rust
        /// `PUT .../public` 要答的状态码（默认 204）。
        public_status: u16,
        /// `GET .../lanes` 答复里的 `public` 标题（默认 `None`）。
        lanes_public: Option<String>,
```

（`#[derive(Default)]` 下 `public_status` 会是 0，`serve_one` 里当 0 为 204。）`serve_one` 里，把现在的

```rust
        let lanes_query = path.ends_with("/lanes");
        st.seen.push(Seen {
```

改成

```rust
        let lanes_query = path.ends_with("/lanes");
        let public_put = method == "PUT" && path.ends_with("/public");
        let lanes_public = st.lanes_public.clone();
        let public_status = st.public_status;
        st.seen.push(Seen {
```

再把 `drop(st);` 之后的答复分支整个换成：

```rust
        let out = if lanes_query {
            let public = match lanes_public {
                Some(t) => format!(r#"{{"title":"{t}"}}"#),
                None => "null".to_string(),
            };
            let json = format!(r#"{{"lanes":["前端"],"viewers":5,"public":{public}}}"#);
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}", json.len())
        } else if public_put && public_status != 0 && public_status != 204 {
            format!("HTTP/1.1 {public_status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        } else {
            "HTTP/1.1 204 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        };
```

（`st_public_title` / `st_public_status` 是在持锁时 `st.lanes_public.clone()` / `st.public_status` 取出来的局部变量——实现者按现有 `serve_one` 的锁范围摆放。）

追加测试（沿用本文件已有的 `until(...)` 和起推帧线程的方式；`grep -n "spawn_pusher(" src/live.rs` 看既有测试怎么起线程、怎么造 `SessionManager`）：

```rust
    /// 公开：推帧线程带着推帧钥匙和发布密钥去 PUT，成功后状态翻成 Listed。
    #[test]
    fn the_pusher_publishes_with_the_push_secret_and_the_key() {
        let srv = FakeSrv::start();
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        live.publish("第3课".into(), "the-key".into());
        let pusher = spawn_pusher(live.clone(), mgr);
        until("PUT 发出去", || srv.seen().iter().any(|s| s.method == "PUT" && s.path.ends_with("/public")));
        let put = srv.seen().into_iter().find(|s| s.method == "PUT").unwrap();
        assert_eq!(put.headers.get("x-live-push"), live.push_secret().as_ref());
        let body = String::from_utf8(put.body).unwrap();
        assert!(body.contains(r#""title":"第3课""#) && body.contains(r#""key":"the-key""#), "{body}");
        until("状态翻成 Listed", || live.info().public == LivePublic::Listed { title: "第3课".into() });
        pusher.stop();
    }

    /// 被 401 拒：Failed，而且**不再重试**。
    #[test]
    fn a_refused_publication_is_not_retried() {
        let srv = FakeSrv::start();
        recover(srv.state.lock()).public_status = 401;
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        live.publish("课".into(), "bad".into());
        let pusher = spawn_pusher(live.clone(), mgr);
        until("失败", || matches!(live.info().public, LivePublic::Failed { .. }));
        std::thread::sleep(PUSH_INTERVAL * 6);
        let puts = srv.seen().iter().filter(|s| s.method == "PUT" && s.path.ends_with("/public")).count();
        assert_eq!(puts, 1, "被拒之后又去撞了中转 {puts} 次");
        pusher.stop();
    }

    /// 取消公开：发 DELETE。
    #[test]
    fn unpublishing_sends_a_delete() {
        let srv = FakeSrv::start();
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        live.publish("课".into(), "k".into());
        let pusher = spawn_pusher(live.clone(), mgr);
        until("先公开", || matches!(live.info().public, LivePublic::Listed { .. }));
        live.unpublish();
        until("DELETE 发出去", || srv.seen().iter().any(|s| s.method == "DELETE" && s.path.ends_with("/public")));
        pusher.stop();
    }

    /// 管理台代为公开：本机没请求过，但中转的 lanes 说公开 → 界面显示 Listed，且本机不发 PUT。
    #[test]
    fn a_publication_made_elsewhere_is_reflected_without_a_put() {
        let srv = FakeSrv::start();
        recover(srv.state.lock()).lanes_public = Some("管理台公开的".into());
        let live = Arc::new(LiveState::new(srv.base()));
        let mgr = Arc::new(crate::session::SessionManager::new());
        live.start(vec![]);
        let pusher = spawn_pusher(live.clone(), mgr);
        until("读到公开", || live.info().public == LivePublic::Listed { title: "管理台公开的".into() });
        assert!(!srv.seen().iter().any(|s| s.method == "PUT"), "不是本机发起的公开，本机不许 PUT");
        pusher.stop();
    }
```

（`live.start(vec![])` 空名单跟既有推帧线程测试一样：这里测的是开房与公开，不需要真的会话。）

`src/daemon.rs` 测试模块追加（沿用 `bare_handle_deps`、`one_staged_session`、`test_live` 等助手）：

```rust
    #[test]
    fn live_publish_without_a_key_is_refused_with_a_code() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let (id, _dir) = one_staged_session(&mgr);
        let live = test_live();
        live.start(vec![(id, "一".into())]);
        let resp = handle(Request::LivePublish { title: "课".into() }, &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(), &live);
        assert!(matches!(resp, Response::Error(ErrorCode::LivePublishKeyMissing)), "{resp:?}");
    }

    #[test]
    fn live_publish_uses_the_stored_key_and_truncates_the_title() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let (id, _dir) = one_staged_session(&mgr);
        recover(secrets.lock()).set(crate::secrets::LIVE_PUBLISH_KEY, "k").unwrap();
        let live = test_live();
        live.start(vec![(id, "一".into())]);
        let long = "字".repeat(dct_link::live::MAX_PUBLIC_TITLE_CHARS + 5);
        let resp = handle(Request::LivePublish { title: long }, &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(), &live);
        let Response::Live(info) = resp else { panic!("{resp:?}") };
        let LivePublic::Pending { title } = info.public else { panic!("{:?}", info.public) };
        assert_eq!(title.chars().count(), dct_link::live::MAX_PUBLIC_TITLE_CHARS);
    }

    #[test]
    fn a_grant_needs_a_live_room() {
        let (mgr, store, secrets, profiles_dir) = bare_handle_deps();
        let live = test_live();
        let resp = handle(Request::LivePublishGrant, &mgr, &store, &secrets, profiles_dir.path(),
            &test_phone(), &test_bridge(), &test_event_tx(), None, &test_pairs(), &live);
        assert!(matches!(resp, Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive))), "{resp:?}");
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test --lib live:: daemon::`
Expected: 编译失败。

- [ ] **Step 4: 实现 `src/live.rs`**

1. `Room` 加字段：

```rust
    /// 本机想让它公开时的标题；`None` = 本机没请求公开（可能是管理台公开的）。
    want_public: Option<String>,
    /// 本机请求公开时拿到的发布密钥。**只活在守护进程内存里**，不进 `LiveInfo`。
    public_key: Option<String>,
    /// 界面看到的公开状态，见 `LivePublic`。
    public: LivePublic,
    /// 公开意图改过几次。推帧线程靠它认出「要重新 PUT/DELETE」。
    public_rev: u64,
```

`start()` 里新房间这四个字段为 `None, None, LivePublic::Private, 0`；`info()` / `restage()` 构造 `LiveInfo` 时带 `public: room.public.clone()`（没在播时 `LivePublic::Private`）。
2. `RoomSnapshot` 加 `want_public: Option<String>`、`public_key: Option<String>`、`public_rev: u64`，`snapshot()` 抄过去。
3. `impl LiveState` 加：

```rust
    pub fn publish(&self, title: String, key: String) -> Option<LiveInfo> {
        {
            let mut guard = recover(self.room.lock());
            let room = guard.as_mut()?;
            room.public = LivePublic::Pending { title: title.clone() };
            room.want_public = Some(title);
            room.public_key = Some(key);
            room.public_rev += 1;
        }
        Some(self.info())
    }

    pub fn unpublish(&self) -> Option<LiveInfo> {
        {
            let mut guard = recover(self.room.lock());
            let room = guard.as_mut()?;
            room.public = LivePublic::Private;
            room.want_public = None;
            room.public_key = None;
            room.public_rev += 1;
        }
        Some(self.info())
    }

    /// 管理台用的公开凭证。没在播是 `None`。
    pub fn grant(&self) -> Option<String> {
        recover(self.room.lock()).as_ref().map(|r| {
            dct_link::live::publish_grant(&dct_link::live::push_hash(&r.push_secret), &r.id)
        })
    }

    pub(crate) fn mark_public(&self, id: &str, rev: u64, public: LivePublic) {
        if let Some(room) = recover(self.room.lock()).as_mut() {
            if room.id == id && room.public_rev == rev {
                room.public = public;
            }
        }
    }

    /// 推帧线程把中转说的公开状态写回；回 `true` 表示要重新 PUT 一次（自愈）。
    pub(crate) fn set_public_seen(&self, id: &str, relay_title: Option<String>) -> bool {
        let mut guard = recover(self.room.lock());
        let Some(room) = guard.as_mut() else { return false };
        if room.id != id {
            return false;
        }
        let (next, heal) = seen_public(room.want_public.as_deref(), &room.public, relay_title);
        room.public = next;
        heal
    }
```

（`push_secret()` 上的 `#[allow(dead_code)]` 若因此不再需要就删掉。）
4. 纯函数：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicAction {
    Nothing,
    Put,
    Delete,
}

/// 这一版公开意图（`rev`）该对中转做什么。`applied` = 这一版已经送达；
/// `stuck` = 这一版被 401/403/413 拒过（不重试，直到老师再按一次 `p`）。
fn public_action(want: Option<&str>, applied: bool, stuck: bool, rev: u64) -> PublicAction {
    if applied || stuck {
        return PublicAction::Nothing;
    }
    match want {
        Some(_) => PublicAction::Put,
        None if rev > 0 => PublicAction::Delete,
        None => PublicAction::Nothing,
    }
}

/// 以中转为准写回公开状态，并判断要不要自愈。唯一的自愈规则：本机想公开、
/// 之前已上列表、中转却说不公开 → 回到 Pending，再 PUT 一次，由那次答复定结果。
fn seen_public(want: Option<&str>, current: &LivePublic, relay_title: Option<String>) -> (LivePublic, bool) {
    match (want, relay_title) {
        (None, Some(t)) => (LivePublic::Listed { title: t }, false),
        (None, None) => (LivePublic::Private, false),
        (Some(_), Some(t)) => (LivePublic::Listed { title: t }, false),
        (Some(w), None) => match current {
            LivePublic::Listed { .. } => (LivePublic::Pending { title: w.to_string() }, true),
            other => (other.clone(), false),
        },
    }
}
```

5. HTTP 小函数：

```rust
#[derive(serde::Serialize)]
struct PublishBody<'a> {
    title: &'a str,
    key: &'a str,
}

fn put_public(agent: &ureq::Agent, base: &str, id: &str, secret: &str, title: &str, key: &str) -> Result<(), LiveFailure> {
    let url = format!("{base}{}", dct_link::live::public_path(id));
    match agent.put(&url).set("x-live-push", secret).send_json(PublishBody { title, key }) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => Err(LiveFailure::Refused(code)),
        Err(ureq::Error::Transport(_)) => Err(LiveFailure::Unreachable),
    }
}

fn delete_public(agent: &ureq::Agent, base: &str, id: &str, secret: &str) -> bool {
    let url = format!("{base}{}", dct_link::live::public_path(id));
    agent.delete(&url).set("x-live-push", secret).call().is_ok()
}
```

`LanesBody` 加 `#[serde(default)] public: Option<PublicTitleBody>`，`struct PublicTitleBody { title: String }`；`fetch_viewers` 改名为 `fetch_lanes`，返回 `Option<(u32, Option<String>)>`，并更新 `fetch_viewers_asks_the_lanes_route_with_the_read_only_token` 测试里的调用。
6. `pusher_loop`：在 `started` 相关变量旁加

```rust
    // 这一场、这一版公开意图（`(id, public_rev)`）已经送达 / 被拒不再重试。
    let mut public_applied: Option<(String, u64)> = None;
    let mut public_stuck: Option<(String, u64)> = None;
    // 连不上中转时的下一次重试时间，复用开播的退避表。
    let mut public_retry_at: Option<Instant> = None;
    let mut public_fails: u32 = 0;
```

在 `start_room` 成功的那一支（`started = Some(this);` 之后）加 `public_applied = None; public_stuck = None;`——房间在中转上是新开的，公开状态不继承，需要重新送达。在读人数那一段之前插入：

```rust
                let pub_this = (room.id.clone(), room.public_rev);
                let due = public_retry_at.is_none_or(|t| Instant::now() >= t);
                if due {
                    match public_action(
                        room.want_public.as_deref(),
                        public_applied.as_ref() == Some(&pub_this),
                        public_stuck.as_ref() == Some(&pub_this),
                        room.public_rev,
                    ) {
                        PublicAction::Nothing => {}
                        PublicAction::Put => {
                            let title = room.want_public.clone().unwrap_or_default();
                            let key = room.public_key.clone().unwrap_or_default();
                            match put_public(&agent, &base, &room.id, &room.push_secret, &title, &key) {
                                Ok(()) => {
                                    live.mark_public(&room.id, room.public_rev, LivePublic::Listed { title });
                                    public_applied = Some(pub_this);
                                    public_fails = 0;
                                    public_retry_at = None;
                                }
                                Err(LiveFailure::Refused(code)) if matches!(code, 401 | 403 | 413) => {
                                    live.mark_public(&room.id, room.public_rev, LivePublic::Failed { title, reason: LiveFailure::Refused(code) });
                                    public_stuck = Some(pub_this);
                                }
                                Err(reason) => {
                                    live.mark_public(&room.id, room.public_rev, LivePublic::Failed { title, reason });
                                    public_fails += 1;
                                    public_retry_at = Some(Instant::now() + start_backoff(public_fails));
                                }
                            }
                        }
                        PublicAction::Delete => {
                            if delete_public(&agent, &base, &room.id, &room.push_secret) {
                                public_applied = Some(pub_this);
                            }
                        }
                    }
                }
```

读人数那一段改成：

```rust
                    if let Some((n, public)) = fetch_lanes(&agent, &base, &room.id, &room.token) {
                        live.set_viewers(&room.id, n);
                        if live.set_public_seen(&room.id, public) {
                            // 自愈：让下一轮重新送达这一版公开意图。
                            public_applied = None;
                        }
                    }
```

`None =>`（没在播）那一支把四个公开变量都清掉。
7. `start_backoff` 若是私有函数，保持同一文件内调用即可。

- [ ] **Step 5: 实现 `src/daemon.rs`**

在 `Request::LiveStatus => ...` 之后加：

```rust
        Request::LivePublish { title } => Ok(live_publish(live, secrets, title)),
        Request::LiveUnpublish => Ok(match live.unpublish() {
            Some(info) => Response::Live(info),
            None => Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive)),
        }),
        Request::LivePublishGrant => Ok(match live.grant() {
            Some(g) => Response::LiveGrant(crate::proto::LiveGrantToken(g)),
            None => Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive)),
        }),
```

并加函数（`secrets` 的具体类型按 `handle` 签名写）：

```rust
/// 公开这场直播：用本机存的发布密钥。**密钥从这里进 `LiveState`，不经过任何响应。**
/// 标题截到中转收得下的长度，理由同路名（`validated_staging`）。
fn live_publish(
    live: &Arc<crate::live::LiveState>,
    secrets: &Arc<Mutex<SecretStore>>,
    title: String,
) -> Response {
    let Some(key) = recover(secrets.lock()).get(crate::secrets::LIVE_PUBLISH_KEY).map(str::to_string) else {
        return Response::Error(ErrorCode::LivePublishKeyMissing);
    };
    let title: String = title.trim().chars().take(dct_link::live::MAX_PUBLIC_TITLE_CHARS).collect();
    match live.publish(title, key) {
        Some(info) => Response::Live(info),
        None => Response::Error(ErrorCode::LiveStagingRejected(LiveStagingProblem::NotLive)),
    }
}
```

- [ ] **Step 6: 跑测试确认通过**

Run: `env -u TERM cargo test --workspace --no-fail-fast`
Expected: PASS。

- [ ] **Step 7: 变异检查**

`public_action` 里 `applied || stuck` 改成 `applied`（`a_refused_publication_is_not_retried` 红）；`seen_public` 的 `(None, Some(t))` 改成 `(LivePublic::Private, false)`（`a_publication_made_elsewhere...` 红）；`start_room` 成功支删掉 `public_applied = None;`（写一条临时测试或确认 `reading_publicity_back_from_the_relay` + 手工推理，报告里写明结论）；`live_publish` 不截断标题（`live_publish_uses_the_stored_key_and_truncates_the_title` 红）。改回。

- [ ] **Step 8: Commit**

```bash
git add src/live.rs src/daemon.rs
git commit -m "feat(live): the daemon publishes with its stored key and heals publicity after relay restarts"
```

---


### Task 3: `dct-srv` —— 房间公开状态

**Files:**
- Modify: `crates/dct-srv/src/live.rs`

**Interfaces:**
- Consumes: Task 1 的 `MAX_PUBLIC_TITLE_CHARS`、`RESERVED_LIVE_ID`、`publish_grant`；Task 2 的 `keys::PublishKeys`（`name_for`、`is_blocked`）。
- Produces:
  - `pub enum Control<'a> { Push(&'a str), Grant(&'a str) }`
  - `pub struct PublicEntry { pub id: String, pub title: String, pub lanes: Vec<String>, pub viewers: u32 }`（`Serialize`）
  - `Live::publish(&self, id: &str, control: Control, keys: Option<&PublishKeys>, key: &str, title: &str) -> Result<(), LinkError>`
  - `Live::unpublish(&self, id: &str, control: Control) -> Result<(), LinkError>`
  - `Live::frame(&self, id: &str, token: Option<&str>, lane: usize) -> Result<(Vec<u8>, u64), LinkError>`（**签名变**：`token` 变 `Option`）
  - `Live::lanes(&self, id: &str, token: Option<&str>) -> Result<(Vec<String>, Option<String>), LinkError>`（**签名变**：多回公开标题）
  - `Live::public_list(&self) -> Vec<PublicEntry>`
  - `Live::reconcile(&self, keys: Option<&PublishKeys>)`

- [ ] **Step 1: 写失败的测试**

在 `crates/dct-srv/src/live.rs` 的 `mod tests` 里追加（`started()` 已存在：房间 `abc`、viewer `"t"*64`、push `"p"*64`、两路）：

```rust
    fn keys_with(name: &str) -> (crate::keys::PublishKeys, String) {
        let mut k = crate::keys::PublishKeys::default();
        let key = k.add(name, 0).unwrap();
        (k, key)
    }

    /// 公开的鉴权矩阵，逐行对应 spec 那张表。
    #[test]
    fn publishing_follows_the_answer_table() {
        let live = started();
        let (keys, key) = keys_with("姜老师");
        let push = "p".repeat(64);
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&push), "abc");

        // 房间不存在 / 推帧钥匙不对 / 凭证不对 → 401
        assert_eq!(live.publish("nope", Control::Push(&push), Some(&keys), &key, "课"), Err(LinkError::Unauthorized));
        assert_eq!(live.publish("abc", Control::Push(&"x".repeat(64)), Some(&keys), &key, "课"), Err(LinkError::Unauthorized));
        assert_eq!(live.publish("abc", Control::Grant("00"), Some(&keys), &key, "课"), Err(LinkError::Unauthorized));
        // 没开公开功能 → 403
        assert_eq!(live.publish("abc", Control::Push(&push), None, &key, "课"), Err(LinkError::NotYours));
        // 发布密钥不对 → 401
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &"0".repeat(64), "课"), Err(LinkError::Unauthorized));
        // 标题越界 → 413
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &key, ""), Err(LinkError::TooBig));
        let long = "字".repeat(dct_link::live::MAX_PUBLIC_TITLE_CHARS + 1);
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &key, &long), Err(LinkError::TooBig));
        // 成功：推帧钥匙和凭证都行
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&keys), &key, "第3课"), Ok(()));
        assert_eq!(live.publish("abc", Control::Grant(&grant), Some(&keys), &key, "第4课"), Ok(()));
        assert_eq!(live.public_list()[0].title, "第4课", "重复公开覆盖标题");
        // 黑名单 → 403
        let mut blocked = keys.clone();
        blocked.block("abc");
        assert_eq!(live.publish("abc", Control::Push(&push), Some(&blocked), &key, "课"), Err(LinkError::NotYours));
    }

    /// 凭证只认公开这件事：推帧、停播都不认它。
    #[test]
    fn a_grant_cannot_push_or_stop() {
        let live = started();
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&"p".repeat(64)), "abc");
        assert_eq!(live.push("abc", &grant, 0, b"x".to_vec()), Err(LinkError::Unauthorized));
        assert_eq!(live.stop("abc", &grant), Err(LinkError::Unauthorized));
    }

    #[test]
    fn unpublishing_is_idempotent_and_needs_control() {
        let live = started();
        let (keys, key) = keys_with("a");
        let push = "p".repeat(64);
        assert_eq!(live.unpublish("abc", Control::Push(&push)), Ok(()), "没公开也回成功");
        live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
        assert_eq!(live.unpublish("abc", Control::Push(&"x".repeat(64))), Err(LinkError::Unauthorized));
        assert_eq!(live.unpublish("abc", Control::Push(&push)), Ok(()));
        assert!(live.public_list().is_empty());
    }

    /// 无令牌只能读公开的房间；私密、不存在的房间对无令牌请求同一个 401；
    /// 带错令牌不因为房间公开而放行。
    #[test]
    fn tokenless_reads_only_reach_public_rooms() {
        let live = started();
        let (keys, key) = keys_with("a");
        let push = "p".repeat(64);
        live.push("abc", &push, 0, b"hi".to_vec()).unwrap();

        assert_eq!(live.frame("abc", None, 0).err(), Some(LinkError::Unauthorized), "私密房间");
        assert_eq!(live.frame("zzz", None, 0).err(), Some(LinkError::Unauthorized), "不存在的房间");
        assert_eq!(live.lanes("abc", None).err(), Some(LinkError::Unauthorized));

        live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
        assert_eq!(live.frame("abc", None, 0).unwrap().0, b"hi");
        assert_eq!(live.lanes("abc", None).unwrap(), (vec!["前端".to_string(), "后端".to_string()], Some("课".to_string())));
        assert_eq!(live.frame("abc", Some(&"w".repeat(64)), 0).err(), Some(LinkError::Unauthorized), "带错令牌照拒");
        assert_eq!(live.lanes("abc", Some(&"t".repeat(64))).unwrap().1, Some("课".to_string()));
    }

    #[test]
    fn the_public_list_has_title_lanes_viewers_and_no_publisher() {
        let live = started();
        let (keys, key) = keys_with("姜老师");
        live.publish("abc", Control::Push(&"p".repeat(64)), Some(&keys), &key, "第3课").unwrap();
        let list = live.public_list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "abc");
        assert_eq!(list[0].lanes, vec!["前端", "后端"]);
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains("姜老师"), "列表里不许出现发布者：{json}");
    }

    /// 吊销、下线、关总开关：重载之后公开状态立刻收回。
    #[test]
    fn reconcile_withdraws_revoked_blocked_and_disabled_publications() {
        let push = "p".repeat(64);
        for case in ["revoke", "block", "disable"] {
            let live = started();
            let (mut keys, key) = keys_with("a");
            live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
            match case {
                "revoke" => {
                    keys.revoke("a");
                    live.reconcile(Some(&keys));
                }
                "block" => {
                    keys.block("abc");
                    live.reconcile(Some(&keys));
                }
                _ => live.reconcile(None),
            }
            assert!(live.public_list().is_empty(), "{case} 之后还挂在公开列表上");
            assert_eq!(live.frame("abc", None, 0).err(), Some(LinkError::Unauthorized), "{case}");
        }
    }

    #[test]
    fn reopening_a_room_does_not_inherit_its_public_state() {
        let live = started();
        let (keys, key) = keys_with("a");
        let push = "p".repeat(64);
        live.publish("abc", Control::Push(&push), Some(&keys), &key, "课").unwrap();
        live.start("abc".into(), "t".repeat(64), push, vec!["前端".into()]).unwrap();
        assert!(live.public_list().is_empty());
    }

    #[test]
    fn the_reserved_word_cannot_be_a_room_id() {
        assert_eq!(
            Live::new().start(dct_link::live::RESERVED_LIVE_ID.into(), "t".repeat(64), "p".repeat(64), vec!["x".into()]),
            Err(LinkError::Unauthorized)
        );
    }
```

同时把本文件既有测试里对 `frame(id, token, lane)` / `lanes(id, token)` 的调用改成 `frame(id, Some(token), lane)` / `lanes(id, Some(token))`，`lanes` 的返回值从 `Vec` 改成取 `.0`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-srv --lib live::`
Expected: 编译失败（`Control`、`publish` 等未定义）。

- [ ] **Step 3: 实现**

在 `crates/dct-srv/src/live.rs`：

1. `use` 里加 `dct_link::live::{MAX_PUBLIC_TITLE_CHARS, RESERVED_LIVE_ID}` 和 `crate::keys::PublishKeys`。
2. `Session` 加字段：

```rust
    /// 这场直播是否公开，以及是哪把发布密钥公开的（名字只给服务器上的人看，
    /// 不进公开列表）。**重开同一个房间号不继承它**。
    public: Option<Public>,
```

并在文件里定义：

```rust
struct Public {
    title: String,
    key_name: String,
}

/// 证明「你控制这场直播」的两种方式。
pub enum Control<'a> {
    /// 老师本机守护进程的推帧钥匙。
    Push(&'a str),
    /// 守护进程签发给管理台的凭证，见 `dct_link::live::publish_grant`。
    Grant(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PublicEntry {
    pub id: String,
    pub title: String,
    pub lanes: Vec<String>,
    pub viewers: u32,
}
```

3. `start` 里，在「id 只许字母数字和 `-`/`_`」那个 `if` 的条件里加 `|| id == RESERVED_LIVE_ID`；`rooms.insert(...)` 的 `Session { ... }` 加 `public: None,`。
4. 新增：

```rust
/// 这一方到底控不控制这个房间。常数时间，理由同 `same`。
fn controls(room: &Session, id: &str, control: &Control) -> bool {
    match control {
        Control::Push(secret) => same(&room.push_hash, &hash(secret)),
        Control::Grant(grant) => {
            let want = dct_link::live::publish_grant(&room.push_hash, id);
            same(&hash(&want), &hash(grant))
        }
    }
}
```

并在 `impl Live` 里追加：

```rust
    /// 公开这场直播。判断顺序就是 spec 那张答复表的顺序：先证明控制房间
    /// （401 不给探测差别），再看总开关（403），再验发布密钥（401），再看
    /// 黑名单（403），最后标题（413）。
    pub fn publish(
        &self,
        id: &str,
        control: Control,
        keys: Option<&PublishKeys>,
        key: &str,
        title: &str,
    ) -> Result<(), LinkError> {
        let mut rooms = self.rooms.lock().expect("live 锁");
        let room = rooms.get_mut(id).ok_or(LinkError::Unauthorized)?;
        if !controls(room, id, &control) {
            return Err(LinkError::Unauthorized);
        }
        let keys = keys.ok_or(LinkError::NotYours)?;
        let key_name = keys.name_for(key).ok_or(LinkError::Unauthorized)?;
        if keys.is_blocked(id) {
            return Err(LinkError::NotYours);
        }
        let n = title.chars().count();
        if n == 0 || n > MAX_PUBLIC_TITLE_CHARS {
            return Err(LinkError::TooBig);
        }
        room.public = Some(Public { title: title.to_string(), key_name });
        Ok(())
    }

    /// 取消公开。**幂等**：没公开也回成功。
    pub fn unpublish(&self, id: &str, control: Control) -> Result<(), LinkError> {
        let mut rooms = self.rooms.lock().expect("live 锁");
        let room = rooms.get_mut(id).ok_or(LinkError::Unauthorized)?;
        if !controls(room, id, &control) {
            return Err(LinkError::Unauthorized);
        }
        room.public = None;
        Ok(())
    }

    /// 公开列表，按在看人数降序。**不含发布者。**
    pub fn public_list(&self) -> Vec<PublicEntry> {
        let rooms = self.rooms.lock().expect("live 锁");
        let mut list: Vec<PublicEntry> = rooms
            .iter()
            .filter_map(|(id, r)| {
                r.public.as_ref().map(|p| PublicEntry {
                    id: id.clone(),
                    title: p.title.clone(),
                    lanes: r.lanes.iter().map(|l| l.name.clone()).collect(),
                    viewers: r.lanes.iter().map(|l| l.tx.receiver_count() as u32).sum(),
                })
            })
            .collect();
        list.sort_by(|a, b| b.viewers.cmp(&a.viewers).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// 密钥文件重载之后调：吊销了的密钥公开的、被下线的、以及总开关关掉时的
    /// 全部公开，立刻收回。
    pub fn reconcile(&self, keys: Option<&PublishKeys>) {
        let mut rooms = self.rooms.lock().expect("live 锁");
        for (id, room) in rooms.iter_mut() {
            let keep = match (keys, room.public.as_ref()) {
                (_, None) => continue,
                (None, Some(_)) => false,
                (Some(k), Some(p)) => {
                    !k.is_blocked(id) && k.keys.iter().any(|e| e.name == p.key_name)
                }
            };
            if !keep {
                room.public = None;
            }
        }
    }
```

5. 把 `authed` 改成接 `Option<&str>`：

```rust
/// 认证在取数之前，而且**认不出来和不存在回同一句话**。
///
/// 没带令牌（`None`）：只有公开的房间放行。带了令牌：照旧只认 viewer 那把，
/// 带错的**不会**因为房间公开而被放行。
fn authed<'a>(
    rooms: &'a HashMap<String, Session>,
    id: &str,
    token: Option<&str>,
) -> Result<&'a Session, LinkError> {
    let room = rooms.get(id).ok_or(LinkError::Unauthorized)?;
    match token {
        None if room.public.is_some() => Ok(room),
        None => Err(LinkError::Unauthorized),
        Some(t) if same(&room.viewer_hash, &hash(t)) => Ok(room),
        Some(_) => Err(LinkError::Unauthorized),
    }
}
```

6. `frame` / `lanes` 改签名：

```rust
    pub fn frame(&self, id: &str, token: Option<&str>, lane: usize) -> Result<(Vec<u8>, u64), LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        let slot = room.lanes.get(lane).ok_or(LinkError::Unauthorized)?;
        Ok((slot.frame.clone(), slot.etag))
    }

    /// 路名，以及公开标题（私密房间是 `None`）。推帧线程靠这第二个值得知
    /// 「我这场现在公不公开」——包括管理台代为公开的情况。
    pub fn lanes(&self, id: &str, token: Option<&str>) -> Result<(Vec<String>, Option<String>), LinkError> {
        let rooms = self.rooms.lock().expect("live 锁");
        let room = authed(&rooms, id, token)?;
        Ok((
            room.lanes.iter().map(|l| l.name.clone()).collect(),
            room.public.as_ref().map(|p| p.title.clone()),
        ))
    }
```

`lib.rs` 里 `live_frame_route` / `live_lanes_route` 的调用临时改成 `Some(token)` 与 `.0`，让 crate 编得过（Task 4 再改成真正的无令牌逻辑）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 5: 变异检查**

逐条：`authed` 的 `None if room.public.is_some()` 改成 `None`（`tokenless_reads_only_reach_public_rooms` 红）；`publish` 删掉黑名单判断（`publishing_follows_the_answer_table` 红）；`reconcile` 的 `keep` 恒为 `true`（`reconcile_withdraws...` 红）；`start` 里 `public: None` 改成保留旧房间的 `public`（`reopening...` 红——若 `insert` 本就整体替换，改成先取旧房间的 `public` 再塞回去）；`controls` 的 `Grant` 分支恒 `true`（`publishing_follows...` 红）。每次改回。

- [ ] **Step 6: Commit**

```bash
git add crates/dct-srv/src/live.rs crates/dct-srv/src/lib.rs
git commit -m "feat(srv): rooms can be published, read without a token while public, and withdrawn"
```

---


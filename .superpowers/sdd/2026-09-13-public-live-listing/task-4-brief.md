### Task 4: `dct-srv` —— HTTP 路由与密钥重载

**Files:**
- Modify: `crates/dct-srv/src/lib.rs`
- Modify: `crates/dct-srv/src/main.rs`
- Modify: `crates/dct-srv/tests/serves.rs`（`serve` 多一个参数）

**Interfaces:**
- Consumes: Task 3 全部；Task 2 `KeyFile`；Task 5 的 `dct_page::public_page()`（**Task 5 之前先用** `axum::response::Html("<!doctype html><title>公开直播</title>")` 占位，Task 5 替换——占位只在两个 Task 之间存在）。
- Produces:
  - `pub struct AppState { pub relay, pub live, pub keys: Arc<std::sync::RwLock<Option<PublishKeys>>> }`
  - `pub async fn serve(listener, relay, live, routes, keys: Option<KeyFile>)`
  - 路由：`PUT/DELETE /live/{id}/public`、`GET /live/public`、`GET /`（`LiveOnly` 也挂）
  - `GET /live/{id}/lanes` 答复多 `"public": {"title": "..."} | null`

- [ ] **Step 1: 写失败的测试**

`lib.rs` 的 `mod tests` 里，把 `app()` / `app_with_live()` 构造 `AppState` 的地方加 `keys: Arc::new(std::sync::RwLock::new(None))`，并新增一个带密钥的底子与测试：

```rust
    fn app_with_keys() -> (Router, Arc<Live>, String) {
        let live = Arc::new(Live::new());
        let mut k = crate::keys::PublishKeys::default();
        let key = k.add("姜老师", 0).unwrap();
        let app = router(
            AppState {
                relay: Arc::new(Relay::new(cfg(200))),
                live: live.clone(),
                keys: Arc::new(std::sync::RwLock::new(Some(k))),
            },
            Routes::LiveOnly,
        );
        live.start("abc".into(), "t".repeat(64), push_secret(), vec!["前端".into()]).unwrap();
        (app, live, key)
    }

    async fn call(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// 公开 → 列表里出现 → 无令牌能读路名（带公开标题）→ 取消公开 → 无令牌被拒。
    #[tokio::test]
    async fn publishing_over_http_end_to_end() {
        let (app, _live, key) = app_with_keys();
        let push = push_secret();
        let body = format!(r#"{{"title":"第3课","key":"{key}"}}"#);
        let ct = ("content-type", "application/json");

        let (s, _) = call(&app, "PUT", "/live/abc/public", &[ct, ("x-live-push", &push)], &body).await;
        assert_eq!(s, 204);
        let (s, list) = call(&app, "GET", "/live/public", &[], "").await;
        assert_eq!(s, 200);
        assert!(list.contains("第3课") && list.contains("前端") && !list.contains("姜老师"), "{list}");
        let (s, lanes) = call(&app, "GET", "/live/abc/lanes", &[], "").await;
        assert_eq!(s, 200, "公开房间无令牌能读路名");
        assert!(lanes.contains(r#""public":{"title":"第3课"}"#), "{lanes}");

        let (s, _) = call(&app, "DELETE", "/live/abc/public", &[("x-live-push", &push)], "").await;
        assert_eq!(s, 204);
        let (s, _) = call(&app, "GET", "/live/abc/lanes", &[], "").await;
        assert_eq!(s, 401);
    }

    #[tokio::test]
    async fn a_grant_header_can_publish() {
        let (app, _live, key) = app_with_keys();
        let grant = dct_link::live::publish_grant(&dct_link::live::push_hash(&push_secret()), "abc");
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json"), ("x-live-grant", &grant)], &body).await;
        assert_eq!(s, 204);
    }

    /// 服务器没开公开功能：403；公开页照样打得开（空列表）。
    #[tokio::test]
    async fn without_a_key_file_publishing_is_forbidden_and_the_list_is_empty() {
        let (app, live) = app_with_live();
        live.start("abc".into(), "t".repeat(64), push_secret(), vec!["前端".into()]).unwrap();
        let body = format!(r#"{{"title":"课","key":"{}"}}"#, "0".repeat(64));
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json"), ("x-live-push", &push_secret())], &body).await;
        assert_eq!(s, 403);
        assert_eq!(call(&app, "GET", "/live/public", &[], "").await, (200, "[]".to_string()));
        assert_eq!(call(&app, "GET", "/", &[], "").await.0, 200);
    }

    /// 既没带推帧钥匙也没带凭证：401，跟推帧、停播一样。
    #[tokio::test]
    async fn publishing_without_proof_of_control_is_unauthorized() {
        let (app, _live, key) = app_with_keys();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json")], &body).await;
        assert_eq!(s, 401);
    }

    /// 空的 `x-live-token` 当成没带：观看页没有令牌时不该因为发了个空头就被拒。
    #[tokio::test]
    async fn an_empty_token_header_counts_as_no_token() {
        let (app, _live, key) = app_with_keys();
        let body = format!(r#"{{"title":"课","key":"{key}"}}"#);
        let (s, _) = call(&app, "PUT", "/live/abc/public", &[("content-type", "application/json"), ("x-live-push", &push_secret())], &body).await;
        assert_eq!(s, 204);
        assert_eq!(call(&app, "GET", "/live/abc/lanes", &[("x-live-token", "")], "").await.0, 200);
    }
```

`tests/serves.rs` 两处 `dct_srv::serve(...)` 末尾加 `None`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-srv`
Expected: 编译失败（`AppState` 没有 `keys`、路由不存在）。

- [ ] **Step 3: 实现**

在 `lib.rs`：

1. `AppState` 加字段：

```rust
    /// 当前生效的发布密钥。`None` = 没开公开功能（没带 `--publish-keys`）。
    /// 重载任务写，公开路由读。
    pub keys: Arc<std::sync::RwLock<Option<crate::keys::PublishKeys>>>,
```

`serve` 构造 `AppState` 时传入。
2. 读令牌的小工具（放在 `header` 旁边）：

```rust
/// 令牌头：没带、或者带了个空串，都当成没带。
fn token_header(headers: &HeaderMap) -> Option<&str> {
    header(headers, "x-live-token").filter(|t| !t.is_empty())
}

/// 证明控制房间的那个头：推帧钥匙优先，其次凭证。
fn control_header(headers: &HeaderMap) -> Option<crate::live::Control<'_>> {
    header(headers, "x-live-push")
        .map(crate::live::Control::Push)
        .or_else(|| header(headers, "x-live-grant").map(crate::live::Control::Grant))
}
```

3. `live_frame_route`：`let token = token_header(&headers);`（不再 `ok_or(Unauthorized)`），两处 `live.frame(&id, token, lane)`。
4. `live_lanes_route` 与答复：

```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct PublicTitle {
    title: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LiveLanesResponse {
    lanes: Vec<String>,
    viewers: u32,
    public: Option<PublicTitle>,
}

async fn live_lanes_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<LiveLanesResponse>, Rejected> {
    let (lanes, public) = live.lanes(&id, token_header(&headers))?;
    let viewers = live.viewers(&id);
    Ok(Json(LiveLanesResponse { lanes, viewers, public: public.map(|title| PublicTitle { title }) }))
}
```

5. 公开 / 取消公开 / 列表 / 公开页：

```rust
#[derive(serde::Deserialize)]
struct PublishRequest {
    title: String,
    key: String,
}

/// 公开这场直播。跟建房共用按来源的限流（同一本账）。
async fn live_publish_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<PublishRequest>,
) -> Result<StatusCode, Rejected> {
    state.live.note_start(&client_key(&headers), Instant::now())?;
    let control = control_header(&headers).ok_or(LinkError::Unauthorized)?;
    let keys = state.keys.read().expect("keys 锁");
    state.live.publish(&id, control, keys.as_ref(), &req.key, &req.title)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn live_unpublish_route(
    State(live): State<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, Rejected> {
    let control = control_header(&headers).ok_or(LinkError::Unauthorized)?;
    live.unpublish(&id, control)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn live_public_list_route(State(live): State<Arc<Live>>) -> Json<Vec<crate::live::PublicEntry>> {
    Json(live.public_list())
}

async fn public_page_route() -> axum::response::Html<&'static str> {
    axum::response::Html(dct_page::public_page())
}
```

（`Live::note_start` 当前是 `pub fn`，若可见性不同按源码调整；`FromRef<AppState> for Arc<Live>` 已有，`State(state): State<AppState>` 需要 `AppState: Clone`——它已经 `#[derive(Clone)]`。）
6. `router`：在 `app` 的直播那一组（`Routes` 两档都挂）里加：

```rust
        .route("/", get(public_page_route))
        .route(dct_link::live::PATH_PUBLIC_LIST, get(live_public_list_route))
        .route(
            "/live/{id}/public",
            axum::routing::put(live_publish_route)
                .delete(live_unpublish_route)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
```

并把 `Routes::WithLink` 分支里原来的 `.route("/", get(page_route))` 删掉——`/` 从此是公开页；手机端网页在 `--with-link` 下改挂到 `/phone`（在那一行写注释说明原因，并更新 `the_relay_serves_the_very_same_page_the_daemon_does` 测试请求的路径为 `/phone`）。**先确认**：`page.html` 里有没有写死的 `/` 相对路径依赖（`fetch("/api/...")` 这类不受影响；`location.pathname` 若被用来拼路径则需要检查），有就在报告里写明并调整。
7. `serve` 加参数与重载任务：

```rust
pub async fn serve(
    listener: tokio::net::TcpListener,
    relay: Arc<Relay>,
    live: Arc<Live>,
    routes: Routes,
    keys: Option<crate::keys::KeyFile>,
) -> Result<(), std::io::Error> {
    let shared = Arc::new(std::sync::RwLock::new(keys.as_ref().map(|k| k.keys().clone())));
    if let Some(mut file) = keys {
        let shared = shared.clone();
        let live = live.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(dct_link::live::PUBLISH_KEYS_RELOAD);
            loop {
                tick.tick().await;
                match file.reload_if_changed() {
                    Ok(true) => {
                        let fresh = file.keys().clone();
                        live.reconcile(Some(&fresh));
                        *shared.write().expect("keys 锁") = Some(fresh);
                    }
                    Ok(false) => {}
                    // 坏文件：保留上一份，只记日志（见 `KeyFile::reload_if_changed`）。
                    Err(why) => eprintln!("dct-srv：发布密钥文件没重新加载，继续用上一份：{why}"),
                }
            }
        });
    }
    // ……原有 TTL 清扫任务不变……
    axum::serve(listener, router(AppState { relay, live, keys: shared }, routes)).await
}
```

8. `main.rs`：把 Task 2 暂存的 `let _ = publish_keys;` 换成把 `publish_keys` 作为 `serve` 第五个参数传入；启动提示在开了公开功能时加一句「已开启公开直播（密钥文件：…）」。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 5: 变异检查**

`token_header` 去掉 `.filter(...)`（`an_empty_token_header_counts_as_no_token` 红）；`live_publish_route` 里把 `keys.as_ref()` 换成 `None`（`publishing_over_http_end_to_end` 红）；`LiveLanesResponse` 去掉 `public`（同一条红）。改回。

- [ ] **Step 6: Commit**

```bash
git add crates/dct-srv
git commit -m "feat(srv): publish, unpublish and list routes, with key file reloading"
```

---


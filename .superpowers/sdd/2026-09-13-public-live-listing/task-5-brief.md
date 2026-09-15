### Task 5: `dct-page` —— 公开页与观看页无令牌模式

**Files:**
- Create: `crates/dct-page/public.html`
- Modify: `crates/dct-page/live.html`
- Modify: `crates/dct-page/src/lib.rs`
- Modify: `crates/dct-srv/src/lib.rs`（去掉 Task 4 的占位，调用 `dct_page::public_page()`）

**Interfaces:**
- Consumes: Task 4 的 `GET /live/public` 答复形状 `[{id,title,lanes,viewers}]`。
- Produces: `pub fn public_page() -> &'static str`

- [ ] **Step 1: 写失败的测试**

`crates/dct-page/src/lib.rs` 的 `mod tests` 追加：

```rust
    /// 公开页真的打包进来了，而且是给公众看的那一页。
    #[test]
    fn the_public_page_is_here() {
        let p = super::public_page();
        assert!(p.contains("<!doctype html>"));
        assert!(p.contains("/live/public"), "公开页要去拉公开列表");
    }

    /// **标题和路名是别人填的自由文本，只能当纯文本塞进页面。** 两页都不许
    /// 出现 `innerHTML`：一个 `<script>` 写进标题，就是公开页上的存储型 XSS。
    #[test]
    fn neither_page_ever_uses_inner_html() {
        for (name, src) in [("public.html", super::public_page()), ("live.html", super::live_page())] {
            assert!(!src.contains("innerHTML"), "{name} 里出现了 innerHTML");
            assert!(!src.contains("outerHTML"), "{name} 里出现了 outerHTML");
            assert!(!src.contains("insertAdjacentHTML"), "{name} 里出现了 insertAdjacentHTML");
        }
    }

    /// 观看页没有令牌时不能发一个空的 `x-live-token`，而要干脆不带。
    #[test]
    fn the_live_page_omits_the_token_header_when_it_has_none() {
        let src = super::live_page();
        assert!(src.contains("function tokenHeaders()"), "拉帧和拉路名要走同一个取头的函数");
        assert!(!src.contains(r#"headers: { "x-live-token": TOKEN }"#), "还有地方直接写死了令牌头");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p dct-page`
Expected: 编译失败（`public_page` 未定义）。

- [ ] **Step 3: 写 `public.html`**

```html
<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>公开直播</title>
<!--
  公开直播列表。只由中转发（`GET /`）。

  规矩：
  - 标题和路名是老师或管理台填的自由文本：**只用 textContent**，不许 innerHTML
    （`dct-page` 的 `neither_page_ever_uses_inner_html` 钉着）。
  - 不设 cookie、不带 referrer、不引外部资源。
  - 每 5 秒拉一次 `/live/public`；标签页在后台时不拉，回到前台立刻拉一次。
-->
<style>
  :root { color-scheme: light dark; --fg: #1d1d1f; --bg: #fafafa; --card: #fff; --dim: #6e6e73; --accent: #0a7d5a; }
  @media (prefers-color-scheme: dark) { :root { --fg: #e8e8ea; --bg: #111; --card: #1c1c1e; --dim: #98989d; --accent: #3ccf9a; } }
  body { margin: 0; padding: 24px 16px; font: 16px/1.5 -apple-system, "PingFang SC", "Microsoft YaHei", system-ui, sans-serif; color: var(--fg); background: var(--bg); }
  main { max-width: 760px; margin: 0 auto; }
  h1 { font-size: 22px; margin: 0 0 16px; }
  .empty { color: var(--dim); }
  a.card { display: block; padding: 14px 16px; margin: 0 0 12px; border-radius: 12px; background: var(--card); color: inherit; text-decoration: none; box-shadow: 0 1px 2px rgba(0,0,0,.08); }
  .title { font-weight: 600; font-size: 18px; }
  .lanes { margin: 6px 0 0; display: flex; flex-wrap: wrap; gap: 6px; }
  .lane { font-size: 13px; padding: 2px 8px; border-radius: 999px; border: 1px solid var(--dim); color: var(--dim); }
  .viewers { margin-top: 6px; font-size: 13px; color: var(--accent); }
</style>
</head>
<body>
<main>
  <h1 id="heading"></h1>
  <p id="empty" class="empty" hidden></p>
  <div id="list"></div>
</main>
<script>
(function () {
  "use strict";
  var STRINGS = {
    zh: { heading: "正在公开直播", empty: "现在没有公开直播", viewers: function (n) { return n + " 人在看"; } },
    en: { heading: "Live now", empty: "Nothing is live publicly right now", viewers: function (n) { return n + " watching"; } }
  };
  var LANG = (navigator.language || "").toLowerCase().indexOf("zh") === 0 ? "zh" : "en";
  var S = STRINGS[LANG];
  document.documentElement.lang = LANG;
  document.getElementById("heading").textContent = S.heading;
  var emptyEl = document.getElementById("empty");
  var listEl = document.getElementById("list");
  var REFRESH_MS = 5000;

  function render(items) {
    listEl.textContent = "";
    emptyEl.hidden = items.length > 0;
    emptyEl.textContent = S.empty;
    items.forEach(function (it) {
      var a = document.createElement("a");
      a.className = "card";
      a.href = "/live/" + encodeURIComponent(it.id);
      var t = document.createElement("div");
      t.className = "title";
      t.textContent = it.title;
      a.appendChild(t);
      var lanes = document.createElement("div");
      lanes.className = "lanes";
      (it.lanes || []).forEach(function (name) {
        var l = document.createElement("span");
        l.className = "lane";
        l.textContent = name;
        lanes.appendChild(l);
      });
      a.appendChild(lanes);
      var v = document.createElement("div");
      v.className = "viewers";
      v.textContent = S.viewers(it.viewers || 0);
      a.appendChild(v);
      listEl.appendChild(a);
    });
  }

  function pull() {
    if (document.hidden) { return; }
    fetch("/live/public", { credentials: "omit", cache: "no-store" })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (items) { if (items) { render(items); } })
      .catch(function () {});
  }

  document.addEventListener("visibilitychange", function () { if (!document.hidden) { pull(); } });
  setInterval(pull, REFRESH_MS);
  pull();
})();
</script>
</body>
</html>
```

`lib.rs` 加：

```rust
const PUBLIC_SRC: &str = include_str!("../public.html");

/// 公开直播列表页，只由中转发（`GET /`）。不用共享渲染代码，所以不做占位符替换。
pub fn public_page() -> &'static str {
    PUBLIC_SRC
}
```

- [ ] **Step 4: 改 `live.html`**

1. 在 `TOKEN` 的定义之后加：

```js
  // 没有令牌就**不带**这个头：公开的直播不需要令牌，而一个空的 x-live-token
  // 会被当成「带了、但是不对」。拉帧和拉路名都从这里取头，别再各写一份。
  function tokenHeaders() {
    return TOKEN ? { "x-live-token": TOKEN } : {};
  }
```

2. 把文件里所有 `headers: { "x-live-token": TOKEN }`（`fetchLanes` 和拉帧那一处；用 `grep -n "x-live-token" crates/dct-page/live.html` 找全）改成 `headers: tokenHeaders()`。
3. `STRINGS.zh` / `STRINGS.en` 各加：

```js
      endedPublic: "这场直播已结束或不再公开",
      backToList: "回到公开列表",
```

```js
      endedPublic: "This live session has ended or is no longer public",
      backToList: "Back to the public list",
```

4. `render()` 改成：

```js
  function render() {
    dotEl.textContent = ICON[state];
    // 没有令牌的观众是从公开列表点进来的：结束和「切回私密」对他是同一件事
    // （中转回的也是同一个 401），说一句能涵盖两者的话，并给一条回列表的路。
    if (state === "ended" && !TOKEN) {
      statusEl.textContent = S.endedPublic + " · ";
      var back = document.createElement("a");
      back.href = "/";
      back.textContent = S.backToList;
      statusEl.appendChild(back);
    } else {
      statusEl.textContent = S[state];
    }
    headEl.className = state === "ended" ? "bad" : state === "paused" ? "warn" : "";
  }
```

5. 在 `crates/dct-srv/src/lib.rs` 把 `public_page_route` 里 Task 4 的占位换成 `dct_page::public_page()`（若 Task 4 已直接写成调用，这一步确认即可）。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p dct-page && cargo test -p dct-srv`
Expected: PASS。

- [ ] **Step 6: 变异检查**

在 `public.html` 的 `t.textContent = it.title;` 改成 `t.innerHTML = it.title;`，确认 `neither_page_ever_uses_inner_html` 红；在 `live.html` 恢复一处写死的令牌头，确认 `the_live_page_omits_the_token_header_when_it_has_none` 红。改回。

- [ ] **Step 7: 手工验收脚手架**

在 `crates/dct-srv/tests/serves.rs` 追加一条 `#[ignore]` 测试，起中转（带临时密钥文件）、开一场房间、公开、推一帧，打印地址后挂 10 分钟：

```rust
/// 手工看公开页：`cargo test -p dct-srv --test serves serve_a_public_room_for_a_manual_look -- --ignored --nocapture`
/// 然后浏览器打开打印出来的地址。
#[tokio::test]
#[ignore]
async fn serve_a_public_room_for_a_manual_look() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("keys.json");
    let mut k = dct_srv::keys::PublishKeys::default();
    let key = k.add("手工验收", 0).unwrap();
    k.save(&file).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let live = Arc::new(Live::new());
    let push = "p".repeat(64);
    live.start("demo01".into(), "t".repeat(64), push.clone(), vec!["前端".into(), "后端".into()]).unwrap();
    let keys = dct_srv::keys::KeyFile::open(file).unwrap();
    live.publish("demo01", dct_srv::Control::Push(&push), Some(keys.keys()), &key, "手工验收 · <script>alert(1)</script>").unwrap();
    // 房间 60 秒没推帧会被 TTL 回收：每 20 秒推一次空帧保活。
    let keepalive = live.clone();
    let keepalive_secret = push.clone();
    tokio::spawn(async move {
        loop {
            let _ = keepalive.push("demo01", &keepalive_secret, 0, Vec::new());
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    });
    tokio::spawn(dct_srv::serve(listener, Arc::new(Relay::new(Config::default())), live, Routes::LiveOnly, Some(keys)));
    println!("公开页：http://{addr}/   （标题里那段 <script> 必须原样显示成文字）");
    tokio::time::sleep(Duration::from_secs(600)).await;
}
```

`Control` 要从 `dct_srv` 顶层可见：在 `lib.rs` 加 `pub use live::{Control, PublicEntry};`。

- [ ] **Step 8: Commit**

```bash
git add crates/dct-page crates/dct-srv
git commit -m "feat(page): public listing page and tokenless viewing of public rooms"
```

---


### Task 4: 网关签中转令牌的契约 + dct 这边换令牌

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-dct-multi-machine-design.md`：末尾加「附录：网关签中转令牌（冻结的线上契约）」
- Create: `src/mesh/mod.rs`（这一步只放 `pub mod login;`，以后的任务再往里加）
- Create: `src/mesh/login.rs`
- Modify: `src/lib.rs`（加 `pub mod mesh;`）

**契约**（照抄进附录；网关由 dc_llm 那边实现，本任务不改 dc_llm 仓库）：

```
POST /admin/api/relay/token
  Authorization: Bearer <学生的 api_key>        ← 只发给网关，永不发给中转
  → {"endpoint": "c-<20 hex>"}
  ← 200 {"token": "<relay token>", "exp": <unix 秒>}
  ← 401 {"error": "invalid_api_key"}
  ← 404  功能开关 DC_RELAY_TOKENS_ENABLED 关着
  ← 400  endpoint 不合法（不是 c- 加 20 位小写十六进制）
令牌：dct-mesh::relay_token 的格式；account = 网关里这个账号的用户 id（十进制字符串）；
exp = 现在 + 7 天；签名钥匙是网关的一把 P-256，公钥交给中转的 --relay-keys 文件。
```

**Interfaces:**
- Produces:
  - `mesh::login::fetch_token(origin: &str, api_key: &str, endpoint: &str, send: &dyn Fn(&str, &str, &str) -> Result<(u16, String), String>) -> Result<(String, u64), String>`
    - `send` 的三个参数是 url、Bearer、JSON 请求体，返回状态码和响应体；测试里注入假的；
    - 失败时返回给用户看的中文，例如 401 → 「登录已失效，请先在 dct 里重新配对 DC 账号」，404 → 「服务器还没开放多电脑功能」。
  - `mesh::login::needs_renewal(exp: u64, now: u64) -> bool`：剩不到 1 天就返回 true。
  - `mesh::login::RELAY_TOKEN_KEY: &str = "__relay__"`：secrets.toml 里存令牌的键。令牌过期时间存在旁边的 `"__relay_exp__"`。

- [ ] **Step 1: 失败的测试**
  - 200 返回 (token, exp)
  - 401 / 404 / 400 / 网络错各自对应的中文
  - `needs_renewal` 的边界
  - api_key 只出现在 Bearer 头里，不出现在请求体
- [ ] **Step 2–4: 实现，测试通过**
- [ ] **Step 5: Commit** —— `git commit -m "feat(mesh): gateway contract for relay tokens and the dct-side exchange"`

控制端在这个任务之后要做一件事：把附录的契约发给 dc-llm 会话，请它实现。**发之前要用户同意。**

---


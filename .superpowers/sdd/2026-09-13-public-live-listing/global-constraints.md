## Global Constraints

- 协议：`PROTOCOL_VERSION` 18 → 19（Task 6 一次到位）。
- 公开标题：1–60 个**字符**（`chars().count()`），常量 `dct_link::live::MAX_PUBLIC_TITLE_CHARS = 60`。
- 密钥文件重载间隔 10 秒（`dct_link::live::PUBLISH_KEYS_RELOAD`）；管理台自愈间隔 30 秒；公开页刷新 5 秒。
- 保留房间号：`public`（`dct_link::live::RESERVED_LIVE_ID`）。
- `PUT/DELETE /live/{id}/public` 答复码：房间不存在或推帧钥匙/凭证不对 401；没开公开功能 403；发布密钥不对或吊销 401；黑名单 403；标题越界 413；限流 429；成功 204。`DELETE` 幂等。
- 凭证：`hex(HMAC-SHA256(key = SHA-256(push_secret), msg = "publish:" + id))`，只认 `PUT/DELETE /live/{id}/public`。
- 发布密钥、推帧钥匙、凭证**不出现在任何 `Debug` 输出里**；发布密钥**从不出现在守护进程发给界面的任何响应里**。
- 标题和路名在网页里只用 `textContent`，`public.html` / `live.html` 源码里不许出现 `innerHTML`。
- 提交信息用英文，不加任何 AI 署名行（仓库约定）。
- 界面文案两种语言（`t!` 宏），不写死一种。
- 每个 Task 结束前跑 `env -u TERM cargo test --workspace --locked --no-fail-fast` 与 `cargo clippy --workspace --all-targets --locked -- -D warnings`（改了 Node 的 Task 另跑 `node --test`）；新测试逐条做变异检查（把对应实现改坏，确认测试变红，再改回来）。
- 本计划里的参考代码**不是权威**：动手前对照真实源码核对名字、签名和调用点，发现不对以源码为准并在报告里写明。


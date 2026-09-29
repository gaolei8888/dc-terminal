### Task 9: 本机端到端和上线准备（上线本身要用户同意）

**Files:**
- Create: `tests/mesh_e2e.rs`
- Modify: `README.md`：写上多电脑的用法，还有 dct-srv `--relay-keys` 的部署方式

**Step 1: 本机端到端测试**，放在 `tests/mesh_e2e.rs`，属于 `#[ignore]` 的集成测试，手动 `cargo test --test mesh_e2e -- --ignored` 跑：
- 起一个 `dct-srv serve --with-link --relay-keys <tmp>/issuer.pub`，绑在 127.0.0.1 的随机端口；
- 两个独立的 `DCT_HOME` 临时目录，各起一个 daemon，用 `token mint` 签的令牌代替网关；
- 走完 login → join → approve → peers → send；
- 断言：B 的会话里出现 marker 文本；中转进程的日志里没有出现留言原文。

**Step 2: 上线清单**（写进 README 的「部署」一节；**执行它属于上线，由控制端问过用户之后再做，子 agent 不执行**）：
1. 网关实现附录契约，打开 `DC_RELAY_TOKENS_ENABLED`，导出签名公钥；
2. dataclue.cn 上新增 `/dct-relay` 反向代理，TLS 用现有证书，参考记忆「dataclue-classroom-server-ops」；
3. 服务器上跑 `dct-srv serve --with-link --relay-keys /etc/dct-srv/gateway.pub --addr 127.0.0.1:<port>`，由反向代理对外。注意：这里 dct-srv 仍然只监听本机，公网流量由反向代理转进来；
4. 用两台真电脑（Mac 加 Windows）走一遍 login → join → send。

- [ ] **Step 3: Commit** —— `git commit -m "test(mesh): local end-to-end over a real relay; deployment notes"`

---

## 自检记录

- Spec 覆盖：
  - 第 0 段：Task 2 和 3 做加密与不解析，第 4 条由 Global Constraints 和裁决 4 保证；
  - 第 1 段：身份（Task 1、4、5），组（Task 1、6），加入与核对（Task 6），移除（Task 6）；
  - 第 2 段：地址与清单（Task 7），发（Task 7），收、标记、排队、回执（Task 7），离线（裁决 4，本步不做），权限（Global Constraints）；
  - 「看得到什么」的 TUI 部分：Task 8。「进入别的电脑的会话」是第 2 步。
- 相对 spec 的改动：见「相对于 spec 的裁决」1–5。
- 类型一致性：
  - `Member` / `SignedRoster` / `MachineKeys` / `Sealed` / `Payload` 在 Task 1–2 定义，Task 5–7 使用；
  - `Link::with_handler` 在 Task 5 定义，Task 6 和 7 通过 `Net` trait 使用；
  - `MeshView` / `PeerView` / `SendOutcome` 在 Task 6–7 定义，Task 8 使用。

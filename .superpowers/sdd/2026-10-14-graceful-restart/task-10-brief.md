### Task 10: 文档与真机验收

**Files:**
- Modify: `README.md`、`README.zh-CN.md`（`dct restart` 那段）

- [ ] **Step 1: 写文档**：说明 `dct restart` 会先等、`--now`、`--wait`、`--nudge-wait`、Ctrl+C 取消、重启后接回对话、codex/opencode 会写交接。大白话，不出现术语。

- [ ] **Step 2: 全量检查**

```bash
cargo test
cargo +1.99.0 clippy --all-targets -- -D warnings
```

Expected: 全绿、零警告。

- [ ] **Step 3: 真机验收（隔离 socket，不碰用户真实后台）**

```bash
cargo build --release
T=$(mktemp -d); SOCK=$T/daemon.sock; BIN=$PWD/target/release/dct
$BIN daemon $SOCK &   # 前台起一个测试后台
```

用测试 socket（若 CLI 不支持指定 socket，就用 `HOME=$T` 跑——沙箱拦 HOME 的话请主会话来做这一步）在一个临时 git 目录里：
1. 起两个 claude 会话（同目录），各说一句可辨认的话（「暗号 A」「暗号 B」）；起一个 codex 会话；让其中一个 claude 做一件要 1–2 分钟的事。
2. `dct restart -y --wait 1 --nudge-wait 1`，看进度行、兜底话、codex 收到交接请求。
3. 重启后：两个 claude 各问「暗号是什么」，各答自己的；被打断那个收到「继续」；codex 收到「先读交接」且文件存在。
4. 在其中一个 claude 会话里让它自己执行 `dct restart -y --wait 1`，确认不卡、新后台起来、它被叫继续。
5. 起一次排空后按 Ctrl+C，确认会话照常、手机/语音以外的输入正常。

把每一步的结果（截屏文字或命令输出）写进报告。**不要**对用户真实的后台做任何一步。

- [ ] **Step 4: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs(restart): explain graceful restart in the READMEs"
```

### Task 3: 实测 qwen 能不能按 id 接回（实施者在真机上做，不写进自动测试）

**Files:**
- Modify（仅当实测通过）：`profiles/qwen.toml`、`profiles/dc.toml`，以及 Task 1 的 `unverified_profiles_do_not_resume_by_id` 测试名单

- [ ] **Step 1: 在一个临时 git 目录里实测**

```bash
d=$(mktemp -d) && cd "$d" && git init -q
id=$(uuidgen | tr A-Z a-z)
# 第一次：指定 id，说一句可辨认的话，然后退出
qwen --approval-mode=yolo --session-id "$id" -p "记住暗号：蓝色长颈鹿。只回答 好。"
# 第二次：按 id 接回，问暗号
qwen --approval-mode=yolo --resume "$id" -p "暗号是什么？只回答暗号。"
```

Expected（通过）：第二次回答「蓝色长颈鹿」。如果 `-p` 模式不支持 `--resume`，改成交互模式在 dct 测试 socket 里各起一次会话手动验证。`dc` profile 走 dc_llm 网关，需要网关可用；不可用就只给 `qwen` 声明，`dc` 留着。

- [ ] **Step 2: 通过则声明，不通过则记下原因**

通过：两个 toml 加与 Task 1 相同的两行（注释写「`qwen --help` 有 `--session-id`/`--resume`；2026-10-1x 实测按 id 接回成功」），从测试名单里移出 `qwen`（和 `dc`，如果也测过）并加进 `claude_based_profiles_resume_by_id` 那样的正向测试。
不通过：不改 toml，在报告里写清命令和输出。

- [ ] **Step 3: `cargo test --lib profile::` 通过后提交**

```bash
git commit -am "feat(profile): qwen resumes a conversation by id (verified by hand)"
```

---


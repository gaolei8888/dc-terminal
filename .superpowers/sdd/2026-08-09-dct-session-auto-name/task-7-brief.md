### Task 7: 两份 README

**Files:**
- Modify: `README.md`、`README.zh-CN.md`

- [ ] **Step 1: 找到该改的段落**

```bash
grep -n "claude\b" README.md README.zh-CN.md | head -30
```

看板/九宫格那一节里出现 `3 claude` 这类示例的地方，以及「会让你不爽的地方」那一节。

- [ ] **Step 2: 写进去**

两份都要写，内容对齐（英文那份不是中文那份的翻译，但事实必须一致）：

- 会话会自动得到一个名字，在第一次干完活时由模型起一次，**之后不再变**
- **没配 LLM 时这个功能安静下线**，名字退回你说的第一句话，再退回 agent 名 —— 会话照跑
- 名字跟着**你输入的语言**走，不跟界面语言走
- 名字不能手改（这一版没有重命名）

顺带把示例里的 `3 claude` 更新成带名字的样子。

- [ ] **Step 3: 提交**

```bash
git add README.md README.zh-CN.md
git commit -m "docs: sessions carry a name now, and it degrades quietly"
```

---


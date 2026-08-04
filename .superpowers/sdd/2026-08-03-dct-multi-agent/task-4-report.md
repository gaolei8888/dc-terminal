# Task 4: 密钥仓 — 报告

## 实现内容

新增 `src/secrets.rs`，实现 `SecretStore`：按 profile 名索引的用户密钥仓，落盘在 `~/.dct/secrets.toml`（跟着 socket 走，测试自动隔离，同 `projects::store_path_for_socket` / `profile::profiles_dir_for_socket` 的模式）。

公开接口：
- `secrets_path_for_socket(&Path) -> PathBuf`
- `SecretStore::load(&Path) -> SecretStore`
- `get(&self, profile: &str) -> Option<&str>`
- `set(&mut self, profile: &str, value: &str) -> Result<()>`
- `remove(&mut self, profile: &str) -> Result<()>`
- `load_error(&self) -> Option<&str>`

安全/正确性要点：
- 文件创建时即 `mode(0o600)`（`OpenOptions::mode`），不是先建后 chmod。
- 原子写：临时文件（同目录）写完 `sync_all` 后 `rename` 替换正式文件。
- 读坏的文件不当空处理：`load_error` 记下原因，`set`/`remove` 遇到 `load_error` 直接拒绝写入，报错里点名具体文件路径，原文件一字节不动。
- 文件不存在是常态，不是错误。
- `src/lib.rs` 加了 `pub mod secrets;`。

## 自我审查后追加的修复

`save()` 里 write_all / sync_all / rename 任一步失败时（如磁盘满、跨设备 rename），brief 原版代码会把半成品临时文件留在目录里。这不是安全漏洞（临时文件本身也是 0600），但会在目录里留下一个来路不明的文件，且下次 `set` 会用 `truncate` 复用它。加了一层：把这几步包进一个闭包，失败时 `remove_file` 尽力清理（成功路径下 `rename` 已经把 tmp 移走，清理是 no-op）。这个分支没有对应的失败注入测试（磁盘满在测试环境里不好稳定复现），是纯粹的防御性改动，不影响任何既有测试路径。

## 测试与结果

TDD 流程：

**RED**
```
~/.cargo/bin/cargo test --lib secrets
```
先只写了 `mod tests`（brief 里给的 7 个测试），`src/secrets.rs` 里没有任何实现代码。输出（节选）：
```
error[E0425]: cannot find function `secrets_path_for_socket` in this scope
error[E0433]: cannot find type `SecretStore` in this scope
   (共 9 处 E0425/E0433，7 个测试全部因编译不过而失败)
```
符合预期：模块还没实现，编译不过。

**GREEN**
补全实现后：
```
~/.cargo/bin/cargo test --lib secrets
```
```
running 7 tests
test secrets::tests::secrets_path_sits_next_to_socket ... ok
test secrets::tests::missing_file_is_empty_and_not_an_error ... ok
test secrets::tests::corrupt_file_refuses_to_write ... ok
test secrets::tests::file_is_owner_only ... ok
test secrets::tests::set_then_get_survives_reload ... ok
test secrets::tests::no_temp_file_is_left_behind ... ok
test secrets::tests::remove_deletes_the_entry ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 85 filtered out; finished in 0.04s
```

追加临时文件清理修复后重跑，同样 7/7 通过，无警告。

**全量验证**
```
~/.cargo/bin/cargo fmt
git diff --check
~/.cargo/bin/cargo test
~/.cargo/bin/cargo clippy --lib
```
全部通过：单元测试 92 passed（含新增 7 个），全部集成测试套件（concurrency / daemon_detach / daemon_roundtrip / projects_flow / signal_restore / slow_input / socket_perms）绿。`clippy --lib` 无警告。`git diff --check` 无尾随空白问题。

## 改动文件

- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/secrets.rs`（新建）
- `/Users/lei/work/dc/dc-terminal/.claude/worktrees/multi-agent/src/lib.rs`（加一行 `pub mod secrets;`）

## 自我审查发现

- 全部 7 个测试、全部 6 个公开函数都覆盖了。
- 命名、结构跟随 brief 与既有 `projects.rs` / `profile.rs` 的模式一致（`Disk` 内层包一层表、`*_for_socket` 命名、Result 语义对比注释）。
- 唯一主动改动：`save()` 增加失败路径的临时文件清理（见上）。这不是过度设计——只是把 brief 里已经写了大段注释强调的"原子写"原则往前推了一步（半成品不该在磁盘上可见），改动量是一个闭包 + 3 行判断，没有引入新的公开接口或新文件。
- 没有发现遗留的 warning，没有新增依赖（`tempfile`、`toml`、`anyhow`、`serde` 均已在 `Cargo.toml`）。
- 未触碰 `.superpowers/sdd/.gitignore`，也没有 `git add -A`；只 `git add src/secrets.rs src/lib.rs`。

## 问题或顾虑

无。范围完全在 brief 的意图之内，唯一的偏离（临时文件清理）是任务说明里明确要求做的自查项，且改动很小。

---

## 审查后修复：内存改动回滚

### 发现

`set()` 和 `remove()` 会先改 `self.secrets` 再调 `save()`，若 `save()` 失败则内存已改但磁盘未变。审查假设这个 store 是短生命周期的 CLI 进程是错误的——后续任务会把它放进 daemon 的 `Arc<Mutex<SecretStore>>` 里，整个 daemon 生命周期内保持同一实例。在这个场景下：
- `set()` 失败后，内存里有新值但磁盘没有
- `get()` 会虚报密钥已保存
- 用户以为没问题，但重启后密钥就没了

### 修复方案

**关键改动：`set()` 和 `remove()` 都改为快照旧值，`save()` 失败时恢复**

```rust
pub fn set(&mut self, profile: &str, value: &str) -> Result<()> {
    // daemon 会在整个生命周期内保持这个 store 实例。内存改动必须和
    // 磁盘写保持同步，否则 save 失败后，get() 会虚报密钥已保存，
    // 用户以为没问题，但下次重启密钥就没了。
    let old_value = self.secrets.get(profile).cloned();
    self.secrets.insert(profile.to_string(), value.to_string());
    match self.save() {
        Ok(()) => Ok(()),
        Err(e) => {
            // save 失败，回滚内存改动
            if let Some(v) = old_value {
                self.secrets.insert(profile.to_string(), v);
            } else {
                self.secrets.remove(profile);
            }
            Err(e)
        }
    }
}
```

`remove()` 同样做回滚（保存旧值，失败时重新插入）。

### 测试覆盖

新增 2 个测试：

1. **`set_rolls_back_memory_on_save_failure`**：加载坏文件（触发 `load_error`），调 `set()` 失败，验证新键没出现在内存
2. **`remove_rolls_back_memory_on_save_failure`**：加载坏文件（触发 `load_error`），调 `remove()` 失败，验证内存状态不变

两个测试都用现有的 `load_error` 守卫来强制 `save()` 失败，无需磁盘满模拟。

### 测试结果

```
~/.cargo/bin/cargo test --lib secrets
running 9 tests
test secrets::tests::secrets_path_sits_next_to_socket ... ok
test secrets::tests::missing_file_is_empty_and_not_an_error ... ok
test secrets::tests::corrupt_file_refuses_to_write ... ok
test secrets::tests::set_rolls_back_memory_on_save_failure ... ok
test secrets::tests::remove_rolls_back_memory_on_save_failure ... ok
test secrets::tests::set_then_get_survives_reload ... ok
test secrets::tests::file_is_owner_only ... ok
test secrets::tests::no_temp_file_is_left_behind ... ok
test secrets::tests::remove_deletes_the_entry ... ok

test result: ok. 9 passed; 0 failed
```

全量测试：`~/.cargo/bin/cargo test` → 94 passed（原 92 + 新增 2），所有集成测试绿。

### 提交

Commit: `8941ee0`  
Message: `fix: SecretStore set/remove 失败时回滚内存改动`

改动文件：`src/secrets.rs`（74 行新增：方法体 + 测试）

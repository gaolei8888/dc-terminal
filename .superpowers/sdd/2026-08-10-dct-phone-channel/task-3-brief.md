## Task 3: 设置页从「选语言」改成「选设置项」

**这是既有功能的重构，独立成一个任务，不掺任何手机通知逻辑。**

**Files:**
- Modify: `src/ui/view.rs`（`View::Settings`）
- Modify: `src/ui/settings_view.rs`
- Modify: `src/i18n.rs`
- Test: `src/ui/settings_view.rs` 内

**Interfaces:**
- Consumes: 无
- Produces: `SettingsItem { Language, Phone }`、`View::Settings { state: ListState }`（下标改为映射 `SettingsItem::all()`）

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 改结构之前，下标直接映射 Lang::all()。改完之后映射设置项。
    /// **这条是回归测试**：语言仍然切得动比手机通知能用更重要。
    #[test]
    fn the_first_item_is_language() {
        assert_eq!(SettingsItem::all()[0], SettingsItem::Language);
    }

    #[test]
    fn phone_is_a_settings_item_too() {
        assert!(SettingsItem::all().contains(&SettingsItem::Phone));
    }

    /// 下标越界不能 panic——`ListState` 的选中项在列表变短时会留在旧位置。
    #[test]
    fn an_out_of_range_index_selects_nothing() {
        assert_eq!(SettingsItem::at(99), None);
        assert_eq!(SettingsItem::at(0), Some(SettingsItem::Language));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --lib ui::settings_view -- --test-threads=1`
Expected: `cannot find type SettingsItem`

- [ ] **Step 3: 实现**

```rust
/// 设置页的条目。**加进第二项之前这一页是纯语言列表**，`ListState` 的下标
/// 直接映射 `Lang::all()`；现在映射这个枚举，选中语言那一项才进语言列表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsItem {
    Language,
    Phone,
}

impl SettingsItem {
    pub(crate) fn all() -> &'static [SettingsItem] {
        &[SettingsItem::Language, SettingsItem::Phone]
    }

    /// 越界返回 `None` 而不是兜底成第一项：`ListState` 的选中项可能停在
    /// 一个已经不存在的位置，那时候什么都不做，比默默把用户带进语言页好。
    pub(crate) fn at(i: usize) -> Option<SettingsItem> {
        SettingsItem::all().get(i).copied()
    }
}
```

`handle_key` 的 `Enter` 分支改成：按选中项分派 —— `Language` 进语言列表（把今天的语言选择逻辑原样搬进去），`Phone` 进 `View::Phone`（Task 4 建）。**方向键的 `move_sel_n` 长度参数从 `Lang::all().len()` 改成 `SettingsItem::all().len()`。**

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --lib ui:: -- --test-threads=1`
Expected: PASS，且既有的语言相关测试全部仍然通过

- [ ] **Step 5: 手动验证语言仍然切得动**

```bash
cargo run --release
```
进看板 → `l` → 选「界面语言」→ 选英文 → 界面变英文 → 退出重进，**仍然是英文**（`save_lang` 写盘生效）。

- [ ] **Step 6: 变异测试**

把 `at()` 的 `.get(i)` 换成 `all()[i.min(1)]`（越界兜底成最后一项），`an_out_of_range_index_selects_nothing` 必须失败。把 `move_sel_n` 的长度改回 `Lang::all().len()`，方向键会走不到第二项 —— **如果没有测试失败，补一条针对方向键能走到 `Phone` 的测试。**

- [ ] **Step 7: 提交**

```bash
cargo fmt && cargo clippy --all-targets
git add src/ui/view.rs src/ui/settings_view.rs src/i18n.rs
git commit -m "refactor: settings is a list of settings, not a list of languages

The list index mapped straight onto Lang::all(), which works right up until
there is a second thing to configure. Language moves one level down and the
page becomes what its name always claimed.

Nothing about the language behaviour changes; the regression test says so."
```

---


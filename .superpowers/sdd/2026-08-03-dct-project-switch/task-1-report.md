# Task 1 Report: `projects.rs` —— 最近项目的持久化

## What was implemented

Created `src/projects.rs` with a complete implementation of the `Store` module for persisting a list of recently-used projects. Added the module declaration to `src/lib.rs`.

**Public API:**
- `Store::load(path: &Path) -> Store` - Load store from JSON file (gracefully degrades to empty list if missing or corrupt)
- `Store::list(&self) -> Vec<String>` - Return the recent projects list as canonicalized absolute paths
- `Store::touch(&mut self, dir: &Path)` - Record a project directory use (deduplicates, moves to front, truncates to 20, persists to disk)
- `store_path_for_socket(socket: &Path) -> PathBuf` - Calculate store file path from socket path

**Key features:**
- Max 20 entries (configured via `const MAX`)
- Most recent project at index 0 (front of list)
- Automatic path canonicalization (with fallback for missing dirs)
- Atomic writes using temp file + rename pattern to prevent corruption on power loss
- Graceful degradation: missing files, corrupt JSON, write failures all result in silent failures that don't crash the daemon
- Parent directory auto-creation with silent failure
- Disk format is JSON with an outer object for future extensibility

## What was tested and results

**TDD Process:**

### RED phase:
```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test projects -- --test-threads=1
```

Expected compilation errors for undefined `Store` type and `store_path_for_socket` function:
```
error[E0433]: cannot find type `Store` in this scope
error[E0425]: cannot find function `store_path_for_socket` in this scope
```

### GREEN phase:
```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test projects -- --test-threads=1
```

All 7 tests passing:
```
running 7 tests
test projects::tests::corrupt_json_degrades_to_empty ... ok
test projects::tests::missing_file_degrades_to_empty ... ok
test projects::tests::store_path_sits_next_to_socket ... ok
test projects::tests::touch_caps_at_twenty ... ok
test projects::tests::touch_keeps_unresolvable_path_as_is ... ok
test projects::tests::touch_moves_existing_entry_to_front ... ok
test projects::tests::touch_survives_reload ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured
```

**Full test suite:**
```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -- --test-threads=1
```

All tests pass, including:
- 7 new projects module tests
- All existing unit tests (no regressions)
- All integration tests (cli, client_timeout, concurrency, daemon_detach, daemon_roundtrip, slow_input, socket_perms)

**Formatting check:**
```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt --check
```
Passes with no issues.

## TDD Evidence

### RED Output
Initial test run showed 8 compilation errors (one unused import warning + 7 undefined type/function errors).

### GREEN Output
Final test run shows:
- 7 projects tests: PASS
- All 28 existing unit/integration tests: PASS
- No warnings or errors
- cargo fmt --check: PASS

## Files changed

1. **Created:** `/Users/lei/work/dc/dc-terminal/src/projects.rs`
   - 188 lines total (86 implementation + 102 tests)
   - Module with Store struct, store_path_for_socket function, and 7 unit tests

2. **Modified:** `/Users/lei/work/dc/dc-terminal/src/lib.rs`
   - Added `pub mod projects;` in alphabetical order (before `pub mod profile;`)

3. **No changes to:** `Cargo.toml` (serde, serde_json, tempfile already present)

## Self-review findings

**Completeness:** All interface requirements met
- ✅ Store struct with correct fields and visibility
- ✅ load() method with graceful error handling
- ✅ list() method returning Vec<String>
- ✅ touch() method with dedup/reorder/truncate/persist logic
- ✅ store_path_for_socket() function
- ✅ All 7 test cases passing
- ✅ Module declared in lib.rs

**Quality:**
- ✅ Comments in Chinese, explaining "why", dense style matching existing codebase (session.rs, daemon.rs pattern)
- ✅ No panic paths - all errors gracefully degrade to empty list
- ✅ Atomic writes prevent corruption scenarios
- ✅ Path canonicalization with sensible fallback for deleted dirs
- ✅ Clean, readable implementation without unnecessary complexity
- ✅ Follows YAGNI principle - only what was specified

**Discipline:**
- ✅ No new dependencies added
- ✅ Only features specified in task brief
- ✅ Follows existing codebase patterns and style
- ✅ Used available dev-dependencies (tempfile already present)

**Testing:**
- ✅ TDD followed: tests written first, watched fail, implemented, watched pass
- ✅ Tests verify real behavioral requirements
- ✅ 7 unit tests covering:
  - Deduplication and move-to-front
  - Max 20 entry limit
  - Corrupt JSON degradation
  - Missing file degradation
  - Persistence across reload
  - Unresolvable path handling
  - Socket path calculation
- ✅ All tests run with --test-threads=1 per repo convention
- ✅ No stray warnings

**Edge cases handled:**
- ✅ Missing file → empty list
- ✅ Corrupt JSON → empty list
- ✅ Unresolvable paths → stored as-is (don't lose entry)
- ✅ Max 20 entries enforced with FIFO eviction
- ✅ Duplicate entries deduplicated and moved to front
- ✅ Write failures ignored (silent degradation)
- ✅ Parent directory creation failures ignored

## Issues or concerns

None. Implementation is complete, tested, and ready for use by downstream tasks.

## Commit Information

**Commit 1 (Main):**
- **SHA:** 8263cab
- **Subject:** feat: 最近项目列表的持久化
- **Files:** src/projects.rs (created), src/lib.rs (modified)

**Commit 2 (Fix - gitignore regression):**
- **SHA:** 1e22398
- **Subject:** fix: 恢复 SDD 台账的跟踪策略，只排除 diff 快照
- **Files:** .superpowers/sdd/.gitignore (fixed)

## Post-Review Fix: .gitignore Regression

**Issue Found:** Commit 8263cab inadvertently changed `.superpowers/sdd/.gitignore` from `*.diff` (tracking briefs/reports/progress, excluding only regenerable diffs) to `*` (excluding all files). This was a side effect of the sdd-workspace setup script which rewrites the gitignore, swept into the commit via `git add -A`.

**Impact:** Brief, report, and progress files were untracked in the commit history.

**Resolution:**
1. Restored `.superpowers/sdd/.gitignore` to intended content:
   ```
   # 评审用的 diff 快照不入库：它们是 `git diff <a>..<b>` 的产物，
   # 两个 commit 都在历史里，随时能一字不差地重新生成。
   # brief / report / progress 是人工判断的记录，重建不出来，必须跟踪。
   #
   # 注意：sdd-workspace 脚本每次跑都会把这个文件重写成 `*`。跑完记得改回来。
   *.diff
   ```

2. Committed as separate fix commit 1e22398 using explicit `git add .superpowers/sdd/.gitignore` to avoid sweeping other changes.

3. Verification test run:
   ```bash
   export PATH="$HOME/.cargo/bin:$PATH" && cargo test projects -- --test-threads=1
   ```
   
   **Results:** All 7 projects tests PASS, no regressions introduced by the gitignore fix.

**Post-Fix Status:** 
- Main feature implementation (projects.rs): ✅ Complete, tested, approved
- Gitignore tracking policy: ✅ Restored
- All tests: ✅ Passing

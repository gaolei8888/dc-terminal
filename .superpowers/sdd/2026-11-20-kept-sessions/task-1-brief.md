### Task 1: `[[keep]]` config parsing

**Files:**
- Create: `src/keep.rs` (register in `src/lib.rs`)
- Modify: `src/proto.rs` (new warning variant), `src/i18n.rs` (zh/en text), `PROTOCOL_VERSION` 26 → 27 in `src/proto.rs`
- Test: unit tests in `src/keep.rs`; plus a test in `src/config.rs` tests that `Config::load` still reads `[llm]` when `[[keep]]` is malformed

**Interfaces — Produces:**
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keep { pub name: String, pub profile: String, pub dir: PathBuf } // dir has `~` expanded
/// Parse only the `keep` array of a config.toml source. Bad entries are skipped
/// and reported; duplicate names keep the first.
pub fn parse(src: &str, home: &Path) -> (Vec<Keep>, Vec<crate::proto::WarningCode>);
/// Read the file; missing file = empty, unreadable/unparseable file = empty + nothing
/// (Config::load already reports a broken file).
pub fn load(config_path: &Path, home: &Path) -> (Vec<Keep>, Vec<crate::proto::WarningCode>);
```
New `WarningCode` variants (add i18n zh + en, matching existing `SessionResumeSkipped` rendering style):
```rust
/// `[[keep]]` 里第 index 条（从 1 数）写坏了，跳过。reason 是人话里的半句（缺哪个字段 / 名字重复）。
KeepEntryInvalid { index: usize, reason: KeepEntryProblem },
/// 常驻会话出问题。
KeptSessionProblem { name: String, reason: KeptSessionReason },
```
with `enum KeepEntryProblem { MissingField(String), EmptyName, DuplicateName(String) }` (or a simpler shape that renders well) and `enum KeptSessionReason { DirGone, ProfileGone, StartFailed, CrashLooping, NameClash }`. Exact enum shapes are the implementer's call; they must be serde-compatible with the existing `WarningCode` encoding.

- [ ] Step 1: Failing tests in `src/keep.rs`: two good entries; `~` expanded against `home`; one entry missing `profile` → skipped + one `KeepEntryInvalid`, other entry kept; name `"  "` → `EmptyName`; same name twice → first kept, `DuplicateName`; no `keep` key → `([], [])`; `keep` with wrong type (e.g. `keep = 3`) → `([], [one warning])` not a panic; other sections (`[llm]`, `[menu]`) ignored.
- [ ] Step 2: Failing test in config tests: a file with valid `[llm]` + a `[[keep]]` entry missing fields → `Config::load(..).llm.is_some()`.
- [ ] Step 3: Implement (parse via `toml::Value`, walk `keep` array by hand so one bad entry doesn't fail the rest). Add the warning variants + i18n + protocol bump. Fix any exhaustive `match` on `WarningCode` the compiler flags.
- [ ] Step 4: `cargo test keep` and config tests pass; clippy clean.
- [ ] Step 5: Commit `feat(keep): parse [[keep]] entries from config.toml on their own`.


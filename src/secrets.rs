use crate::proto::{coded, ErrorCode, Operation, WarningCode};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 磁盘格式。包一层 `[secrets]` 表而不是把键平铺在顶层，
/// 是为了将来加别的配置段时老文件仍能读。
#[derive(Default, Serialize, Deserialize)]
struct Disk {
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// 按 profile 名索引的用户密钥。落盘在 `~/.dct/secrets.toml`，0600。
pub struct SecretStore {
    path: PathBuf,
    secrets: BTreeMap<String, String>,
    /// 读失败的原因。非 None 时**拒绝任何写入**——见 `set()` 的注释。
    load_error: Option<WarningCode>,
}

/// 手机通知的令牌存在密钥仓里，用一个 profile 不可能占用的名字。
///
/// **它不会出现在密钥页（`c`）里**，因为那一页遍历的是 profiles 再查
/// `has_secret`（见 `ui/view.rs` 的 `secret_rows`），不是遍历这个文件的键。
/// 将来谁把密钥页改成遍历 `secrets.toml`，这个名字就会作为一个不存在的
/// agent 冒出来——改那里的人请回来看这一句。
pub const PHONE_TOKEN_KEY: &str = "__phone__";

/// 配对完成之后的主人 chat id，跟令牌一样存在密钥仓里。**这是 C1 的修复
/// 的一半**：以前"谁是主人"只活在内存里的 `Bridge::owner`，daemon 一
/// 重启就忘光，又从头允许任何人配对——而 bot 用户名是公开可搜的，攻击者
/// 只要趁 dct 关着抢先发一条消息，重启后 Telegram 积压里他的消息排第一
/// 个，就会被判成主人。持久化这个值、重启时读回来交给 `Bridge::new`，
/// 配对就只在真正第一次填令牌之后发生一次，不会随重启反复重开。
pub const PHONE_OWNER_KEY: &str = "__phone_owner__";

/// 跟着 socket 走，测试自动隔离（同 `projects::store_path_for_socket`）。
pub fn secrets_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("secrets.toml"),
        None => PathBuf::from("secrets.toml"),
    }
}

impl SecretStore {
    pub fn load(path: &Path) -> SecretStore {
        let (secrets, load_error) = match std::fs::read_to_string(path) {
            // 文件还没建过是常态，不是错误
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (BTreeMap::new(), None),
            Err(e) => {
                // 系统原话（比如「Permission denied (os error 13)」）只写
                // stderr 留痕迹，不能冒泡到界面上——见 describe_io_error
                // 的注释，非程序员看不懂 errno。「密钥文件读不了：」这半句
                // 前缀在这里加，不留给调用方——`load_error()` 对外必须永远
                // 是一句自足、说清楚是在说哪个文件出了什么问题的中文，
                // daemon.rs 组装 warning 时才能只管拼路径，不用关心
                // 这条错误具体是哪一类（见 daemon.rs 里的注释）。
                eprintln!("密钥文件读取失败（{}）：{e}", path.display());
                (
                    BTreeMap::new(),
                    Some(WarningCode::SecretsUnreadable {
                        path: path.display().to_string(),
                        reason: crate::profile::io_reason(&e),
                    }),
                )
            }
            Ok(src) => match toml::from_str::<Disk>(&src) {
                Ok(d) => (d.secrets, None),
                // IMPORTANT 4（最终整分支 code review）：以前这里复用
                // `profile.rs` 的 `describe_toml_error`（「第 N 行：原因」），
                // 跟处理 profile 文件用的是同一套逻辑。那对 profile 文件是
                // 对的——用户确实在手编一份配置，行号和「原因」半句（哪怕
                // 是 toml 库的原始英文，比如 `invalid key`/`expected ...`）
                // 都能帮他改对。但密钥文件不是 profile 文件：README 明确写
                // 着「密钥只该在这里改，不需要也不支持手动去改
                // secrets.toml」，而 `save()` 的 `load_error` 守卫会拒绝
                // 任何写入（保护还能手工救回的文件），也就是说 `c` 进来的
                // 改/删两条路径全都是死的——用户能做的唯一有效动作是删掉
                // 这个文件、回 dct 里重新粘贴一遍密钥，不是照着行号去抠
                // 一份他被告知不该碰的 TOML 语法。继续把 toml 库的英文
                // 「原因」糊给他，等于把他往一条错误的路上支。
                //
                // 原始错误（行号、toml 库原文）只留一份在 stderr 方便排查，
                // 界面上的话完全不提这些细节，直接给一句做得到的下一步。
                Err(e) => {
                    eprintln!("密钥文件解析失败（{}）：{e}", path.display());
                    (
                        BTreeMap::new(),
                        Some(WarningCode::SecretsCorrupt {
                            path: path.display().to_string(),
                        }),
                    )
                }
            },
        };
        SecretStore {
            path: path.to_path_buf(),
            secrets,
            load_error,
        }
    }

    pub fn load_error(&self) -> Option<&WarningCode> {
        self.load_error.as_ref()
    }

    /// 密钥文件路径。daemon 组装警告文案时要点名是哪个文件，让用户去看。
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, profile: &str) -> Option<&str> {
        self.secrets.get(profile).map(String::as_str)
    }

    pub fn set(&mut self, profile: &str, value: &str) -> Result<()> {
        // daemon 会在整个生命周期内保持这个 store 实例。内存改动必须和
        // 磁盘写保持同步，否则 save 失败后，get() 会虚报密钥已保存，
        // 用户以为没问题，但下次重启密钥就没了。这里先记住改动前的值，
        // 若 save 失败就恢复，保证内存状态始终只反映已落盘的数据。
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

    pub fn remove(&mut self, profile: &str) -> Result<()> {
        // daemon 会在整个生命周期内保持这个 store 实例。内存改动必须和
        // 磁盘写保持同步，否则 save 失败后，密钥看起来被删了但其实没有，
        // 用户以为没问题，但下次重启密钥又回来了。这里先记住改动前是否
        // 存在该键，若 save 失败就恢复。
        let old_value = self.secrets.get(profile).cloned();
        self.secrets.remove(profile);
        match self.save() {
            Ok(()) => Ok(()),
            Err(e) => {
                // save 失败，回滚内存改动
                if let Some(v) = old_value {
                    self.secrets.insert(profile.to_string(), v);
                }
                Err(e)
            }
        }
    }

    /// 和 `projects::Store::save` 不同，这里**落盘失败要报错**：那边丢的是
    /// 「最近项目」这种便利性缓存，这边丢的是用户刚手打的密钥——静默失败
    /// 意味着他下次回来发现还得再填一遍，且不知道为什么。
    fn save(&self) -> Result<()> {
        // 读坏了就不写。当空覆盖的话，用户手改坏的文件（也许只是少个引号，
        // 完全能救回来）会被我们内存里那份残缺数据彻底盖掉。
        // 这条**是用户可见的**：它经守护进程冒泡到界面，用户按 c 改密钥
        // 时会看到。所以同样报码不报句子。
        if self.load_error.is_some() {
            return Err(coded(ErrorCode::SecretsFileBroken {
                path: self.path.display().to_string(),
            }));
        }

        let parent = self
            .path
            .parent()
            .ok_or_else(|| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;
        std::fs::create_dir_all(parent)
            .map_err(|_| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;

        let text = toml::to_string(&Disk {
            secrets: self.secrets.clone(),
        })
        .map_err(|_| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;

        // 原子写：先写同目录的临时文件再 rename。直接覆写的话写到一半断电
        // 会留下半截 TOML，下次 load 就走进「读坏了」分支。
        //
        // 临时文件从**创建那一刻**就是 0600，不是先建再 chmod ——
        // 那中间有一个别的账号能读到密钥的窗口。
        let tmp = self.path.with_extension("toml.tmp");
        let result = (|| -> Result<()> {
            let mut f = crate::sys::fs::create_private(&tmp)
                .map_err(|_| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;
            f.write_all(text.as_bytes())
                .map_err(|_| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;
            f.sync_all()
                .map_err(|_| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;
            // 关掉再改名。Unix 上开着句柄改名是家常便饭，Windows 上要靠
            // 打开时给了 FILE_SHARE_DELETE 才允许（见 `sys::fs::create_private`）。
            // 与其依赖那个共享位，不如先关——反正内容已经 sync 下去了。
            drop(f);
            std::fs::rename(&tmp, &self.path)
                .map_err(|_| coded(ErrorCode::OperationFailed(Operation::SaveSecret)))?;
            Ok(())
        })();

        // 写到一半失败（比如磁盘满了）不能把半成品 tmp 文件留在目录里——
        // 下次 `set` 会拿 truncate 复用它，看着没事，但目录里多一个来路
        // 不明的文件本身就是留给用户的一个疑惑。rename 成功时 tmp 已经不
        // 存在了，这里删不到东西，remove_file 的错误无需理会。
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// `PHONE_TOKEN_KEY` 只是这个仓库里的另一个键——手机令牌用同一套
    /// `set`/`get`/`remove`，没有单独的存储路径。这条钉住它确实是一个
    /// 合法的、不会跟真实 profile 撞名的键。
    #[test]
    fn phone_token_key_round_trips_like_any_other_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set(PHONE_TOKEN_KEY, "123456:AAH-token").unwrap();
        assert_eq!(s.get(PHONE_TOKEN_KEY), Some("123456:AAH-token"));
        // 没有哪个真实 profile 会叫这个名字——双下划线包裹的写法本来就
        // 不是任何 CLI 工具会用的 profile 名。
        assert!(PHONE_TOKEN_KEY.starts_with("__") && PHONE_TOKEN_KEY.ends_with("__"));
    }

    #[test]
    fn secrets_path_sits_next_to_socket() {
        let p = secrets_path_for_socket(Path::new("/home/x/.dct/daemon.sock"));
        assert_eq!(p, PathBuf::from("/home/x/.dct/secrets.toml"));
    }

    #[test]
    fn set_then_get_survives_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");

        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();
        drop(s);

        let s2 = SecretStore::load(&f);
        assert_eq!(s2.get("kimi"), Some("sk-abc"));
    }

    /// 只在 Unix 上验位。Windows 的对应保证是一条只有当前用户的 ACL，
    /// 验它要另一套调用（见 `sys::acl`），不在这条测试里凑。
    #[test]
    #[cfg(unix)]
    fn file_is_owner_only() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();

        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "密钥文件只能属主可读写");
    }

    #[test]
    fn no_temp_file_is_left_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "secrets.toml")
            .collect();
        assert!(
            leftovers.is_empty(),
            "原子写的临时文件要收干净：{leftovers:?}"
        );
    }

    #[test]
    fn remove_deletes_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let mut s = SecretStore::load(&f);
        s.set("kimi", "sk-abc").unwrap();
        s.set("glm", "sk-def").unwrap();
        s.remove("kimi").unwrap();

        assert_eq!(s.get("kimi"), None);
        assert_eq!(s.get("glm"), Some("sk-def"), "只删指定的那条");
    }

    #[test]
    fn missing_file_is_empty_and_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let s = SecretStore::load(&tmp.path().join("还没建过.toml"));
        assert_eq!(s.get("kimi"), None);
        assert!(s.load_error().is_none(), "文件还没建过是常态");
    }

    #[test]
    fn corrupt_file_refuses_to_write() {
        // 关键行为：读坏了**不能**当空。当空的话用户以为密钥丢了，
        // 接着一次写入就把本来还能手工救回的文件彻底覆盖。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let mut s = SecretStore::load(&f);
        assert!(s.load_error().is_some(), "要记住读失败了");

        // 报的是码；人话由界面组。组出来那句仍要说人话、并给出下一步。
        let err = s.set("kimi", "sk-abc").unwrap_err();
        let code = err
            .downcast::<crate::proto::CodedError>()
            .expect("拒绝写入要带错误码")
            .0;
        assert!(matches!(code, ErrorCode::SecretsFileBroken { .. }));
        let line = crate::i18n::msg::error(crate::i18n::Lang::Zh, &code);
        assert!(line.contains("密钥文件"), "拒绝写入时要说人话：{line}");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "这不是 TOML {{{",
            "原文件必须一个字节都没动"
        );
    }

    #[test]
    fn corrupt_file_load_error_is_plain_chinese_not_a_toml_stack_dump() {
        // 之前的实现对 toml::de::Error 直接 format!("{e}")：那是给等宽
        // 终端排版看的多行 ASCII 图（"TOML parse error at line 1, column
        // 1\n  |\n1 | ...\n  |  ^\n..."），糊在选择器标题上就是一份变相
        // 栈追踪。
        //
        // IMPORTANT 4（最终整分支 code review）：这条测试原来只检查了
        // *格式*（单行、没有 toml 库的图形化 Display、带中文「第 N 行」），
        // 从没检查过*内容*本身是不是真的说人话——中间那版实现把
        // `describe_toml_error` 的「原因」半句原样接了过来，那半句是 toml
        // 库的原始英文（`invalid key`/`expected \`"\`, \`'\`` 这种），
        // 格式检查全部通过，糊出来的整句照样是「第 2 行：invalid
        // string；expected..」，用户一个字都读不懂。密钥文件跟 profile
        // 文件不一样：`save()` 会拒绝任何写入（见 `corrupt_file_refuses_to_write`），
        // 而 README 又明说不支持手改这个文件，所以行号 + 英文语法原因对
        // 这个场景没有任何可操作性——这里直接要求整句话不含 toml 库会吐出
        // 来的任何英文技术词，并且要给一个真正做得到的下一步（删掉重填）。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let s = SecretStore::load(&f);
        let code = s.load_error().expect("要记住读失败了");
        // 结构上就没有地方能塞进 toml 的原文——这一类码只带路径。
        assert!(matches!(code, WarningCode::SecretsCorrupt { .. }));
        let err = crate::i18n::msg::warning(crate::i18n::Lang::Zh, code);
        assert!(!err.contains('\n'), "不能是多行栈追踪：{err}");
        assert!(
            !err.contains("TOML parse error"),
            "toml 库自带的图形化 Display 不能漏出来：{err}"
        );
        for jargon in ["invalid", "expected", "line", "column"] {
            assert!(
                !err.to_lowercase().contains(jargon),
                "不能夹带 toml 库的原始英文原因，那半句不该出现在这个文件的错误里：{err}"
            );
        }
        assert!(
            err.contains("删") && err.contains("重新"),
            "要给一个真正做得到的下一步——删掉文件、重新填一遍密钥：{err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_file_load_error_has_no_raw_os_error_text() {
        // 权限错误的 io::Error Display 是英文系统原话，比如
        // "Permission denied (os error 13)"——零编程经验的用户看不懂
        // "os error 13" 是什么。load_error() 只能给中文，原文只写 stderr。
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "[secrets]\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();

        struct RestorePerms<'a>(&'a std::path::Path);
        impl Drop for RestorePerms<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o600));
            }
        }
        let _restore = RestorePerms(&f);

        // root（常见于容器化 CI）不受文件权限位约束，这条测试验证的分支
        // 触发不了——老实跳过，好过硬跑出一个和权限无关的 flaky 失败。
        if std::fs::read_to_string(&f).is_err() {
            let s = SecretStore::load(&f);
            let code = s.load_error().expect("要记住读失败了");
            assert!(matches!(
                code,
                WarningCode::SecretsUnreadable {
                    reason: crate::proto::IoReason::PermissionDenied,
                    ..
                }
            ));
            let err = crate::i18n::msg::warning(crate::i18n::Lang::Zh, code);
            assert!(!err.contains("os error"), "不能漏出 errno：{err}");
            assert!(
                !err.contains("Permission denied"),
                "不能漏出英文原话：{err}"
            );
            assert!(err.contains("权限"), "要点名是权限问题：{err}");
        }
    }

    #[test]
    fn path_exposes_the_underlying_file() {
        // daemon 组装警告文案时要点名是哪个文件，用户才知道去哪修。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        let s = SecretStore::load(&f);
        assert_eq!(s.path(), f);
    }

    #[test]
    fn set_rolls_back_memory_on_save_failure() {
        // set 失败时必须回滚内存改动。这在 daemon 场景中很关键：
        // 如果 set 后 save 失败，内存里有新值但磁盘没有，get() 会虚报
        // 密钥已保存，用户以为没问题，但重启后发现密钥没了。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let mut s = SecretStore::load(&f);
        // 预期：文件坏了，加载时 load_error 被设置，secrets 是空的
        assert_eq!(s.get("key"), None, "初始状态无此键");

        // set 会因为 load_error 而失败
        let err = s.set("key", "value").unwrap_err();
        let code = err
            .downcast::<crate::proto::CodedError>()
            .expect("拒绝写入要带错误码")
            .0;
        let err_msg = crate::i18n::msg::error(crate::i18n::Lang::Zh, &code);
        assert!(err_msg.contains("没有改它"));
        // 错误要告诉用户删掉文件重新填，不能建议手动修复
        assert!(
            err_msg.contains("删"),
            "密钥文件坏了时，错误消息要指向删掉文件这个解决方案：{err_msg}"
        );
        assert!(
            !err_msg.contains("先修") && !err_msg.contains("修好"),
            "不能建议用户手动修复密钥文件：{err_msg}"
        );

        // 关键检验：失败后内存应该恢复到改动前的状态
        assert_eq!(s.get("key"), None, "set 失败后，新键不能出现在内存");
    }

    #[test]
    fn remove_rolls_back_memory_on_save_failure() {
        // remove 失败时也必须回滚内存改动。这在 daemon 场景中很关键：
        // 如果 remove 后 save 失败，内存里没有该键但磁盘还有，get() 会虚报
        // 密钥已删除，但重启后发现密钥还在，造成混淆。
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("secrets.toml");
        std::fs::write(&f, "这不是 TOML {{{").unwrap();

        let mut s = SecretStore::load(&f);
        // 预期：文件坏了，加载时 load_error 被设置，secrets 是空的
        assert_eq!(s.get("key"), None, "初始状态无此键");

        // remove 会因为 load_error 而失败
        let err = s.remove("key").unwrap_err();
        let code = err
            .downcast::<crate::proto::CodedError>()
            .expect("拒绝写入要带错误码")
            .0;
        let err_msg = crate::i18n::msg::error(crate::i18n::Lang::Zh, &code);
        assert!(err_msg.contains("没有改它"));
        // 错误要告诉用户删掉文件重新填，不能建议手动修复
        assert!(
            err_msg.contains("删"),
            "密钥文件坏了时，错误消息要指向删掉文件这个解决方案：{err_msg}"
        );
        assert!(
            !err_msg.contains("先修") && !err_msg.contains("修好"),
            "不能建议用户手动修复密钥文件：{err_msg}"
        );

        // 关键检验：失败后内存应该恢复到改动前的状态（依旧无此键）
        assert_eq!(s.get("key"), None, "remove 失败后，状态应该不变");
    }
}

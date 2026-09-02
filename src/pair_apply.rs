//! 配对成功之后落盘的那几件事：把 `pair::Approved` 写进 `secrets`/
//! `pair-models.toml`，并按学生在配对屏上的勾选决定要不要顺手把 `[llm]`
//! 也接上。
//!
//! **不假装它是原子的。** `secrets.rs` 那套「save 失败就回滚内存」是按
//! 单键写的，两次 set 就是两次落盘。第二次失败**不回滚第一次**——回滚会把
//! 学生刚拿到的、网关那边已经标成 claimed 的钥匙扔掉，那把钥匙他再也领不
//! 回来了。领取是一次性的，这是这里唯一重要的约束。
//!
//! **写盘顺序按最坏情况排。** Task 3 的裁决留了一个口子：`apply` 跑到一半，
//! 取消可能已经到了，这扇窗口没关上。所以顺序是钥匙 → 模型名 → `[llm]`：
//! 一把只有钥匙没有模型名的账号还能用（学生自己在配置里填模型名），一个
//! 只有模型名没有钥匙的 `[llm]` 不能用——先写最坏情况下也有用的那部分。
//!
//! **模型名不写进 `~/.dct/profiles/`。** 这看起来更自然，但两个理由都
//! 站不住：`Profile::command` 没有 `#[serde(default)]`（`profile.rs`），
//! 一份只有 `name`/`[env]` 的文件解析不过去，`load_dir` 会把它当成一个
//! 「坏掉的自定义 profile」报警——报的还是 dct 自己写的文件。就算写成
//! 完整的一份，`all_profiles` 按整条记录替换合并（`profile.rs` 的
//! `*slot = p`），用户目录里的 `dc.toml` 不是给内置那份「加上」`[env]`，
//! 是把它整个换掉，`command`/`[api]`/`[secret]` 全部消失，而且冻住了
//! 今天这份内置 profile——以后 dct 升级修 `dc.toml` 也到不了已经配过对
//! 的学生手上。所以模型名单独存一个小文件（`pair-models.toml`），
//! `session.rs::create_inner` 在算子进程 env 的时候把它按 profile 名合
//! 进去，profile 文件本身一个字节不动。

use crate::pair::Approved;
use std::collections::BTreeMap;
use std::path::Path;

/// 这次配对给这个账号开了哪几条路。成功屏据此换话说。
pub struct Ready {
    pub anthropic: bool,
    pub openai: bool,
}

/// 侧写文件的第一行。认这行才敢重写——没有它的文件是用户自己写的。
const MARK: &str = "# 由 dct 配对写入：这个账号在网关上能用的模型名。下次配对会重写。手改请删掉这一行。";

/// `home` 是 dct 的家目录（`socket.parent()`），不是 profiles 目录——这一步
/// 既要往 `home/secrets.toml` 写钥匙，也要往 `home/pair-models.toml` 写
/// 模型名，两者是同一个锚下的两个子路径。
pub fn apply(
    a: &Approved,
    home: &Path,
    secrets: &std::sync::Mutex<crate::secrets::SecretStore>,
    opt_in_llm: bool,
) -> Result<Ready, String> {
    apply_inner(a, home, secrets, opt_in_llm, crate::llm_optin::enable)
}

/// 真正的实现，`llm_optin::enable` 换成一个可以在测试里录调用的钩子。
/// 生产路径（`apply` 上面那个）永远传真的 `llm_optin::enable`；测试传一个
/// 记录参数的闭包，这样「`opt_in_llm` 到底有没有接上」这件事能被单测钉住，
/// 而不是靠读代码相信它接上了。
fn apply_inner(
    a: &Approved,
    home: &Path,
    secrets: &std::sync::Mutex<crate::secrets::SecretStore>,
    opt_in_llm: bool,
    llm_enable: impl FnOnce(&Path, &str, &str) -> Result<bool, String>,
) -> Result<Ready, String> {
    // 第一步：钥匙。两次 set 不是一个事务——第二把失败不回滚第一把，见
    // 文件头那段。网关已经把这把钥匙标成 claimed 了，回滚等于把它扔掉。
    {
        let mut s = secrets.lock().unwrap_or_else(|e| e.into_inner());
        s.set("dc", &a.api_key)
            .map_err(|e| format!("dc_secret_write_failed: {e}"))?;
        s.set("qwen", &a.api_key)
            .map_err(|e| format!("qwen_secret_write_failed: {e}"))?;
    }

    // 第二步：模型名，不是钥匙。免费账号的 anthropic 那组是空的，这里绝不
    // 编一个模型名进去：一个解析不出模型的 `ANTHROPIC_MODEL` 比没有这行
    // 更坏，学生会撞上一个没有任何解释的 404。空的那一组不落一个 section，
    // 而不是落一个空 section——`[dc]` 段存在与否，直接就是「这个账号有没有
    // Anthropic 那一路」这件事本身，不用去看里面有没有键。
    let mut dc_env = BTreeMap::new();
    if let Some(m) = &a.models.anthropic.default {
        dc_env.insert("ANTHROPIC_MODEL".to_string(), m.clone());
    }
    if let Some(m) = &a.models.anthropic.small_fast {
        dc_env.insert("ANTHROPIC_SMALL_FAST_MODEL".to_string(), m.clone());
    }
    let mut qwen_env = BTreeMap::new();
    if let Some(m) = &a.models.openai.default {
        qwen_env.insert("OPENAI_MODEL".to_string(), m.clone());
    }
    if let Some(m) = &a.models.openai.small_fast {
        qwen_env.insert("OPENAI_SMALL_FAST_MODEL".to_string(), m.clone());
    }
    let mut sections = BTreeMap::new();
    if !dc_env.is_empty() {
        sections.insert("dc".to_string(), dc_env);
    }
    if !qwen_env.is_empty() {
        sections.insert("qwen".to_string(), qwen_env);
    }
    write_pair_models(home, &sections)?;

    // 第三步：`[llm]` 自举，只有学生在配对屏上勾了才做。写在最后——它是
    // 三件事里唯一「没有前面两步就没意义」的一件：`[llm]` 指着一个模型名，
    // 但那个模型名要靠的钥匙如果还没落盘，`[llm]` 写了也用不了。
    if opt_in_llm {
        let (provider, model) = if let Some(m) = &a.models.anthropic.default {
            ("dc", m.clone())
        } else if let Some(m) = &a.models.openai.default {
            ("qwen", m.clone())
        } else {
            // 两边都没有模型名：写一个没有模型的 `[llm]` 段等于写了个解析
            // 不出来的配置，什么都不写好过写这个。
            return Ok(Ready {
                anthropic: a.models.anthropic.default.is_some(),
                openai: a.models.openai.default.is_some(),
            });
        };
        let config_path = home.join("config.toml");
        llm_enable(&config_path, provider, &model)?;
    }

    Ok(Ready {
        anthropic: a.models.anthropic.default.is_some(),
        openai: a.models.openai.default.is_some(),
    })
}

/// 渲染 `pair-models.toml`：每个有模型名的 profile 一个 section。空 map
/// 的调用方（`write_pair_models` 在真没有任何东西可写时）不会走到这里。
fn render_pair_models(sections: &BTreeMap<String, BTreeMap<String, String>>) -> String {
    let mut out = format!("{MARK}\n");
    for (name, env) in sections {
        out.push_str(&format!("\n[{name}]\n"));
        for (k, v) in env {
            out.push_str(&format!("{k} = \"{v}\"\n"));
        }
    }
    out
}

fn write_pair_models(
    home: &Path,
    sections: &BTreeMap<String, BTreeMap<String, String>>,
) -> Result<(), String> {
    let f = home.join("pair-models.toml");
    if let Ok(existing) = std::fs::read_to_string(&f) {
        // 用户手改过的文件不许动。他写在里面的东西比我们知道的多。
        if !existing.starts_with(MARK) {
            return Ok(());
        }
    }
    std::fs::create_dir_all(home).map_err(|e| format!("{e}"))?;
    std::fs::write(&f, render_pair_models(sections)).map_err(|e| format!("{e}"))
}

/// 配对给某个 profile 存下的模型名，供 `session.rs::create_inner` 在起
/// 子进程前合进 env。**缺文件、文件损坏、这个 profile 没配过对**——
/// 三种情况一律退化成空 map，绝不能因为一个坏掉的侧写文件让会话起不来：
/// 有没有模型名从来不是能不能开会话的前提，密钥缺失在 `create_inner`
/// 里都不拦（见那边的注释），这里比密钥还次要。
pub fn env_for(home: &Path, profile: &str) -> BTreeMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(home.join("pair-models.toml")) else {
        return BTreeMap::new();
    };
    let Ok(doc) = raw.parse::<toml::Table>() else {
        return BTreeMap::new();
    };
    let Some(section) = doc.get(profile).and_then(|v| v.as_table()) else {
        return BTreeMap::new();
    };
    section
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// 一次配对填两个 profile。第 6 步「同一把钥匙再填一遍」是这整件事
    /// 想消灭的东西之一。
    #[test]
    fn one_pairing_fills_both_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        let a = approved_with_both_wires();
        apply(&a, dir.path(), &store, false).unwrap();
        let s = store.lock().unwrap();
        assert_eq!(s.get("dc"), Some("sk-live"));
        assert_eq!(s.get("qwen"), Some("sk-live"));
    }

    /// 两个 Anthropic 变量都要写进 `[dc]` 段。只钉主模型的话，起标题、扫
    /// 文件那个便宜的快模型会以课堂上没人查得出来的方式坏掉。
    #[test]
    fn both_anthropic_model_variables_get_written() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        apply(&approved_with_both_wires(), dir.path(), &store, false).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("pair-models.toml")).unwrap();
        assert!(raw.contains("ANTHROPIC_MODEL = \"claude-x\""), "{raw}");
        assert!(
            raw.contains("ANTHROPIC_SMALL_FAST_MODEL = \"claude-small\""),
            "{raw}"
        );
    }

    /// 免费账号：anthropic 那一组是空的。钥匙照写，**模型名一个都不许编**——
    /// 写一个跑不通的模型名比不写更坏，学生会撞上一个没有任何解释的 404。
    /// 文件里只该有 `[qwen]`，`[dc]`/`ANTHROPIC_MODEL` 一处都不该出现。
    #[test]
    fn a_free_account_gets_only_the_qwen_section_and_no_invented_model_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        let ready = apply(&approved_openai_only(), dir.path(), &store, false).unwrap();
        assert!(!ready.anthropic, "免费账号没有 Anthropic 那一路");
        assert!(ready.openai);
        assert_eq!(store.lock().unwrap().get("dc"), Some("sk-live"));
        let raw = std::fs::read_to_string(dir.path().join("pair-models.toml")).unwrap();
        assert!(!raw.contains("ANTHROPIC_MODEL"), "没有就不许写：{raw}");
        assert!(!raw.contains("[dc]"), "没有模型名就不该有这个 section：{raw}");
        assert!(raw.contains("[qwen]"), "{raw}");
    }

    /// 侧写文件要带一行标记：下次配对认它才敢重写。用户手改过的文件
    /// （没有这行）绝不覆盖。
    #[test]
    fn a_hand_edited_pair_models_file_is_never_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("pair-models.toml");
        std::fs::write(&f, "# 我自己写的\n[dc]\nANTHROPIC_MODEL = \"mine\"\n").unwrap();
        let store = Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        apply(&approved_with_both_wires(), dir.path(), &store, false).unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("mine"), "用户手写的东西不许动：{after}");
    }

    /// `env_for`：`[dc]` 段里的键给 `"dc"`，不泄漏给 `"qwen"`；文件不存在
    /// 退化成空 map，不是错误——见函数上面那段注释，坏文件不该拦会话起来。
    #[test]
    fn env_for_reads_its_own_section_and_degrades_to_empty_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pair-models.toml"),
            format!("{MARK}\n\n[dc]\nANTHROPIC_MODEL = \"claude-x\"\n"),
        )
        .unwrap();
        let dc = env_for(dir.path(), "dc");
        assert_eq!(dc.get("ANTHROPIC_MODEL").map(String::as_str), Some("claude-x"));
        assert!(env_for(dir.path(), "qwen").is_empty());
        assert!(env_for(dir.path(), "does-not-exist").is_empty());

        let missing = tempfile::tempdir().unwrap();
        assert!(env_for(missing.path(), "dc").is_empty());
    }

    /// `opt_in_llm` 为真、免费账号：没有 Anthropic 模型名，落到 openai 那组，
    /// provider 必须是 "qwen" 而不是 "dc"——这是网关给出的真实免费账号载荷
    /// （`models.anthropic` 是 `{}`），不是边缘情况。
    #[test]
    fn opt_in_picks_qwen_provider_for_a_free_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        let calls: std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>> =
            Default::default();
        let calls2 = calls.clone();
        let hook = move |_path: &Path, provider: &str, model: &str| {
            calls2
                .borrow_mut()
                .push((provider.to_string(), model.to_string()));
            Ok(true)
        };
        apply_inner(&approved_openai_only(), dir.path(), &store, true, hook).unwrap();
        let recorded = calls.borrow();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0].0, "qwen");
        assert_eq!(recorded[0].1, "qwen3.8:27b");
    }

    /// `opt_in_llm` 为假：不管学生的账号有多少模型可用，一次都不许调
    /// `llm_optin::enable`。这个参数存在就是为了防「勾了但什么都没发生」，
    /// 所以「没勾就真的什么都没做」也要有测试钉住，不能只靠读代码相信。
    #[test]
    fn opt_in_false_never_calls_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let store = Mutex::new(crate::secrets::SecretStore::load(
            &dir.path().join("secrets.toml"),
        ));
        let called = std::rc::Rc::new(std::cell::Cell::new(false));
        let called2 = called.clone();
        let hook = move |_path: &Path, _provider: &str, _model: &str| {
            called2.set(true);
            Ok(true)
        };
        apply_inner(&approved_with_both_wires(), dir.path(), &store, false, hook).unwrap();
        assert!(!called.get(), "opt_in_llm=false 时不许碰 llm_optin::enable");
    }

    fn approved_with_both_wires() -> crate::pair::Approved {
        crate::pair::Approved {
            api_key: "sk-live".into(),
            models: crate::pair::Models {
                anthropic: crate::pair::WireModels {
                    default: Some("claude-x".into()),
                    small_fast: Some("claude-small".into()),
                },
                openai: crate::pair::WireModels {
                    default: Some("qwen3.8:27b".into()),
                    small_fast: Some("qwen-small".into()),
                },
            },
            platforms: BTreeMap::new(),
            quota: None,
        }
    }

    fn approved_openai_only() -> crate::pair::Approved {
        let mut a = approved_with_both_wires();
        a.models.anthropic = Default::default();
        a
    }
}

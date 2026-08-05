//! 界面设置的持久化。目前只有语言一项。
//!
//! 这个模块**只管读写，不做判断**：盘上没有可用的选择就返回 `None`，至于最终
//! 用哪种语言由 `i18n::resolve` 一处说了算。优先级规则散在两个文件里各写一半，
//! 是这类 bug 最常见的来源。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::i18n::Lang;

/// 盘上格式：`{"lang":"zh"}`。包一层对象而不是直接存字符串，是为了将来加设置项
/// 时老文件仍能读（跟 `projects.rs` 的 `Disk` 同一个理由）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Disk {
    #[serde(default)]
    lang: Option<String>,
}

/// 位置跟着 socket 走，与 `projects.rs::store_path_for_socket` 同一套推导：
/// 生产是 `~/.dct/settings.json`，集成测试把 socket 建在临时目录里就自动拿到
/// 一份隔离的设置，不会去动你真实的那份。
pub fn settings_path_for_socket(socket: &Path) -> PathBuf {
    match socket.parent() {
        Some(d) => d.join("settings.json"),
        None => PathBuf::from("settings.json"),
    }
}

/// 用户存过的语言。文件不存在、JSON 坏了、语言码不认识——一律 `None`。
/// 「盘上没有可用的选择」是一种情况，不是三种：调用方拿它去做的事完全一样
/// （交给 `resolve` 继续往下找），分成三种只会让每个调用点都抄一遍同样的兜底。
pub fn load_lang(path: &Path) -> Option<Lang> {
    let s = std::fs::read_to_string(path).ok()?;
    let disk: Disk = serde_json::from_str(&s).ok()?;
    Lang::from_code(&disk.lang?)
}

/// **跟 `projects.rs::save` 刻意不同：这里返回 `Result`。**
/// 「最近项目」是缓存，丢了无所谓，所以它吞掉一切错误；语言是用户明确做出的
/// 选择，写不进去必须说一声，否则他下次开 dct 发现语言变回去了，不知道该怪谁。
pub fn save_lang(path: &Path, lang: Lang) -> Result<()> {
    let parent = path
        .parent()
        .context("设置文件路径没有上级目录，存不了语言设置")?;
    std::fs::create_dir_all(parent).context("建不了设置目录，语言设置没存下来")?;

    // 先读回来再写，不是直接覆盖：将来多一个设置项时，存语言不能顺手把别的
    // 设置抹掉。现在只有一项，但这个坑要在只有一项的时候就填上。
    let mut disk: Disk = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    disk.lang = Some(lang.code().to_string());

    let json = serde_json::to_string(&disk).context("设置序列化失败")?;
    // 原子写：直接覆写的话，写到一半断电会留下半截 JSON，下次读就当成没设置过。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).context("写不了设置文件，语言设置没存下来")?;
    std::fs::rename(&tmp, path).context("设置文件换名失败，语言设置没存下来")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_language_survives_a_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("settings.json");
        save_lang(&f, Lang::Zh).unwrap();
        assert_eq!(load_lang(&f), Some(Lang::Zh));
    }

    #[test]
    fn a_missing_file_means_no_choice_was_made() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load_lang(&tmp.path().join("没有这个文件.json")), None);
    }

    /// 坏文件必须退化成「没设置过」，不能 panic——设置文件坏掉不该让 dct 起不来。
    #[test]
    fn a_corrupt_file_means_no_choice_was_made() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("settings.json");
        std::fs::write(&f, "{ 这不是 JSON").unwrap();
        assert_eq!(load_lang(&f), None);
    }

    /// 认不出的语言码（老版本存的、或者手改坏了）也是「没有可用的选择」。
    #[test]
    fn an_unknown_language_code_means_no_choice_was_made() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("settings.json");
        std::fs::write(&f, r#"{"lang":"klingon"}"#).unwrap();
        assert_eq!(load_lang(&f), None);
    }

    /// 将来多一个设置项时，存语言不能顺手把它抹掉。现在就钉住。
    #[test]
    fn saving_the_language_keeps_unknown_fields_out_of_harms_way() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("settings.json");
        std::fs::write(&f, r#"{"lang":"en"}"#).unwrap();
        save_lang(&f, Lang::Zh).unwrap();
        assert_eq!(load_lang(&f), Some(Lang::Zh), "新值要生效");
    }

    /// 写不进去必须报错，不能默默成功——这正是它跟 `projects.rs::save` 的区别。
    #[test]
    fn saving_into_an_unwritable_place_is_an_error_not_a_shrug() {
        // 用一个「上级是文件而不是目录」的路径：create_dir_all 必然失败
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("我是个文件");
        std::fs::write(&blocker, "x").unwrap();
        let f = blocker.join("settings.json");
        assert!(
            save_lang(&f, Lang::Zh).is_err(),
            "存不下去却返回 Ok，用户会以为自己选好了"
        );
    }

    #[test]
    fn settings_sit_next_to_the_socket() {
        assert_eq!(
            settings_path_for_socket(Path::new("/home/x/.dct/daemon.sock")),
            PathBuf::from("/home/x/.dct/settings.json")
        );
    }
}

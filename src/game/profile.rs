//! 棋盘配置：窗口、棋盘在窗口里的位置、行列数、读棋盘的参数。第一轮手写：内置一份 Candy Crush，
//! `~/.dct/games/<游戏>.toml` 存在就用它。棋盘位置每关不同，换关要改——自动校准是下一轮的事。
use dct_game::play::Profile;
use dct_game::Weights;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 第 1712 关的版面（出处：dc-octo `tests/grid_real.rs` 的 region；odd_share 0.15 是 dco 实测：
/// 包装糖也标得出来，且不误标普通糖）。
pub const CANDY_CRUSH: &str = r#"window = { app = "iPhone Mirroring" }
region = { x = 0.2372, y = 0.3156, w = 0.5257, h = 0.4622 }
rows = 9
cols = 5
inset = 0.6
class_de = 24
top_div = 5
odd_share = 0.15

# 关卡号在画面上的写法（一个捕获组）。有它、又读得出合格棋盘的画面就是棋盘，不看 HUD 上的符号。
# 过渡期的本地数据：长期要搬进 dcv。
level_pattern = '(\d{3,5})\s*/'

# 打分权重。这是过渡期的本地数据：长期要搬进 dcv，dct 的代码里不留任何游戏的数值。
# special_swap 写 0：两颗特殊糖互换却消不掉东西的步不走（特殊糖的标记会闪，会反复选到这种空步）。
[weights]
cleared = 1
cascade = 0.5
striped = 6
wrapped = 8
bomb = 15
triggered = 5
special_swap = 0
low_row = 1
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    window: Window,
    region: Region,
    rows: usize,
    cols: usize,
    inset: Option<f64>,
    class_de: Option<f64>,
    top_div: Option<u32>,
    odd_share: Option<f64>,
    /// 关卡号在画面上的写法：恰好一个捕获组的正则。过渡期的本地数据，长期搬进 dcv。
    level_pattern: Option<String>,
    /// 可选的重复表 `[[class]]`：把读出来的颜色类别标成「不是糖」（洞、蜂蜜块、糖果机）。
    #[serde(default)]
    class: Vec<ClassEntry>,
    /// 可选的打分权重表；没写的项保持通用的中性默认。
    weights: Option<WeightsEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WeightsEntry {
    cleared: Option<f64>,
    cascade: Option<f64>,
    striped: Option<f64>,
    wrapped: Option<f64>,
    bomb: Option<f64>,
    triggered: Option<f64>,
    special_swap: Option<f64>,
    low_row: Option<f64>,
}

/// 一类颜色。只有 `kind = "fixed"` 的会进 Profile；`candy` 只是写给人看的备注，被接受后忽略。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassEntry {
    name: String,
    rgb: [i64; 3],
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Window {
    app: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

pub struct Loaded {
    pub profile: Profile,
    /// 配置文本的 sha256（十六进制），记进每一步，事后才知道那一步是按哪份配置走的。
    pub sha256: String,
    /// 说给用户听的来源：「内置的」或文件路径。
    pub source: String,
}

pub fn games_dir(home: &Path) -> PathBuf {
    home.join(".dct").join("games")
}

pub fn load(home: &Path, game: &str) -> Result<Loaded, String> {
    if game.is_empty() || !game.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(format!("游戏名「{game}」只能用小写英文字母、数字和短横线"));
    }
    let path = games_dir(home).join(format!("{game}.toml"));
    let (text, source) = match std::fs::read_to_string(&path) {
        Ok(t) => (t, path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if game == "candy-crush" {
                (CANDY_CRUSH.to_string(), "内置的".to_string())
            } else {
                return Err(format!("不认识游戏「{game}」。现在只内置了 candy-crush，其它的要自己写 {}", path.display()));
            }
        }
        Err(e) => return Err(format!("读不了 {}：{e}", path.display())),
    };
    let f: File = toml::from_str(&text).map_err(|e| format!("{source} 写得不对：{e}"))?;
    check(&f).map_err(|m| format!("{source} 写得不对：{m}"))?;
    let mut extra = Map::new();
    if let Some(v) = f.inset {
        extra.insert("inset".into(), json!(v));
    }
    if let Some(v) = f.class_de {
        extra.insert("class_de".into(), json!(v));
    }
    if let Some(v) = f.top_div {
        extra.insert("top_div".into(), json!(v));
    }
    if let Some(v) = f.odd_share {
        extra.insert("odd_share".into(), json!(v));
    }
    // 颜色距离的阈值沿用读盘用的 class_de：「同一类」在两边是同一个意思
    let match_de = f.class_de.unwrap_or(24.0);
    let fixed_rgb: Vec<[u8; 3]> =
        f.class.iter().filter(|c| c.kind == "fixed").map(|c| [c.rgb[0] as u8, c.rgb[1] as u8, c.rgb[2] as u8]).collect();
    let mut weights = Weights::default();
    if let Some(w) = &f.weights {
        for (slot, v) in [
            (&mut weights.cleared, w.cleared),
            (&mut weights.cascade, w.cascade),
            (&mut weights.striped, w.striped),
            (&mut weights.wrapped, w.wrapped),
            (&mut weights.bomb, w.bomb),
            (&mut weights.triggered, w.triggered),
            (&mut weights.special_swap, w.special_swap),
            (&mut weights.low_row, w.low_row),
        ] {
            if let Some(v) = v {
                *slot = v;
            }
        }
    }
    let sha256 = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Ok(Loaded {
        profile: Profile {
            window: json!({ "app": f.window.app }),
            region: [f.region.x, f.region.y, f.region.w, f.region.h],
            rows: f.rows,
            cols: f.cols,
            extra: Value::Object(extra),
            fixed_rgb,
            match_de,
            weights,
            level_pattern: f.level_pattern,
        },
        sha256,
        source,
    })
}

fn check(f: &File) -> Result<(), String> {
    let r = &f.region;
    let inside = |v: f64| (0.0..=1.0).contains(&v);
    if ![r.x, r.y, r.w, r.h].iter().all(|&v| inside(v)) || r.w <= 0.0 || r.h <= 0.0 || r.x + r.w > 1.0 || r.y + r.h > 1.0 {
        return Err("region 要整个落在窗口里：x、y、w、h 都取 0 到 1，w 和 h 大于 0，x+w、y+h 不超过 1".into());
    }
    if !(1..=32).contains(&f.rows) || !(1..=32).contains(&f.cols) {
        return Err("rows、cols 都要在 1 到 32 之间".into());
    }
    for c in &f.class {
        if c.kind != "fixed" && c.kind != "candy" {
            return Err(format!("[[class]]「{}」的 kind 只能写 \"fixed\"（不是糖）或 \"candy\"（糖），现在写的是「{}」", c.name, c.kind));
        }
        if c.rgb.iter().any(|v| !(0..=255).contains(v)) {
            return Err(format!("[[class]]「{}」的 rgb 三个数都要在 0 到 255 之间", c.name));
        }
    }
    if let Some(pat) = &f.level_pattern {
        if pat.chars().count() > 100 {
            return Err("level_pattern 最长 100 个字符".into());
        }
        let re = regex::Regex::new(pat).map_err(|e| format!("level_pattern 不是合法的正则：{e}"))?;
        // captures_len 含整体匹配那一个，所以「恰好一个捕获组」是 2
        if re.captures_len() != 2 {
            return Err(format!("level_pattern 要恰好有一个括号捕获组（关卡号），现在有 {} 个", re.captures_len() - 1));
        }
    }
    if let Some(w) = &f.weights {
        for (name, v) in [
            ("cleared", w.cleared),
            ("cascade", w.cascade),
            ("striped", w.striped),
            ("wrapped", w.wrapped),
            ("bomb", w.bomb),
            ("triggered", w.triggered),
            ("special_swap", w.special_swap),
            ("low_row", w.low_row),
        ] {
            if let Some(v) = v {
                if !(0.0..=100.0).contains(&v) {
                    return Err(format!("[weights] 的 {name} 要在 0 到 100 之间，现在写的是 {v}"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(game: &str, text: &str) -> tempfile::TempDir {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(games_dir(h.path())).unwrap();
        std::fs::write(games_dir(h.path()).join(format!("{game}.toml")), text).unwrap();
        h
    }

    #[test]
    fn the_built_in_candy_crush_loads_and_passes_its_own_checks() {
        let h = tempfile::tempdir().unwrap();
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!((l.profile.rows, l.profile.cols), (9, 5));
        assert_eq!(l.profile.window["app"], "iPhone Mirroring");
        assert_eq!(l.profile.extra["odd_share"], 0.15);
        assert_eq!(l.source, "内置的");
        assert_eq!(l.sha256.len(), 64);
    }

    #[test]
    fn a_file_in_the_games_folder_wins_and_changes_the_fingerprint() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("rows = 9", "rows = 8"));
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!(l.profile.rows, 8);
        let builtin = load(tempfile::tempdir().unwrap().path(), "candy-crush").unwrap();
        assert_ne!(l.sha256, builtin.sha256);
        assert!(l.source.ends_with("candy-crush.toml"));
    }

    #[test]
    fn a_typo_names_the_file_and_the_line() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("cols = 5", "colz = 5"));
        let e = load(h.path(), "candy-crush").err().unwrap();
        assert!(e.contains("candy-crush.toml") && e.contains("colz"), "{e}");
    }

    #[test]
    fn out_of_range_values_are_refused_with_a_reason() {
        for (from, to) in [("rows = 9", "rows = 0"), ("cols = 5", "cols = 33"), ("w = 0.5257", "w = 0.9"), ("w = 0.5257", "w = 0.0")] {
            let h = home_with("candy-crush", &CANDY_CRUSH.replace(from, to));
            assert!(load(h.path(), "candy-crush").is_err(), "{to} 该被拒");
        }
    }

    #[test]
    fn unknown_games_and_unsafe_names_are_refused() {
        let h = tempfile::tempdir().unwrap();
        assert!(load(h.path(), "chess").err().unwrap().contains("不认识"));
        for bad in ["", "../x", "A", "a/b", "a.b"] {
            assert!(load(h.path(), bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn class_tables_split_into_fixed_colours_and_ignored_candies() {
        let text = format!(
            "{CANDY_CRUSH}\n[[class]]\nname = \"honey\"\nrgb = [196, 148, 101]\nkind = \"fixed\"\n\n[[class]]\nname = \"gap\"\nrgb = [10, 20, 30]\nkind = \"fixed\"\n\n[[class]]\nname = \"red\"\nrgb = [200, 30, 30]\nkind = \"candy\"\n"
        );
        let h = home_with("candy-crush", &text);
        let l = load(h.path(), "candy-crush").unwrap();
        assert_eq!(l.profile.fixed_rgb, vec![[196, 148, 101], [10, 20, 30]]);
        assert_eq!(l.profile.match_de, 24.0);
    }

    #[test]
    fn match_de_follows_class_de_when_the_file_has_one() {
        let h = home_with("candy-crush", &CANDY_CRUSH.replace("class_de = 24", "class_de = 18"));
        assert_eq!(load(h.path(), "candy-crush").unwrap().profile.match_de, 18.0);
    }

    #[test]
    fn a_bad_kind_is_refused_naming_the_file() {
        let text = format!("{CANDY_CRUSH}\n[[class]]\nname = \"x\"\nrgb = [1, 2, 3]\nkind = \"wall\"\n");
        let h = home_with("candy-crush", &text);
        let e = load(h.path(), "candy-crush").err().unwrap();
        assert!(e.contains("candy-crush.toml") && e.contains("kind"), "{e}");
    }

    #[test]
    fn an_rgb_out_of_range_or_a_missing_field_is_refused() {
        let bad_rgb = format!("{CANDY_CRUSH}\n[[class]]\nname = \"x\"\nrgb = [300, 2, 3]\nkind = \"fixed\"\n");
        let e = load(home_with("candy-crush", &bad_rgb).path(), "candy-crush").err().unwrap();
        assert!(e.contains("candy-crush.toml") && e.contains("rgb"), "{e}");
        let missing = format!("{CANDY_CRUSH}\n[[class]]\nname = \"x\"\nkind = \"fixed\"\n");
        let e = load(home_with("candy-crush", &missing).path(), "candy-crush").err().unwrap();
        assert!(e.contains("candy-crush.toml"), "{e}");
    }

    #[test]
    fn a_file_without_class_tables_has_no_fixed_colours() {
        let h = home_with("candy-crush", CANDY_CRUSH);
        assert!(load(h.path(), "candy-crush").unwrap().profile.fixed_rgb.is_empty());
    }

    #[test]
    fn a_weights_table_loads_its_numbers_and_keeps_defaults_for_missing_keys() {
        let text = format!("{}\n[weights]\nstriped = 3.5\nspecial_swap = 20\n", CANDY_CRUSH.split("\n# 打分权重").next().unwrap());
        let w = load(home_with("candy-crush", &text).path(), "candy-crush").unwrap().profile.weights;
        assert_eq!((w.striped, w.special_swap), (3.5, 20.0));
        assert_eq!((w.cleared, w.cascade, w.wrapped, w.low_row), (1.0, 0.5, 0.0, 1.0));
    }

    #[test]
    fn weights_out_of_range_or_unknown_are_refused_naming_the_file() {
        let base = CANDY_CRUSH.split("\n# 打分权重").next().unwrap();
        for body in ["striped = 101", "bomb = -1", "sparkle = 3"] {
            let e = load(home_with("candy-crush", &format!("{base}\n[weights]\n{body}\n")).path(), "candy-crush").err().unwrap();
            assert!(e.contains("candy-crush.toml"), "{body}: {e}");
        }
    }

    #[test]
    fn level_pattern_loads_and_bad_ones_are_refused_naming_the_file() {
        let base = CANDY_CRUSH.split("\nlevel_pattern").next().unwrap().to_string() + "\n";
        let rest = format!("\n[weights]{}", CANDY_CRUSH.split("\n[weights]").nth(1).unwrap());
        let with = |pat: &str| format!("{base}level_pattern = '{pat}'\n{rest}");
        let l = load(home_with("candy-crush", &with(r"(\d+)/")).path(), "candy-crush").unwrap();
        assert_eq!(l.profile.level_pattern.as_deref(), Some(r"(\d+)/"));
        assert_eq!(load(tempfile::tempdir().unwrap().path(), "candy-crush").unwrap().profile.level_pattern.as_deref(), Some(r"(\d{3,5})\s*/"));
        let long = format!("({})", "a".repeat(100));
        for bad in ["(unclosed", r"\d+/", r"(\d+)/(\d+)", long.as_str()] {
            let e = load(home_with("candy-crush", &with(bad)).path(), "candy-crush").err().unwrap();
            assert!(e.contains("candy-crush.toml") && e.contains("level_pattern"), "{bad}: {e}");
        }
        let none = home_with("candy-crush", &format!("{base}{rest}"));
        assert_eq!(load(none.path(), "candy-crush").unwrap().profile.level_pattern, None);
    }

    #[test]
    fn a_file_without_weights_loads_neutral_weights() {
        let base = CANDY_CRUSH.split("\n# 打分权重").next().unwrap();
        let w = load(home_with("candy-crush", base).path(), "candy-crush").unwrap().profile.weights;
        assert_eq!(w, Weights::default());
        assert_eq!((w.striped, w.special_swap), (0.0, 0.0));
    }

    #[test]
    fn the_built_in_candy_crush_carries_its_tuning_without_the_junk_special_swap() {
        let w = load(tempfile::tempdir().unwrap().path(), "candy-crush").unwrap().profile.weights;
        assert_eq!((w.striped, w.wrapped, w.bomb, w.triggered), (6.0, 8.0, 15.0, 5.0));
        assert_eq!((w.special_swap, w.cleared, w.cascade, w.low_row), (0.0, 1.0, 0.5, 1.0));
    }
}

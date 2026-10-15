//! `dct do` 和用户说话：终端（编号清单、y/N）或 `--json`（stdout 一行一个事件，stdin 一行一个回答）。
//! 进来的选项文字和原因已经被摸索循环脱敏（参数值换回 `{名字}`）；这里不再加任何画面原文。
use dct_explore::ladder::UserPick;
use dct_explore::options::{Act, Opt};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// 一个回答读不懂时最多再问几次。
const MAX_TRIES: u32 = 3;

pub trait Say {
    /// 让用户从选项里挑一个。None = 用户说停（或输入读完了）。
    fn pick(&mut self, all: &[Opt], picture: Option<&str>, why: &str) -> Option<UserPick>;
    /// 问一句是 / 否。终端读 y/N，JSON 读 `{"verdict":"yes"}`；其余一律当「不」。
    fn yes(&mut self, event: &str, text: &str, extra: Value, picture: Option<&str>) -> bool;
    /// 只说一句，不等回答。
    fn tell(&mut self, event: &str, text: &str, extra: Value);
}

pub struct Talk<R, W> {
    pub r: R,
    pub w: W,
    pub json: bool,
    /// 编号图存在这里（`~/.dct/explore/runs/<编号>`）；None = 不存。
    pics: Option<PathBuf>,
    saved: u32,
}

impl<R: BufRead, W: Write> Talk<R, W> {
    pub fn new(r: R, w: W, json: bool, pics: Option<PathBuf>) -> Self {
        Talk {
            r,
            w,
            json,
            pics,
            saved: 0,
        }
    }

    /// 读一行；读完了（或读不了）= None。
    fn line(&mut self) -> Option<String> {
        let mut s = String::new();
        match self.r.read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s),
        }
    }

    fn say_line(&mut self, text: &str) {
        let _ = writeln!(self.w, "{text}");
        let _ = self.w.flush();
    }

    fn event(&mut self, event: &str, mut v: Value) {
        if !v.is_object() {
            v = Value::Object(Default::default());
        }
        v["event"] = Value::from(event);
        self.say_line(&v.to_string());
    }

    /// 编号图存成 `ask-<n>.jpg`（0600），返回路径；没图、存不下 = None（照样问，只是没图）。
    fn save_picture(&mut self, b64: Option<&str>) -> Option<PathBuf> {
        use base64::Engine;
        let dir = self.pics.clone()?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64?.trim())
            .ok()?;
        std::fs::create_dir_all(&dir).ok()?;
        self.saved += 1;
        let path = dir.join(format!("ask-{}.jpg", self.saved));
        write_private(&path, &bytes).ok()?;
        Some(path)
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)
}

/// 没字、也还没名字的图标：用户选它就得起个名字。
fn needs_name(o: &Opt) -> bool {
    matches!(&o.act, Act::TapIcon { name, .. } if name.as_deref().is_none_or(|n| n.trim().is_empty()))
}

/// 终端里的一行选择：编号（从 1 起）或选项名；`q` / `停` = 停下。
pub fn choose(line: &str, all: &[Opt]) -> Result<Option<usize>, String> {
    let t = line.trim();
    if matches!(t, "q" | "Q" | "停" | "停下") {
        return Ok(None);
    }
    if let Ok(n) = t.parse::<usize>() {
        if (1..=all.len()).contains(&n) {
            return Ok(Some(n - 1));
        }
    } else if let Some(i) = all.iter().position(|o| o.key == t) {
        return Ok(Some(i));
    }
    Err(format!(
        "没有这个选项：「{t}」。输入 1 到 {} 的编号，或 q 停下。",
        all.len()
    ))
}

/// `--json` 的一行回答：`{"key": …, "name": …}` 或 `{"stop": true}`。
pub fn json_pick(line: &str, all: &[Opt]) -> Result<Option<UserPick>, String> {
    let v: Value =
        serde_json::from_str(line.trim()).map_err(|_| "回答要是一行 JSON".to_string())?;
    if v["stop"].as_bool() == Some(true) {
        return Ok(None);
    }
    let key = v["key"]
        .as_str()
        .ok_or_else(|| "回答里要有 key（选项名），或 {\"stop\": true}".to_string())?;
    let opt = all
        .iter()
        .find(|o| o.key == key)
        .ok_or_else(|| format!("没有这个选项：{key}"))?;
    let name = v["name"]
        .as_str()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if needs_name(opt) && name.is_none() {
        return Err(format!("{key} 是没有字的图标，要带上 name（它叫什么）"));
    }
    Ok(Some(UserPick {
        key: key.to_string(),
        name: if needs_name(opt) { name } else { None },
    }))
}

fn is_yes(line: &str) -> bool {
    matches!(
        line.trim().to_lowercase().as_str(),
        "y" | "yes" | "是" | "好" | "确认"
    )
}

impl<R: BufRead, W: Write> Talk<R, W> {
    fn pick_json(&mut self, all: &[Opt], pic: Option<PathBuf>, why: &str) -> Option<UserPick> {
        let options: Vec<Value> = all
            .iter()
            .map(|o| serde_json::json!({"key": o.key, "text": o.text, "needs_name": needs_name(o)}))
            .collect();
        self.event(
            "ask",
            serde_json::json!({"why": why, "options": options,
                "picture": pic.map(|p| p.display().to_string())}),
        );
        for _ in 0..MAX_TRIES {
            match json_pick(&self.line()?, all) {
                Ok(p) => return p,
                Err(m) => self.event("error", serde_json::json!({"message": m})),
            }
        }
        None
    }

    fn pick_terminal(&mut self, all: &[Opt], pic: Option<PathBuf>, why: &str) -> Option<UserPick> {
        self.say_line(&format!("需要你来选（{why}）："));
        for (i, o) in all.iter().enumerate() {
            self.say_line(&format!("  {}. {}", i + 1, o.text));
        }
        if let Some(p) = pic {
            self.say_line(&format!("编号图：{}", p.display()));
        }
        let mut chosen = None;
        for _ in 0..MAX_TRIES {
            self.say_line("输入编号（或选项名），q 停下：");
            match choose(&self.line()?, all) {
                Ok(None) => return None,
                Ok(Some(i)) => {
                    chosen = Some(i);
                    break;
                }
                Err(m) => self.say_line(&m),
            }
        }
        let opt = &all[chosen?];
        if !needs_name(opt) {
            return Some(UserPick {
                key: opt.key.clone(),
                name: None,
            });
        }
        for _ in 0..MAX_TRIES {
            self.say_line("这个图标叫什么？");
            let n = self.line()?.trim().to_string();
            if !n.is_empty() {
                return Some(UserPick {
                    key: opt.key.clone(),
                    name: Some(n),
                });
            }
        }
        None
    }
}

impl<R: BufRead, W: Write> Say for Talk<R, W> {
    fn pick(&mut self, all: &[Opt], picture: Option<&str>, why: &str) -> Option<UserPick> {
        let pic = self.save_picture(picture);
        if self.json {
            self.pick_json(all, pic, why)
        } else {
            self.pick_terminal(all, pic, why)
        }
    }
    fn yes(&mut self, event: &str, text: &str, mut extra: Value, picture: Option<&str>) -> bool {
        let pic = self.save_picture(picture);
        if self.json {
            if !extra.is_object() {
                extra = Value::Object(Default::default());
            }
            extra["message"] = Value::from(text);
            extra["picture"] = pic.map(|p| p.display().to_string()).into();
            self.event(event, extra);
            let Some(l) = self.line() else { return false };
            serde_json::from_str::<Value>(l.trim())
                .ok()
                .is_some_and(|v| v["verdict"].as_str() == Some("yes"))
        } else {
            if let Some(p) = pic {
                self.say_line(&format!("画面：{}", p.display()));
            }
            if text.trim_end().ends_with("[y/N]") {
                self.say_line(text);
            } else {
                self.say_line(&format!("{text} [y/N]"));
            }
            self.line().is_some_and(|l| is_yes(&l))
        }
    }
    fn tell(&mut self, event: &str, text: &str, mut extra: Value) {
        if self.json {
            if !extra.is_object() {
                extra = Value::Object(Default::default());
            }
            extra["message"] = Value::from(text);
            self.event(event, extra);
        } else {
            self.say_line(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use dct_explore::hands::{El, Frac};
    use serde_json::json;
    use std::io::Cursor;

    fn el(id: &str) -> El {
        El {
            id: id.into(),
            text: String::new(),
            role: "image".into(),
            frac: Frac {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            source: "detector".into(),
        }
    }
    fn opts() -> Vec<Opt> {
        vec![
            Opt {
                key: "e3".into(),
                text: "按钮 Post".into(),
                act: Act::Tap {
                    el: el("e3"),
                    intent: "Post".into(),
                },
                publish: false,
            },
            Opt {
                key: "e5".into(),
                text: "没有字的图标（在 Post 旁边）".into(),
                act: Act::TapIcon {
                    el: el("e5"),
                    name: None,
                },
                publish: false,
            },
            Opt {
                key: "stuck".into(),
                text: "卡住了".into(),
                act: Act::Stuck,
                publish: false,
            },
        ]
    }
    fn talk(input: &str, json: bool) -> Talk<Cursor<Vec<u8>>, Vec<u8>> {
        Talk::new(
            Cursor::new(input.as_bytes().to_vec()),
            Vec::new(),
            json,
            None,
        )
    }
    fn out(t: &Talk<Cursor<Vec<u8>>, Vec<u8>>) -> String {
        String::from_utf8(t.w.clone()).unwrap()
    }
    fn events(t: &Talk<Cursor<Vec<u8>>, Vec<u8>>) -> Vec<Value> {
        out(t)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
            .collect()
    }

    #[test]
    fn a_typed_choice_is_a_number_or_a_key_and_is_checked_against_the_options() {
        let o = opts();
        assert_eq!(choose("1", &o), Ok(Some(0)));
        assert_eq!(choose(" 2 \n", &o), Ok(Some(1)));
        assert_eq!(choose("stuck", &o), Ok(Some(2)));
        assert_eq!(choose("q", &o), Ok(None));
        assert_eq!(choose("停", &o), Ok(None));
        assert!(choose("0", &o).is_err());
        assert!(choose("4", &o).is_err());
        assert!(choose("e9", &o).is_err());
        assert!(choose("", &o).is_err());
    }

    #[test]
    fn the_terminal_lists_numbered_options_and_asks_again_after_a_bad_answer() {
        let mut t = talk("9\n1\n", false);
        let p = t.pick(&opts(), None, "Jeff 没有把握").unwrap();
        assert_eq!((p.key.as_str(), p.name), ("e3", None));
        let s = out(&t);
        assert!(s.contains("Jeff 没有把握"), "{s}");
        assert!(s.contains("1. 按钮 Post") && s.contains("3. 卡住了"), "{s}");
        assert!(s.contains("没有这个选项"), "{s}");
    }

    #[test]
    fn a_text_less_icon_asks_what_it_is_called() {
        let mut t = talk("2\n\n  加图片  \n", false);
        let p = t.pick(&opts(), None, "w").unwrap();
        assert_eq!((p.key.as_str(), p.name.as_deref()), ("e5", Some("加图片")));
        assert!(out(&t).contains("这个图标叫什么？"));
    }

    #[test]
    fn end_of_input_or_q_means_stop() {
        assert!(talk("", false).pick(&opts(), None, "w").is_none());
        assert!(talk("q\n", false).pick(&opts(), None, "w").is_none());
        assert!(
            talk("2\n", false).pick(&opts(), None, "w").is_none(),
            "图标没名字就读完了 = 停"
        );
        assert!(
            talk("x\nx\nx\n1\n", false)
                .pick(&opts(), None, "w")
                .is_none(),
            "连错 3 次就停"
        );
    }

    #[test]
    fn the_marks_picture_is_saved_privately_and_its_path_shown() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("runs").join("x-20261011-0001");
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"JPEGDATA");
        let mut t = Talk::new(
            Cursor::new(b"1\n".to_vec()),
            Vec::new(),
            false,
            Some(dir.clone()),
        );
        t.pick(&opts(), Some(&b64), "w").unwrap();
        let f = dir.join("ask-1.jpg");
        assert_eq!(std::fs::read(&f).unwrap(), b"JPEGDATA");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert!(String::from_utf8(t.w.clone())
            .unwrap()
            .contains(&f.display().to_string()));
    }

    #[test]
    fn a_json_answer_must_name_an_option_and_give_an_icon_a_name() {
        let o = opts();
        let p = json_pick(r#"{"key":"e3"}"#, &o).unwrap().unwrap();
        assert_eq!(p.key, "e3");
        let p = json_pick(r#"{"key":"e5","name":"add media"}"#, &o)
            .unwrap()
            .unwrap();
        assert_eq!(p.name.as_deref(), Some("add media"));
        assert!(
            json_pick(r#"{"key":"e5"}"#, &o).is_err(),
            "没字的图标要名字"
        );
        assert!(json_pick(r#"{"key":"e5","name":"  "}"#, &o).is_err());
        assert!(json_pick(r#"{"key":"e9"}"#, &o).is_err());
        assert!(json_pick("not json", &o).is_err());
        assert!(json_pick(r#"{"key":3}"#, &o).is_err());
        assert!(json_pick(r#"{"stop":true}"#, &o).unwrap().is_none());
    }

    #[test]
    fn json_mode_writes_one_event_per_line_and_reads_one_answer_per_line() {
        let mut t = talk("{\"key\":\"e9\"}\n{\"key\":\"e3\"}\n", true);
        let p = t.pick(&opts(), None, "Jeff 没有回答").unwrap();
        assert_eq!(p.key, "e3");
        let ev = events(&t);
        assert_eq!(ev[0]["event"], "ask");
        assert_eq!(ev[0]["why"], "Jeff 没有回答");
        assert_eq!(ev[0]["options"][1]["key"], "e5");
        assert_eq!(ev[0]["options"][1]["needs_name"], true);
        assert_eq!(ev[0]["options"][0]["needs_name"], false);
        assert_eq!(ev[1]["event"], "error");
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn yes_reads_y_in_the_terminal_and_a_verdict_in_json() {
        assert!(talk("y\n", false).yes("card", "确认吗？[y/N]", json!({}), None));
        assert!(talk("是\n", false).yes("card", "确认吗？", json!({}), None));
        assert!(!talk("\n", false).yes("card", "确认吗？", json!({}), None));
        assert!(!talk("", false).yes("card", "确认吗？", json!({}), None));
        let mut t = talk("{\"verdict\":\"yes\"}\n", true);
        assert!(t.yes("card", "确认吗？", json!({"procedure": "v:p"}), None));
        let ev = events(&t);
        assert_eq!(
            (ev[0]["event"].as_str(), ev[0]["procedure"].as_str()),
            (Some("card"), Some("v:p"))
        );
        assert!(!talk("{\"verdict\":\"no\"}\n", true).yes("card", "?", json!({}), None));
        assert!(
            !talk("yes\n", true).yes("card", "?", json!({}), None),
            "JSON 模式只认 JSON"
        );
    }

    #[test]
    fn tell_is_a_plain_line_or_one_json_event() {
        let mut t = talk("", false);
        t.tell("done", "发出去了", json!({"exit": 0}));
        assert_eq!(out(&t), "发出去了\n");
        let mut t = talk("", true);
        t.tell("done", "发出去了", json!({"exit": 0}));
        let ev = events(&t);
        assert_eq!(
            ev[0],
            json!({"event": "done", "message": "发出去了", "exit": 0})
        );
    }
}

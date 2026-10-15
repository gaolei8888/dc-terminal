//! 画面原文：只在内存里给规矩用——停下（登录 / 验证码 / 警告）、禁点词、档位、发布词。
//!
//! 脱敏（参数值换回 `{名字}`）只管「出去的字」：给 Jeff / 大模型 / 用户看的 state 和选项、轨迹、
//! 训练题、报上去的那行字。规矩必须看原文，否则一段写着「Log in…」「follow us」的帖子会把
//! 登录墙、关注按钮脱敏成 `{post_text}`，规矩就失灵了。
//!
//! 值元素（整段就是帖子的一部分，见 `scrub.rs`）不参加任何规矩：打进发帖框的帖子里有「扫码」「locked」
//! 「follow」也不会停下或让发帖框变得不能点。值元素也不能按它自己的字去点。
//!
//! `Raw` 不实现 Debug / Clone / Serialize，原文拿不出这个类型：对外只回答「停不停」（那行字已脱敏）、
//! 「能不能点」「是不是发布词」。
use super::screen::Scrub;
use crate::app_profile::AppProfile;
use crate::gate::{fold, screen_halt, tappable, Halt};
use crate::hands::El;

pub(crate) struct Raw {
    els: Vec<El>,
    /// 每个元素是不是值元素（是的话是哪个参数）。
    value: Vec<Option<String>>,
}

impl Raw {
    pub(crate) fn new(els: &[El], scrub: &Scrub) -> Raw {
        Raw {
            els: els.to_vec(),
            value: els
                .iter()
                .map(|e| scrub.value_of(&e.text).map(str::to_owned))
                .collect(),
        }
    }

    fn ui(&self) -> impl Iterator<Item = &El> {
        self.els
            .iter()
            .zip(&self.value)
            .filter(|(_, v)| v.is_none())
            .map(|(e, _)| e)
    }

    /// 按界面元素的原文判断要不要停；报上去的那行字脱敏。
    pub(crate) fn halt(&self, scrub: &Scrub) -> Option<Halt> {
        let ui: Vec<El> = self.ui().cloned().collect();
        screen_halt(&ui).map(|h| match h {
            Halt::Login(l) => Halt::Login(scrub.line(&l)),
            Halt::Captcha(l) => Halt::Captcha(scrub.line(&l)),
            Halt::Warning(l) => Halt::Warning(scrub.line(&l)),
        })
    }

    /// 这个元素（按 id，取第一个）的原文能不能点：值元素不行；禁点词（通用 + 平台）、档位 >= Critical
    /// 一律不行。没字的元素照常（图标的名字另外管）；找不到这个 id 的当不能点。
    pub(crate) fn tappable(&self, id: &str, p: &AppProfile) -> bool {
        let Some(i) = self.els.iter().position(|e| e.id == id) else {
            return false;
        };
        let t = self.els[i].text.trim();
        self.value[i].is_none() && (t.is_empty() || tappable(t, p))
    }

    /// 记下的「填字」选项在这一屏还用得上：有个界面元素的原文就是它的意图（发帖框的提示字），
    /// 或者这个参数的值元素在画面上（发帖框里还剩半截帖子 / 提示字和帖子开头一样）。
    pub(crate) fn has_field(&self, intent: &str, param: &str) -> bool {
        let want = fold(intent.trim());
        self.els.iter().zip(&self.value).any(|(e, v)| match v {
            Some(p) => p == param,
            None => !want.is_empty() && fold(e.text.trim()) == want,
        })
    }

    /// 画面上还有能点的发布按钮：界面元素的原文（fold 后）整句等于平台的发布词，且能点（同 `all_options` 认发布的规矩）。
    pub(crate) fn has_publish_button(&self, p: &AppProfile) -> bool {
        let words: Vec<String> = p.publish_words.iter().map(|w| fold(w)).collect();
        self.ui().any(|e| {
            let t = e.text.trim();
            let f = fold(t);
            !f.is_empty() && words.contains(&f) && tappable(t, p)
        })
    }

    /// 画面上有平台表里核实过的「发出去了」那行字（`publish_done` 有、且不在 `unverified` 里）。
    /// 只看界面元素：帖子里写着同样的字不算。
    pub(crate) fn shows_publish_done(&self, p: &AppProfile) -> bool {
        if p.unverified.iter().any(|u| u == "publish_done") {
            return false;
        }
        let Some(done) = p.publish_done.as_deref().map(fold) else {
            return false;
        };
        let done = done.trim().to_string();
        !done.is_empty() && self.ui().any(|e| fold(&e.text).contains(&done))
    }

    /// 脱敏后的元素，给外面看（值元素是 `{名字}`）。界面元素的原文正好是平台的发布词时保留原字（那是平台的词，不是用户的内容），
    /// 这样发布按钮不会因为帖子里也有这个词而认不出来。
    pub(crate) fn shown(&self, scrub: &Scrub, p: &AppProfile) -> Vec<El> {
        let words: Vec<String> = p.publish_words.iter().map(|w| fold(w)).collect();
        let mut out = scrub.els(&self.els);
        for ((o, e), v) in out.iter_mut().zip(&self.els).zip(&self.value) {
            let f = fold(e.text.trim());
            if v.is_none() && !f.is_empty() && words.contains(&f) {
                o.text = e
                    .text
                    .trim()
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
            }
        }
        out
    }
}

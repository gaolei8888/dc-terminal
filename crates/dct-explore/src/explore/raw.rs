//! 画面原文：只在内存里给规矩用——停下（登录 / 验证码 / 警告）、禁点词、档位、发布词。
//!
//! 脱敏（参数值换回 `{名字}`）只管「出去的字」：给 Jeff / 大模型 / 用户看的 state 和选项、轨迹、
//! 训练题、报上去的那行字。规矩必须看原文，否则一段写着「Log in…」「follow us」的帖子会把
//! 登录墙、关注按钮脱敏成 `{post_text}`，规矩就失灵了。
//!
//! `Raw` 不实现 Debug / Clone / Serialize，原文拿不出这个类型：对外只回答「停不停」（那行字已脱敏）、
//! 「能不能点」「是不是发布词」。
use super::screen::Scrub;
use crate::gate::{fold, screen_halt, tappable, Halt};
use crate::hands::El;
use crate::platform::Platform;

pub(crate) struct Raw(Vec<El>);

impl Raw {
    pub(crate) fn new(els: &[El]) -> Raw {
        Raw(els.to_vec())
    }

    /// 按原文判断要不要停；报上去的那行字脱敏。
    pub(crate) fn halt(&self, scrub: &Scrub) -> Option<Halt> {
        screen_halt(&self.0).map(|h| match h {
            Halt::Login(l) => Halt::Login(scrub.line(&l)),
            Halt::Captcha(l) => Halt::Captcha(scrub.line(&l)),
            Halt::Warning(l) => Halt::Warning(scrub.line(&l)),
        })
    }

    /// 这个元素（按 id，取第一个）的原文能不能点：禁点词（通用 + 平台）、档位 >= Critical 一律不行。
    /// 没字的元素照常（图标的名字另外管）；找不到这个 id 的当不能点。
    pub(crate) fn tappable(&self, id: &str, p: &Platform) -> bool {
        self.0.iter().find(|e| e.id == id).is_some_and(|e| {
            let t = e.text.trim();
            t.is_empty() || tappable(t, p)
        })
    }

    /// 脱敏后的元素，给外面看。原文正好是平台的发布词时保留原字（那是平台的词，不是用户的内容），
    /// 这样发布按钮不会因为帖子里也有这个词而认不出来。
    pub(crate) fn shown(&self, scrub: &Scrub, p: &Platform) -> Vec<El> {
        let words: Vec<String> = p.publish_words.iter().map(|w| fold(w)).collect();
        let mut out = scrub.els(&self.0);
        for (o, e) in out.iter_mut().zip(&self.0) {
            let f = fold(e.text.trim());
            if !f.is_empty() && words.contains(&f) {
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

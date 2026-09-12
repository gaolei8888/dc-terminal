//! 守护进程这一侧的直播状态槽。
//!
//! **同一时刻最多一场直播**——学生看的是「老师现在在播的这一场」，不是
//! 「老师今天开过的第几场」，多场并存没有对应的界面概念，先不做。
//!
//! 这里只管「有没有在播、钥匙是什么、上架了哪几路」这几件事本身；真正
//! 把终端画面推出去的线程是下一个任务的范围（见
//! `.superpowers/sdd/2026-09-12-live-viewer-link/progress.md` 里 T4→T5 那条
//! 预检裁决）。

use std::sync::Mutex;

use crate::proto::LiveInfo;

/// 一场直播的内部记录。跟 [`LiveInfo`] 分开，是因为 [`LiveInfo`] 要经手线
/// 上协议、`Debug` 已经把两把钥匙打了码——这里是 daemon 自己进程内存里的
/// 那一份原文，两者不能是同一个类型。
struct Room {
    id: String,
    token: String,
    push_secret: String,
    staged: Vec<(u32, String)>,
}

/// 守护进程里的直播状态槽。
pub struct LiveState {
    /// 学生链接的 origin，比如 `https://example.tzspace.cn`——`start()` 拼
    /// 链接时要用，跟中转那一侧约定好的地址由调用方传进来，这里不猜。
    base: String,
    room: Mutex<Option<Room>>,
}

impl LiveState {
    pub fn new(base: String) -> Self {
        Self {
            base,
            room: Mutex::new(None),
        }
    }

    /// 开一场：生成 live-id、两把钥匙、拼出学生链接。
    ///
    /// 后开的这一场**直接顶掉**前一场（如果有）——`stop()` 之外还有一条能
    /// 让上一场结束的路，避免忘了手动停播还得再点一次。
    pub fn start(&self, staged: Vec<(u32, String)>) -> LiveInfo {
        let id = new_hex(dct_link::live::LIVE_ID_LEN);
        let token = new_hex(dct_link::live::LIVE_TOKEN_LEN);
        let push_secret = new_hex(dct_link::live::LIVE_TOKEN_LEN);
        // token 放在 fragment 里：浏览器不会把 `#` 后面的东西发给服务器，
        // 它不进任何一层访问日志（中转的、反代的都不进）。
        let url = format!("{}/live/{id}#t={token}", self.base);

        let info = LiveInfo {
            id: id.clone(),
            token: token.clone(),
            push_secret: push_secret.clone(),
            url,
            staged: staged.clone(),
            viewers: 0,
        };

        *recover(self.room.lock()) = Some(Room {
            id,
            token,
            push_secret,
            staged,
        });

        info
    }

    pub fn stop(&self) {
        *recover(self.room.lock()) = None;
    }

    /// 没在播的时候：`id`/`token`/`push_secret`/`url` 都是空串，`staged`
    /// 是空表，`viewers` 是 0。**空串而不是 `Option`**——`LiveInfo` 是要经过
    /// 协议线的形状，`Option<LiveInfo>` 才是「有没有在播」该长的样子，但这
    /// 一层就要先定下「没有」具体长什么样，好让界面不用先判断一次「有没有
    /// 这个字段」才能往下渲染；空串本身就是一个不会被误认成真实 id/链接的
    /// 值。观众数眼下没有真实来源（推帧线程是下一个任务的事），在播时也
    /// 先答 0，不假装知道。
    pub fn info(&self) -> LiveInfo {
        match &*recover(self.room.lock()) {
            Some(room) => LiveInfo {
                id: room.id.clone(),
                token: room.token.clone(),
                push_secret: room.push_secret.clone(),
                url: format!("{}/live/{}#t={}", self.base, room.id, room.token),
                staged: room.staged.clone(),
                viewers: 0,
            },
            None => LiveInfo {
                id: String::new(),
                token: String::new(),
                push_secret: String::new(),
                url: String::new(),
                staged: Vec::new(),
                viewers: 0,
            },
        }
    }
}

/// 生成 `hex_len` 个十六进制字符的随机串，取自系统 CSPRNG。**不是**时间戳、
/// **不是**引入 `rand`——两把直播钥匙和 live-id 都是能让人推帧/停播/看到别
/// 人直播的凭据，可预测的来源在这里是安全缺陷，不是实现细节。做法照抄
/// `web::mod::new_token`。
fn new_hex(hex_len: usize) -> String {
    let mut raw = vec![0u8; hex_len / 2];
    getrandom::getrandom(&mut raw).expect("系统随机数不可用，没法生成直播钥匙");
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// 拿到一把互斥锁；中毒（另一个持锁线程 panic 了）也要能继续用——直播状态
/// 槽跟手机通知那份共享状态是同一个道理，守护进程活得久，不能因为一次
/// panic 就让这个槽永远锁死。
fn recover<T>(r: std::sync::LockResult<T>) -> T {
    r.unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没在播的时候：`info()` 回一个空但完整的形状，界面不用先判断「这个
    /// 字段存不存在」才能往下渲染。
    #[test]
    fn info_before_any_start_is_an_empty_but_complete_shape() {
        let live = LiveState::new("https://x".into());
        let info = live.info();
        assert_eq!(info.id, "");
        assert_eq!(info.token, "");
        assert_eq!(info.push_secret, "");
        assert_eq!(info.url, "");
        assert!(info.staged.is_empty());
        assert_eq!(info.viewers, 0);
    }

    /// 链接形状固定：`{base}/live/{id}#t={token}`，token 在 fragment 里。
    #[test]
    fn start_builds_the_student_link_with_the_token_in_the_fragment() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![(1, "前端".into())]);
        assert_eq!(
            info.url,
            format!("https://x/live/{}#t={}", info.id, info.token)
        );
    }

    /// 两把钥匙必须不同——合成一把的话，学生手上那份链接就能拿去推假画面
    /// （见 `LiveInfo` 上的文档注释）。
    #[test]
    fn the_viewer_token_and_the_push_secret_are_different_keys() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![]);
        assert_ne!(info.token, info.push_secret);
    }

    /// id 和两把钥匙的长度要跟 `dct_link::live` 里定的常量对得上——
    /// 两个 crate 各写一份长度就是「一边改了另一边没改」。
    #[test]
    fn the_generated_lengths_match_the_shared_constants() {
        let live = LiveState::new("https://x".into());
        let info = live.start(vec![]);
        assert_eq!(info.id.len(), dct_link::live::LIVE_ID_LEN);
        assert_eq!(info.token.len(), dct_link::live::LIVE_TOKEN_LEN);
        assert_eq!(info.push_secret.len(), dct_link::live::LIVE_TOKEN_LEN);
    }

    /// 停播之后 `info()` 要回到「没在播」的那个空形状，不能留着上一场的
    /// 尾巴——尤其是钥匙，停播之后它们不该再答得出来。
    #[test]
    fn stop_clears_the_room() {
        let live = LiveState::new("https://x".into());
        live.start(vec![(1, "前端".into())]);
        live.stop();
        let info = live.info();
        assert_eq!(info.id, "");
        assert!(info.staged.is_empty());
    }

    /// 再开一场直接顶掉上一场——不用先手动停播。
    #[test]
    fn starting_again_replaces_the_previous_room() {
        let live = LiveState::new("https://x".into());
        let first = live.start(vec![(1, "前端".into())]);
        let second = live.start(vec![(2, "后端".into())]);
        assert_ne!(first.id, second.id);
        let info = live.info();
        assert_eq!(info.id, second.id);
        assert_eq!(info.staged, vec![(2, "后端".into())]);
    }
}

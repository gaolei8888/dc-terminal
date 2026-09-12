//! 直播那条单向管的共享定义。
//!
//! **两侧唯一的真相来源。** 推帧的是 `dct`，存帧发帧的是 `dct-srv`，两个
//! crate 谁也不依赖谁——各写一份常量就是「一边改了另一边没改」，而这类
//! 错误只在真机上、上课当中才看得见。

use std::time::Duration;

/// 老师开播（或者自己重开同一场）的路径。body 里带 `id`/`viewer_token`/
/// `push_secret`/`lanes`——这是中转唯一得知这几件事的地方，推帧线程在
/// 第一次推帧之前必须先调这条路径，不然中转不认得这个 id。
pub const PATH_START: &str = "/live/start";

/// 老师推帧的路径。live-id 和 lane 在 body 的头里，不在 URL 上——推帧要带
/// 配对身份，URL 上再带一遍 id 只是多一处会对不上的地方。
pub const PATH_FRAME: &str = "/live/frame";

/// 学生那一侧所有路径的前缀。
pub const LIVE_PREFIX: &str = "/live";

/// 一帧最大多少字节（gzip 之后）。一屏终端压完 3–5 KB，256 KB 已经是
/// 「这不对劲」的信号，不是正常值。
pub const MAX_FRAME_BYTES: usize = 256 * 1024;

/// 一场直播最多几路。够「前端/后端/测试/日志」，而不设限等于让一台机器
/// 把中转的内存吃光。
pub const MAX_LANES: usize = 4;

/// 中转上最多同时活着几场直播。
///
/// **`POST /live/start` 是 spec 里唯一要对公网开的路由，而它这一期没有
/// 配对身份可验**——谁都能建房间。没有这条上限的话，一个脚本几秒钟就能
/// 建出几十万间，每间 `MAX_LANES` 路、每路最大 `MAX_FRAME_BYTES`，中转
/// 的内存就是这么被吃光的。
///
/// 64 的来历：最坏情况 64 × 4 × 256 KB = 64 MB，一台小机器扛得住；而
/// 「同一台中转上同时有 64 位老师在直播」已经远超这个功能眼下的部署形态
/// （一所学校一台）。不够用的那天把这个数改大，别把这条上限拿掉。
pub const MAX_ROOMS: usize = 64;

/// 同一个来源在 [`START_RATE_WINDOW`] 里最多能建几场直播。
///
/// 房间总数有上限之后，剩下的攻击是「把上限占满」：反复建房间，让真正的
/// 老师开不了播。按来源节流拦的是这个——一位老师一节课开一次播，偶尔换
/// 几次链接，10 次一分钟绰绰有余。
pub const MAX_STARTS_PER_WINDOW: u32 = 10;

/// 建房限流的窗口长度。
pub const START_RATE_WINDOW: Duration = Duration::from_secs(60);

/// 推帧最快多久一次。终端不是视频，2 Hz 已经快过人读字。
pub const PUSH_INTERVAL: Duration = Duration::from_millis(500);

/// 画面没变也要推一次的间隔。中转按 `LIVE_TTL` 回收，不保活的话老师盯着
/// 屏幕想一分钟事情，直播自己就没了。
pub const KEEPALIVE: Duration = Duration::from_secs(20);

/// 多久没收到帧就把整条直播（连同 token）扔掉。
pub const LIVE_TTL: Duration = Duration::from_secs(60);

/// `?wait=1` 最多挂多久。短于常见反代的 30 秒空闲上限。
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(25);

/// live-id 的十六进制长度（4 字节随机数）。
pub const LIVE_ID_LEN: usize = 8;

/// 学生那把钥匙的十六进制长度（32 字节随机数）。
pub const LIVE_TOKEN_LEN: usize = 64;

/// 学生拉某一路画面的路径。
pub fn frame_path(id: &str, lane: usize) -> String {
    format!("{LIVE_PREFIX}/{id}/frame?lane={lane}")
}

/// 学生页本体的路径。
pub fn page_path(id: &str) -> String {
    format!("{LIVE_PREFIX}/{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 学生那一侧的路径由这里拼，两个 crate 都用它——各拼各的就是
    /// `/api/scroll` 打成 `/api/scrol` 那种 bug，而且只有真机上才看得见。
    #[test]
    fn the_frame_path_is_built_in_exactly_one_place() {
        assert_eq!(frame_path("7f3a2c91", 2), "/live/7f3a2c91/frame?lane=2");
        assert_eq!(page_path("7f3a2c91"), "/live/7f3a2c91");
    }

    /// 保活必须明显快过 TTL，否则老师盯着屏幕想事情的时候直播自己就没了。
    #[test]
    fn the_keepalive_beats_the_ttl_with_room_to_spare() {
        assert!(
            KEEPALIVE * 2 < LIVE_TTL,
            "保活 {KEEPALIVE:?} 对 TTL {LIVE_TTL:?} 来说太慢，丢一次就断播"
        );
        assert!(PUSH_INTERVAL <= KEEPALIVE);
    }
}

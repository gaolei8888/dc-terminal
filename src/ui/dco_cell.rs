//! 看板标题行右侧那一小格：本机 dco 现在的状态（只读）。
//!
//! **绘制路径上不发请求，也不阻塞主循环**：读取放在后台线程里（最多 `BUDGET` 一次），
//! 主循环每轮只做一次非阻塞的 `try_recv`；每 `POLL_EVERY` 才起一次读取，没有忙等。
//! 没装 dco（没有 `~/.dco/endpoint.json`）整格不画；连不上只在刚连得上之后一小会儿内画「未连接」。
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use dct_game::dco_status::{DcoState, DcoStatus};

use super::app::App;
use super::view::View;
use crate::i18n::{text, Key};

pub(crate) const POLL_EVERY: Duration = Duration::from_secs(3);
/// 刚连得上过多久之内，断了还显示「未连接」；更久就当它没装，整格隐藏。
const RECENT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct DcoPanel {
    rx: Option<Receiver<Option<DcoStatus>>>,
    last_fetch: Option<Instant>,
    pub(crate) shown: Option<DcoStatus>,
    pub(crate) last_up: Option<Instant>,
}

impl DcoPanel {
    /// 要画的那一格；`None` = 不画。
    pub(crate) fn cell(&self, now: Instant) -> Option<&DcoStatus> {
        let s = self.shown.as_ref()?;
        let recently_up = self.last_up.is_some_and(|t| now.saturating_duration_since(t) <= RECENT);
        (s.state != DcoState::Down || recently_up).then_some(s)
    }
}

pub(crate) fn label(state: DcoState, lang: crate::i18n::Lang) -> &'static str {
    text(
        match state {
            DcoState::Idle => Key::DcoIdle,
            DcoState::Running => Key::DcoRunning,
            DcoState::WaitingUser => Key::DcoWaiting,
            DcoState::Paused => Key::DcoPaused,
            DcoState::Halted => Key::DcoHalted,
            DcoState::PermissionMissing => Key::DcoPermission,
            DcoState::ScreenLocked => Key::DcoLocked,
            DcoState::Down => Key::DcoDown,
            DcoState::Unknown => Key::DcoUnknown,
        },
        lang,
    )
}

/// 主循环每轮调一次。
pub(crate) fn poll(app: &mut App, now: Instant) {
    poll_with(app, now, || {
        crate::sys::home().and_then(|h| dct_game::dco_status::probe(&h, dct_game::dco_status::BUDGET))
    });
}

pub(crate) fn poll_with(app: &mut App, now: Instant, fetch: impl FnOnce() -> Option<DcoStatus> + Send + 'static) {
    let p = &mut app.dco;
    if let Some(rx) = &p.rx {
        match rx.try_recv() {
            Ok(r) => {
                p.rx = None;
                if matches!(&r, Some(s) if s.state != DcoState::Down) {
                    p.last_up = Some(now);
                }
                p.shown = r;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => p.rx = None,
        }
    }
    let due = p.last_fetch.is_none_or(|t| now.saturating_duration_since(t) >= POLL_EVERY);
    if due && p.rx.is_none() && matches!(app.view, View::Board) {
        p.last_fetch = Some(now);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
        });
        p.rx = Some(rx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(state: DcoState) -> Option<DcoStatus> {
        Some(DcoStatus::bare(state))
    }

    fn wait_result(app: &mut App, now: Instant) {
        for _ in 0..200 {
            poll_with(app, now, || None);
            if app.dco.rx.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("后台读取一直没回来");
    }

    #[test]
    fn a_slow_read_never_blocks_the_loop_and_is_not_started_twice() {
        let (mut app, _d) = App::test_app();
        let t0 = Instant::now();
        let started = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        for _ in 0..3 {
            let s = started.clone();
            poll_with(&mut app, Instant::now(), move || {
                s.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_secs(2));
                st(DcoState::Idle)
            });
        }
        assert!(t0.elapsed() < Duration::from_millis(200), "poll 阻塞了主循环：{:?}", t0.elapsed());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 1, "上一条还没回就又起了一条");
        assert!(app.dco.cell(Instant::now()).is_none(), "还没回来就画了");
    }

    #[test]
    fn polls_only_every_few_seconds() {
        let (mut app, _d) = App::test_app();
        let t = Instant::now();
        wait_result(&mut app, t); // 第一次
        assert!(app.dco.last_fetch.is_some());
        let first = app.dco.last_fetch;
        poll_with(&mut app, t + Duration::from_secs(1), || st(DcoState::Idle));
        assert_eq!(app.dco.last_fetch, first, "没到点就又问了");
        poll_with(&mut app, t + POLL_EVERY, || None);
        assert_ne!(app.dco.last_fetch, first);
    }

    #[test]
    fn hidden_when_not_installed_shown_when_up_and_down_only_for_a_while() {
        let (mut app, _d) = App::test_app();
        let t = Instant::now();
        assert!(app.dco.cell(t).is_none());
        // 没装：结果是 None
        wait_result(&mut app, t);
        assert!(app.dco.cell(t).is_none());
        // 连得上
        app.dco.shown = st(DcoState::Running);
        app.dco.last_up = Some(t);
        assert_eq!(app.dco.cell(t).unwrap().state, DcoState::Running);
        // 刚断：还画「未连接」
        app.dco.shown = st(DcoState::Down);
        assert_eq!(app.dco.cell(t + Duration::from_secs(10)).unwrap().state, DcoState::Down);
        // 断了很久：不画
        assert!(app.dco.cell(t + RECENT + Duration::from_secs(1)).is_none());
        // 从来没连上过就是 down：不画
        app.dco.last_up = None;
        assert!(app.dco.cell(t).is_none());
    }

    #[test]
    fn polling_is_for_the_board_only() {
        let (mut app, _d) = App::test_app();
        app.view = View::Settings { state: ratatui::widgets::ListState::default(), sub: None };
        poll_with(&mut app, Instant::now(), || st(DcoState::Idle));
        assert!(app.dco.rx.is_none());
    }

    #[test]
    fn every_state_has_a_label_in_both_languages_without_han_in_english() {
        use crate::i18n::Lang;
        for s in [DcoState::Idle, DcoState::Running, DcoState::WaitingUser, DcoState::Paused, DcoState::Halted, DcoState::PermissionMissing, DcoState::ScreenLocked, DcoState::Down, DcoState::Unknown] {
            assert!(label(s, Lang::En).is_ascii(), "{s:?}");
            assert!(!label(s, Lang::Zh).is_empty());
        }
    }
}

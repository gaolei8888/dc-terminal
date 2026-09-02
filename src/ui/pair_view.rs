//! 配对三屏：Starting → Waiting → Done，或者在任何一步落进 Failed。
//!
//! 入口不在这个文件里——它在 `secret.rs`：`EnterSecret` 屏幕上，profile
//! 可配对（`Profile::pairable`，目前是 `"dc"`/`"qwen"` 两个内置 profile，
//! 见 `profile.rs::builtin_names` 和 `pair_apply.rs` 头上「往 dc/qwen 两把
//! 钥匙写」那段）时的 Ctrl+A。不占用一个字母键——`o` 早就留给了密钥输入
//! 本身（见 `secret.rs` 那条「Ctrl+O 不用 o」的注释），这里是同一条键位
//! 规矩的另一个例子。
//!
//! **URL 在本地拼，绝不接受线上答复里的 origin。** `daemon::pair_origin`
//! 只读这个 profile 自己的 `[api].base_url`，取它的 origin；界面这一侧
//! 独立调用同一个函数再算一遍，而不是信任 `PairStartedInfo`——事实上
//! 它连信任的余地都没有，那个类型压根没有 origin 字段（见它的文档注释）。
//! 这不是图省事的巧合：`/pair/start` 是无认证接口，一条从线上答复里拿到
//! 的 origin，会让任何打得到那个接口的人决定 dct 替学生打开的是哪个
//! 页面——一条能用的钓鱼路。origin 是信任锚，随 dct 自己发布，不来自网络。
//!
//! **这个文件里的 `handle_key` 也不写 `continue`。** 理由跟 `secret.rs`
//! 头上那条一模一样：它是从主循环的 `match app.view.clone()` 里调用的
//! 一个独立函数，不是内联在循环体里，`return` 本身是安全的，但如果哪天
//! 又被搬回循环体，这条约束要重新生效。

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};

use crate::i18n::{msg, text, Key, Lang};
use crate::proto::{socket_path, PairStartedInfo, PairTick, Request, Response, SecretPrompt};

use super::app::App;
use super::view::{is_plain_key, PairPhase, SecretPhase, View};
use super::widgets::Msg;
use super::{accent, danger, dim, open_url};

/// 配对用哪个 origin。**只读本地的 profile 文件**——见模块头注释。
/// 跟 `daemon::pair_origin` 共用同一个函数，不是各自抄一份：两边分别
/// 实现的话，某一天悄悄漂开，界面这一侧的独立校验就名存实亡了。
fn origin_for(app: &App, profile: &str) -> Option<String> {
    let dir = crate::profile::profiles_dir_for_socket(&app.socket);
    crate::daemon::pair_origin(&dir, profile)
}

/// 这次配对要不要顺手打开「报错时的 AI 解释」。**唯一的信号来源是本地
/// `[llm]` 那一段写没写**——`config.rs` 头上那段说得很清楚：用户显式写下
/// `[llm]`，哪怕一个字段都不填，就是那个隐私边界上唯一一次人的点头。
/// 配对屏不另开一屏重新问一遍：那份点头（或者没有点头）已经躺在磁盘上，
/// 这里只是如实带上，交给 `Request::PairStart`——真正落盘发生在 daemon
/// 的后台线程里（见 `proto::Request::PairStart` 的文档注释），界面这一刻
/// 早就不在那条调用栈上了，所以这个 bool 必须现在就问清楚、跟着请求走。
pub(crate) fn opt_in_llm(app: &App) -> bool {
    let path = crate::config::config_path_for_socket(&app.socket);
    crate::config::Config::load(&path).llm.is_some()
}

/// 起一条配对：真网络（daemon 转发到网关的 `/pair/start`），**必须丢给
/// 后台线程**——同 `secret.rs`/`phone.rs` 里「Enter 提交」的道理，不能堵
/// 在按键循环里，等待的这几秒会话视图也要继续刷新。
pub(crate) fn start_pairing(app: &mut App, profile: String) {
    let opt_in = opt_in_llm(app);
    let (tx, rx) = std::sync::mpsc::channel();
    let sock = socket_path();
    // 发起时的身份留一份在这——线程闭包要吃掉 `profile` 本体去发请求，
    // 视图和送回来的结果都得靠这份拷贝对上号（同 `verify_rx` 的道理）。
    let view_profile = profile.clone();
    let stamped = profile.clone();
    std::thread::spawn(move || {
        let outcome = crate::client::Client::connect(&sock)
            .and_then(|mut c| {
                c.call(Request::PairStart {
                    profile,
                    opt_in_llm: opt_in,
                })
            })
            .map(|r| match r {
                Response::PairStarted(res) => res,
                _ => Err("unreachable".to_string()),
            })
            .unwrap_or_else(|e| Err(e.to_string()));
        let _ = tx.send((stamped, outcome));
    });
    app.pair_start_rx = Some(rx);
    app.view = View::Pair {
        profile: view_profile,
        phase: PairPhase::Starting,
    };
}

/// `PairStart` 的结果回来了（`app.pair_start_rx` 收的那一半，`ui/mod.rs`
/// 主循环调用），把它变成下一屏。
pub(crate) fn apply_started(
    app: &mut App,
    profile: String,
    outcome: Result<PairStartedInfo, String>,
) -> View {
    match outcome {
        Ok(info) => {
            // origin 拿不到——profile 文件在两次读之间被人手改坏了，或者
            // 磁盘上根本没有这个 profile 了（daemon 在 `PairStart` 成功
            // 之前已经用同一个函数验证过一次，见 `daemon::pair_origin`
            // 头上的注释，所以这一刻的失败发生在那之后）。**不能退化成
            // 空 origin 硬凑一个 URL**：`open_url` 打不开一个语法都不完整
            // 的地址，而屏幕上还会印出这句打不开的地址让学生手抄——手抄
            // 也打不开，比不给地址更坏。落一个不可重试的 `Failed`：
            // 再按 `r` 只会撞上同一个读不出来的文件，`retryable: false`
            // 如实说明这一点。
            let Some(origin) = origin_for(app, &profile) else {
                return View::Pair {
                    profile,
                    phase: PairPhase::Failed {
                        message: text(Key::PairProfileUnreadable, app.lang).to_string(),
                        retryable: false,
                    },
                };
            };
            let url = format!("{origin}{}?code={}", info.verify_path, info.user_code);
            // MINOR 8 同款的顾虑（见 `secret.rs` Ctrl+O 分支）：`open_url`
            // 打不开的话必须说一声，不能让这一屏看着有个地址、其实浏览器
            // 根本没弹出来，用户会以为是自己眼花。
            if !open_url(&url) {
                app.message = Msg::err(msg::cannot_open_browser(app.lang, &url));
            }
            View::Pair {
                profile,
                phase: PairPhase::Waiting {
                    user_code: info.user_code,
                    url,
                    deadline: std::time::Instant::now()
                        + std::time::Duration::from_secs(info.expires_in),
                },
            }
        }
        Err(reason) => View::Pair {
            profile,
            phase: PairPhase::Failed {
                message: fail_message(&reason, app.lang),
                retryable: true,
            },
        },
    }
}

/// 一次 `PairPoll` 的结果（`ui/mod.rs` 主循环里节流调用）变成下一屏。
/// `current` 是眼下这一屏的 `PairPhase`——`PairTick::Waiting` 时原样
/// 留着（`user_code`/`url`/`deadline` 都不该被一次「还没好」的轮询抹掉）。
pub(crate) fn apply_tick(lang: Lang, profile: String, current: PairPhase, tick: PairTick) -> View {
    match tick {
        PairTick::Waiting => View::Pair {
            profile,
            phase: current,
        },
        PairTick::Done {
            anthropic_ready,
            openai_ready,
        } => View::Pair {
            profile,
            phase: PairPhase::Done {
                anthropic: anthropic_ready,
                openai: openai_ready,
            },
        },
        // 两种过期不能共享一句话——见 `PairPhase::Failed` 的文档注释和
        // 下面 `the_two_expiries_do_not_share_one_sentence`。`retryable`
        // 的那一种网关给的 `message` 是空串（`pair.rs::Machine::step` 的
        // ttl 分支），dct 自己给一句「按 r 换一个」；不可重试的那一种
        // 原样带着网关给的那句话，绝不许换成 dct 自己那句「过期了」——
        // 学生会按 r 按到天荒地老，每一次都走到同一个地方。
        PairTick::Expired { retryable, message } => View::Pair {
            profile,
            phase: PairPhase::Failed {
                message: if retryable {
                    text(Key::PairCodeExpired, lang).to_string()
                } else {
                    message
                },
                retryable,
            },
        },
        PairTick::Failed(reason) => View::Pair {
            profile,
            phase: PairPhase::Failed {
                message: fail_message(&reason, lang),
                retryable: true,
            },
        },
    }
}

/// 没有专门词条的失败原因码，套进一句人话。**原样带上原因码**——报码
/// 不组句是 daemon 那边的规矩（同 `proto::ErrorCode`），这里不替一个
/// 没见过的原因编一个更具体的说法：编错了比说不清更容易把人导向错误
/// 的下一步（`pair_apply.rs` 头上那条「宁可什么都不写」是同一种谨慎）。
fn fail_message(reason: &str, lang: Lang) -> String {
    match reason {
        "not_enabled" => text(Key::PairNotEnabled, lang).to_string(),
        "denied" => text(Key::PairDenied, lang).to_string(),
        // `pair.rs::Machine::step` 里空钥匙那一支：网关批了，钥匙却是空的。
        "empty_key" => text(Key::PairKeyUnreadable, lang).to_string(),
        other => msg::pair_failed(lang, other),
    }
}

/// **这个函数里永远不要 `continue`。** 理由见模块头注释——它是从主循环的
/// `match app.view.clone()` 里调用的独立函数，`return` 目前是安全的。
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let View::Pair { profile, phase } = app.view.clone() else {
        return Ok(());
    };
    // Esc 在任何阶段都是真取消：发 `Request::PairCancel`，不能只切视图。
    // 不发的话，用户退出去了，后台还在替他领钥匙，领到了写进 secrets，
    // 而他以为自己取消了——`proto::Request::PairCancel` 头上的注释说的
    // 就是这件事，`esc_sends_a_cancel_and_leaves_the_view` 钉着它。
    //
    // 哪怕还在 `Starting`（请求可能还没飞回来）也要发：daemon 一收到
    // `PairStart` 就已经在内存里起了轮询线程（见 `daemon.rs::handle`），
    // `PairCancel` 到晚了也比不发强——`pairs` 表里还没有这个 profile 时，
    // 它本来就是个空操作（`daemon.rs` 的 `PairCancel` 分支）。
    if key.code == KeyCode::Esc {
        app.pair_start_rx = None;
        let _ = app.client().and_then(|c| {
            c.call(Request::PairCancel {
                profile: profile.clone(),
            })
        });
        app.view = super::home_view(app);
        return Ok(());
    }
    // p：手动填一直在，见模块头注释——老用户、离线课堂、网关配对坏掉的
    // 那天都要它。四个阶段都认：哪怕请求还飞着（`Starting`），用户也该
    // 能随时改主意换成手填，不用先等一个不知道多久的网络往返。
    //
    // **`p` 跟 `Esc` 一样是真取消，不能只切视图。** 上面那段说的危险在这
    // 条路上更具体：学生按 `p`、自己粘一把钥匙进 `secrets.toml`，几分钟后
    // 顺手把还开着的那个浏览器页面点了确认——后台那条还在跑的轮询线程
    // 于是领到钥匙，把两个 profile 和 `pair-models.toml` 全部覆盖掉，
    // 发生在他离开那块屏幕之后，而屏幕上不会有任何东西说这件事。
    // 学生自己填的那把钥匙就这么被换掉了，他无从知道。
    if key.code == KeyCode::Char('p') && is_plain_key(&key) {
        app.pair_start_rx = None;
        let _ = app.client().and_then(|c| {
            c.call(Request::PairCancel {
                profile: profile.clone(),
            })
        });
        app.view = manual_entry_view(app, &profile);
        return Ok(());
    }
    match phase {
        PairPhase::Starting | PairPhase::Done { .. } => {}
        PairPhase::Waiting { url, .. } => {
            if key.code == KeyCode::Char('o') && is_plain_key(&key) && !open_url(&url) {
                app.message = Msg::err(msg::cannot_open_browser(app.lang, &url));
            }
        }
        PairPhase::Failed { retryable, .. } => {
            if retryable && key.code == KeyCode::Char('r') && is_plain_key(&key) {
                start_pairing(app, profile);
            }
        }
    }
    Ok(())
}

/// `p`：切到手动填密钥（`View::EnterSecret`，`secret.rs` 那一套）。**尽量
/// 拿到真提示**（申领页链接、人话提示），但拿不到也不能把这条退路堵死——
/// 见模块头「手动填一直在」那句：daemon 连不上、这个 profile 没声明密钥
/// 提示，都不该让 `p` 变成一个按下去没反应的键。
fn manual_entry_view(app: &mut App, profile: &str) -> View {
    let lang = app.lang;
    let found = app
        .client()
        .and_then(|c| c.call(Request::Profiles { lang }))
        .ok()
        .and_then(|r| match r {
            Response::Profiles { entries, .. } => entries.into_iter().find(|e| e.name == profile),
            _ => None,
        });
    let (label, prompt, pairable) = match found {
        Some(e) => (
            e.label,
            e.secret.unwrap_or(SecretPrompt {
                hint: String::new(),
                url: None,
            }),
            e.pairable,
        ),
        // 找不回这条 profile（daemon 连不上/文件被删）时兜个 `true`，不是
        // `false`：能走到这个函数，说明用户此刻正站在 `View::Pair` 里，
        // 而只有 `pairable` 的 profile 才起得了 `View::Pair`（见
        // `pick.rs`/`secret.rs` 里 `start_pairing` 的调用点）——`false`
        // 会在 Ctrl+A 上悄悄关掉一条本该还开着的退路。
        None => (
            profile.to_string(),
            SecretPrompt {
                hint: String::new(),
                url: None,
            },
            true,
        ),
    };
    View::EnterSecret {
        profile: profile.to_string(),
        label,
        prompt,
        buf: String::new(),
        phase: SecretPhase::Typing,
        // 从配对屏改道过来的意图是「先把密钥填上」，不是「回设置页」——
        // 跟从选择器进来的 `AskSecret` 走的是同一条约定（见 `pick.rs`）。
        return_to_settings: false,
        pairable,
    }
}

/// 一个阶段的核心一句话——draw() 的标题行，也是
/// `the_two_expiries_do_not_share_one_sentence` 直接比对的对象。
pub(crate) fn phase_line(phase: &PairPhase, lang: Lang) -> String {
    match phase {
        PairPhase::Starting => text(Key::PairContacting, lang).to_string(),
        PairPhase::Waiting { .. } => text(Key::PairEnterCodeInBrowser, lang).to_string(),
        PairPhase::Failed { message, .. } => message.clone(),
        PairPhase::Done { anthropic, openai } => {
            if *anthropic && *openai {
                text(Key::PairDoneBoth, lang).to_string()
            } else if *openai {
                // 免费账号：只有 Qwen 那一路。必须点名，也要点名 Claude
                // 需要付费升级——不能让学生对着一个用不了的 Claude 猜
                // 为什么（见 `PairPhase::Done` 的文档注释）。
                text(Key::PairDoneQwenOnly, lang).to_string()
            } else {
                // 两条路都没开——网关批了配对却一条能用的路都没给，
                // 这不是学生做错了什么，跟空钥匙一样是网关那侧的问题，
                // 用同一句「读不出来」的话，不替它编一个更具体的说法。
                text(Key::PairKeyUnreadable, lang).to_string()
            }
        }
    }
}

pub(crate) fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let View::Pair { phase, .. } = app.view.clone() else {
        return;
    };
    let border_style = if app.connected {
        Style::default()
    } else {
        danger()
    };
    let body = super::widgets::header(f, area, text(Key::AutoPair, app.lang), border_style);

    let mut lines: Vec<Line> = Vec::new();
    let headline_style = match &phase {
        PairPhase::Failed { .. } => danger(),
        PairPhase::Starting => accent(),
        PairPhase::Waiting { .. } | PairPhase::Done { .. } => accent(),
    };
    lines.push(Line::from(Span::styled(
        phase_line(&phase, app.lang),
        headline_style,
    )));

    if let PairPhase::Waiting {
        user_code,
        url,
        deadline,
    } = &phase
    {
        lines.push(Line::from(""));
        // 大字印在屏幕上，学生照着念或照着敲——见 `PairPhase::Waiting`
        // 的文档注释。
        lines.push(Line::from(Span::styled(
            user_code.clone(),
            accent().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        // 地址本身也要印出来，不止是拿去开浏览器——见模块头注释和
        // `the_url_is_on_screen_not_only_in_the_browser`。
        lines.push(Line::from(Span::styled(url.clone(), dim())));
        lines.push(Line::from(""));
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        lines.push(Line::from(Span::styled(
            msg::pair_countdown(app.lang, remaining),
            dim(),
        )));
    }

    // 免费账号那句要点名「Qwen 那一路」+「Claude 需要付费升级」，两句都
    // 已经在 `PairDoneQwenOnly` 里说了；这里只在 Done 阶段额外补一行
    // 「报错时的 AI 解释」——只在勾了的时候写，没勾就整行不出现（沉默
    // 本身就是「没开」，不用另开一句「未开启」去提醒一件用户自己决定
    // 没要的事）。
    if matches!(phase, PairPhase::Done { .. }) && opt_in_llm(app) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            text(Key::PairLlmOptIn, app.lang),
            dim(),
        )));
    }

    f.render_widget(
        Paragraph::new(lines)
            // 地址、失败原因都可能比窄终端还长，不折行会被裁掉半句关键信息。
            .wrap(Wrap { trim: false }),
        body,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn phase_waiting() -> PairPhase {
        PairPhase::Waiting {
            user_code: "HJ4K-9QTZ".into(),
            url: "https://dc-llm.tzspace.cn/pair?code=HJ4K-9QTZ".into(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(900),
        }
    }

    fn phase_expired(retryable: bool, message: String) -> PairPhase {
        PairPhase::Failed { message, retryable }
    }

    // 带着 `TempDir` guard 一起交出去——同 `App::test_app` 文档注释里的
    // 提醒：不接住它会在函数返回时被立刻删掉，而 `draw()` 里的
    // `opt_in_llm` 真的会去读 `app.socket` 派生出来的那个配置文件路径
    // （文件不存在时 `Config::load` 退化成默认值，不会报错，但目录本身
    // 要还在）。
    fn test_app_with_phase(phase: PairPhase) -> (App, tempfile::TempDir) {
        let (mut app, dir) = App::test_app();
        app.view = View::Pair {
            profile: "dc".into(),
            phase,
        };
        (app, dir)
    }

    fn test_app_in_pair_waiting() -> (App, tempfile::TempDir) {
        test_app_with_phase(phase_waiting())
    }

    fn render_lines(app: &mut App) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, f.area(), app)).unwrap();
        let buf = term.backend().buffer();
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol()))
                    .collect::<String>()
            })
            .collect()
    }

    fn render_phase(phase: &PairPhase) -> String {
        phase_line(phase, Lang::Zh)
    }

    /// **MINOR (round 1 review).** 一个语法都不完整的 URL（空 origin +
    /// `verify_path`）比不给地址更坏：`open_url` 打不开，屏幕上还印着
    /// 一句手抄也没用的话。origin 拿不到时必须落一个不可重试的
    /// `Failed`，而不是把这次已经成功的起步硬凑成一屏能看却打不开的
    /// `Waiting`。用一个磁盘上压根不存在的 profile 名字触发
    /// `origin_for` 返回 `None`——真正的 `"dc"` 是内置 profile，
    /// `all_profiles` 找得到它，这里要的是"找不到"的那一分支。
    #[test]
    fn a_profile_that_cannot_be_read_back_fails_instead_of_composing_a_broken_url() {
        let (mut app, _dir) = App::test_app();
        let outcome = Ok(PairStartedInfo {
            user_code: "HJ4K-9QTZ".into(),
            verify_path: "/pair".into(),
            expires_in: 900,
        });
        let view = apply_started(&mut app, "no-such-profile".into(), outcome);
        match view {
            View::Pair {
                phase: PairPhase::Failed { retryable, message },
                ..
            } => {
                assert!(!retryable, "读不出配置不是过期，按 r 换不来别的结果");
                assert!(!message.is_empty());
            }
            _ => panic!("origin 拿不到时该落一个不可重试的 Failed"),
        }
    }

    /// **Esc 要能真的取消。** 不发 `PairCancel` 的话，用户退出去了，
    /// 后台还在替他领钥匙，领到了写进 secrets，而他以为自己取消了——
    /// `phone.rs::verifying_ignores_everything_but_escape` 是同一条属性
    /// 在手机通知验证上的版本。
    ///
    /// **一个断连的 `App::test_app()` 证明不了这条属性**——`app.client()`
    /// 在那种 App 上永远是 `Err`，一个只切视图、压根不碰 `client()` 的
    /// 版本跟真的调用了它但连不上，从外面看一模一样。这里起一个真的
    /// （只在本机、只认这一条协议的）假守护进程，让它把收到的每条请求
    /// 都记下来，再断言 `PairCancel` 真的落到了那张记录上——不是靠
    /// 读代码相信它发了。
    #[test]
    fn esc_sends_a_cancel_and_leaves_the_view() {
        let (sock, _fake_dir, received) = fake_daemon();
        let (mut app, _dir) = App::test_app();
        app.client = Some(crate::client::Client::connect(&sock).unwrap());
        app.view = View::Pair {
            profile: "dc".into(),
            phase: phase_waiting(),
        };

        handle_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(
            !matches!(app.view, View::Pair { .. }),
            "Esc 要真的离开配对屏"
        );
        assert!(
            received
                .lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Request::PairCancel { profile } if profile == "dc")),
            "Esc 必须真的发出 PairCancel，不能只是切视图"
        );
    }

    /// 起一个只认协议帧（一行 JSON 请求，一行 JSON `Response::Ok` 回答）
    /// 的假守护进程，本机 Unix socket，不碰真正的网络。返回收到的每一条
    /// `Request`——测试用它断言"某个请求真的被发出去了"，而不是只信任
    /// 代码读起来像是发了。
    fn fake_daemon() -> (
        std::path::PathBuf,
        tempfile::TempDir,
        std::sync::Arc<std::sync::Mutex<Vec<Request>>>,
    ) {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("s.sock");
        // `sys::ipc::bind_private` 而不是 `std::os::unix::net::UnixListener`：
        // 那一行只在 Unix 上存在，而这条测试本身没有 `#[cfg(unix)]`——整个
        // 测试二进制于是在 Windows 上编译不过，`cargo test` 在那个平台上
        // 连跑都跑不起来。Windows 的学生正是「零 C 依赖」那条规矩存在的
        // 理由，不能让测试套件把这个平台漏在外面。`sys::ipc` 那一层的全部
        // 用处就是把这个差异收在一个地方（Windows 走 `uds_windows`，形状
        // 一模一样），生产代码早就只认它了，测试没有理由绕过去。
        let listener = crate::sys::ipc::bind_private(&sock).unwrap();
        let received: std::sync::Arc<std::sync::Mutex<Vec<Request>>> = Default::default();
        let recv2 = received.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut writer = stream;
                loop {
                    let mut line = String::new();
                    let n = reader.read_line(&mut line).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    let Ok(req) = serde_json::from_str::<Request>(&line) else {
                        break;
                    };
                    recv2.lock().unwrap().push(req);
                    let resp = serde_json::to_string(&Response::Ok).unwrap();
                    if writeln!(writer, "{resp}").is_err() {
                        break;
                    }
                }
            }
        });
        (sock, dir, received)
    }

    /// **`p` 也要真的取消。** 形状照抄 `esc_sends_a_cancel_and_leaves_the_view`，
    /// 理由更具体：不发 `PairCancel` 的话，学生按 `p` 自己粘了一把钥匙，
    /// 几分钟后顺手确认了那个还开着的浏览器页面，后台那条没人停的轮询
    /// 线程就会把他刚填的钥匙连同两个 profile 的模型名一起覆盖掉——
    /// 发生在他早已离开这块屏幕之后，屏上一个字都不会说。
    ///
    /// 同那条测试：一个断连的 `App::test_app()` 证明不了这件事，必须对着
    /// 一个真的（本机、只认这一条协议的）假守护进程断言请求真的落到了
    /// 它的记录上。
    #[test]
    fn p_sends_a_cancel_too_before_switching_to_manual_entry() {
        let (sock, _fake_dir, received) = fake_daemon();
        let (mut app, _dir) = App::test_app();
        app.client = Some(crate::client::Client::connect(&sock).unwrap());
        app.view = View::Pair {
            profile: "dc".into(),
            phase: phase_waiting(),
        };

        handle_key(&mut app, key(KeyCode::Char('p'))).unwrap();

        assert!(
            matches!(app.view, View::EnterSecret { .. }),
            "p 应该进手动填"
        );
        assert!(
            received
                .lock()
                .unwrap()
                .iter()
                .any(|r| matches!(r, Request::PairCancel { profile } if profile == "dc")),
            "p 必须真的发出 PairCancel，不能把轮询线程留在后台替学生领钥匙"
        );
    }

    /// 浏览器打不开是常态（SSH、WSL、没设默认浏览器），屏上必须有个能
    /// 手抄的地址，否则学生就卡死在这一屏。
    #[test]
    fn the_url_is_on_screen_not_only_in_the_browser() {
        let (mut app, _dir) = test_app_in_pair_waiting();
        let lines = render_lines(&mut app);
        assert!(
            lines.iter().any(|l| l.contains("dc-llm.tzspace.cn/pair")),
            "地址要印出来：{lines:?}"
        );
    }

    /// 过期的两种理由文案不一样：能重试的说按 r，不能的说去用网关自己
    /// 给的那句话（比如去重新生成）。
    #[test]
    fn the_two_expiries_do_not_share_one_sentence() {
        let a = phase_expired(true, String::new());
        let b = phase_expired(false, "请点「重新生成」".into());
        assert_ne!(render_phase(&a), render_phase(&b));
        assert!(render_phase(&b).contains("重新生成"));
    }

    /// 手动填那条退路一直在。老用户、离线课堂、网关配对坏掉的那天都
    /// 要它——四个阶段（包括 `Starting`）都要能按 `p` 到手动填。
    #[test]
    fn manual_entry_stays_reachable_from_every_phase() {
        for phase in [
            PairPhase::Starting,
            phase_waiting(),
            phase_expired(true, String::new()),
            PairPhase::Done {
                anthropic: true,
                openai: true,
            },
        ] {
            let (mut app, _dir) = test_app_with_phase(phase);
            handle_key(&mut app, key(KeyCode::Char('p'))).unwrap();
            assert!(
                matches!(app.view, View::EnterSecret { .. }),
                "p 应该进手动填"
            );
        }
    }

    /// 免费账号的成功屏必须点名「Qwen」，也要点名「Claude 需要付费升级」——
    /// 不能让学生对着一个用不了的 Claude 猜为什么。
    #[test]
    fn a_free_account_is_told_plainly_it_is_on_the_qwen_path() {
        let line = phase_line(
            &PairPhase::Done {
                anthropic: false,
                openai: true,
            },
            Lang::Zh,
        );
        assert!(line.contains("Qwen"), "{line}");
        assert!(line.contains("付费") || line.contains("升级"), "{line}");
    }

    /// `apply_tick` 是真正把 `PairTick::Expired` 变成一句话的地方：
    /// 可重试的一支（网关给的 `message` 是空串，见 `pair.rs` 的 ttl 分支）
    /// 必须由 dct 自己补一句「按 r 换一个」，不能原样把空串糊到屏幕上；
    /// 不可重试的一支要原样带着网关给的那句话。两句必须不同——同
    /// `the_two_expiries_do_not_share_one_sentence` 是同一条属性，
    /// 只是这里从 `PairTick` 起步，钉住的是 `apply_tick` 本身而不是
    /// 手搭的 `PairPhase`。
    #[test]
    fn apply_tick_fills_in_the_retryable_expiry_and_keeps_the_gateways_own_words() {
        let retryable = apply_tick(
            Lang::Zh,
            "dc".into(),
            PairPhase::Starting,
            PairTick::Expired {
                retryable: true,
                message: String::new(),
            },
        );
        let View::Pair {
            phase:
                PairPhase::Failed {
                    message: a,
                    retryable: true,
                },
            ..
        } = retryable
        else {
            panic!("该是可重试的 Failed");
        };
        assert!(!a.is_empty(), "空串不能原样糊到屏幕上");

        let not_retryable = apply_tick(
            Lang::Zh,
            "dc".into(),
            PairPhase::Starting,
            PairTick::Expired {
                retryable: false,
                message: "请点「重新生成」".into(),
            },
        );
        let View::Pair {
            phase:
                PairPhase::Failed {
                    message: b,
                    retryable: false,
                },
            ..
        } = not_retryable
        else {
            panic!("该是不可重试的 Failed");
        };
        assert!(b.contains("重新生成"), "{b}");
        assert_ne!(a, b);
    }

    /// 两种语言都要覆盖——词条表本身已经被 `i18n` 的守卫钉住了非空和
    /// 不许夹汉字，这里额外确认配对屏真的会调用它们，不是写了词条却
    /// 没接上任何一处渲染。
    #[test]
    fn both_languages_produce_non_empty_phase_lines() {
        // `phase_expired(true, "")` 不进这份清单：那份空 `message` 只是
        // `PairPhase::Failed` 这个类型允许的一个值，真正走到屏幕上的
        // 那一份永远经过 `apply_tick`——`retryable` 的一支会现填一句
        // `PairCodeExpired`，见 `apply_tick` 的实现。这里测的是
        // `phase_line` 本身对四种真实可达状态给不给得出话。
        for lang in Lang::all() {
            for phase in [
                PairPhase::Starting,
                phase_waiting(),
                phase_expired(false, "网关的原话".into()),
                PairPhase::Done {
                    anthropic: true,
                    openai: true,
                },
            ] {
                assert!(!phase_line(&phase, *lang).is_empty());
            }
        }
    }
}

//! Task 7：整条配对流程打一个假网关，不碰真实网络。
//!
//! 两条测试各自钉住 spec 里最容易悄悄坏掉的一半：
//! - 一条成功的配对，必须真的经过一次 `pending`——一次就批准的测试永远
//!   测不到「等」这个状态，而学生大部分时间正待在这个状态里。
//! - 取消之后哪怕网关随后批准，也不许有一个字节落盘——这是唯一能抓住
//!   「后台轮询线程活得比学生关掉的那块屏幕还久」这种 bug 的办法。

use dct::proto::{PairTick, Request, Response};
use std::time::Duration;

mod common;

/// 完整一条：start → pending → approved。断言落盘的三处都对，外加
/// `opt_in_llm: true` 时 `config.toml` 也真的长出一段 `Config::load`
/// 认得的 `[llm]`。
#[test]
fn a_full_pairing_writes_both_secrets_and_both_model_names() {
    let gw = common::fake_gateway(vec![
        // 第一次 poll 还没批准，第二次批准——真实节奏就是这样，一次就成的
        // 测试测不到「等」这件事。
        r#"{"status":"pending"}"#,
        // 这是网关真实回过的形状：免费账号 anthropic 那一组是空的，只有
        // openai（qwen 方言）那一组有模型名。
        r#"{"status":"approved","api_key":"sk-live-0123456789abcdef0123456789abcdef01234567",
            "models":{"anthropic":{},"openai":{"default":"qwen3.5:35b","small_fast":"gemma4:31b"}},
            "platforms":{"qwen3.5:35b":"local"}}"#,
    ]);
    let home = tempfile::tempdir().unwrap();
    let d = common::daemon_with(home.path(), &gw.origin());

    match d.call(Request::PairStart {
        profile: "dc".into(),
        opt_in_llm: true,
    }) {
        Response::PairStarted(Ok(_)) => {}
        other => panic!("配对没能起步：{other:?}"),
    }

    let tick = common::wait_for_tick(&d, "dc", true, Duration::from_secs(10));
    assert!(
        matches!(
            tick,
            PairTick::Done {
                anthropic_ready: false,
                openai_ready: true,
            }
        ),
        "免费账号只有 openai 那一路：{tick:?}"
    );

    // 第一处：secrets.toml 里 dc 和 qwen 都拿到了同一把钥匙。
    let secrets = std::fs::read_to_string(home.path().join("secrets.toml")).unwrap();
    assert!(secrets.contains("dc"), "{secrets}");
    assert!(secrets.contains("qwen"), "{secrets}");

    // 第二处：pair-models.toml 里有网关给的两个模型名。
    let models = std::fs::read_to_string(home.path().join("pair-models.toml")).unwrap();
    assert!(models.contains("qwen3.5:35b"), "{models}");
    assert!(models.contains("gemma4:31b"), "{models}");
    assert!(
        !models.contains("ANTHROPIC_MODEL"),
        "免费账号没有 anthropic 那一路，不该编一个模型名出来：{models}"
    );

    // 第三处：勾了 opt_in_llm，config.toml 长出一段 Config::load 认得的
    // [llm]——不是「文件里出现了字符串」，是「解析器真的把它读回来了」。
    let cfg_raw = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
    let cfg = dct::config::Config::from_toml(&cfg_raw).unwrap();
    let llm = cfg.llm.expect("勾了 opt_in_llm，[llm] 应该被写出来");
    assert_eq!(llm.provider, "qwen", "免费账号只有 openai 那一路能用来自举");
    assert_eq!(llm.model.as_deref(), Some("qwen3.5:35b"));
}

/// 取消之后，哪怕网关随后批准了，也一个字节都不许落盘。
///
/// 这是唯一能抓住「轮询线程活得比学生关掉的那块屏幕还久」这种 bug 的
/// 办法。**取消必须发生在一次轮询真的在途的时候**：假网关接到 `/pair/poll`
/// 之后卡住不回，等测试确认这个请求已经堵住了再发 `PairCancel`——这样
/// 取消才落在 `pair_poll_once` 里两道「再查一次取消」的门守着的那段窗口
/// 里，而不是抢在第一次轮询发出之前就把表项删掉（那样两道门永远不会被
/// 走到，删掉它们测试也照样绿）。
#[test]
fn cancelling_means_nothing_is_ever_written() {
    let gw = common::fake_gateway_gated();
    let home = tempfile::tempdir().unwrap();
    let d = common::daemon_with(home.path(), &gw.origin());

    match d.call(Request::PairStart {
        profile: "dc".into(),
        opt_in_llm: true,
    }) {
        Response::PairStarted(Ok(_)) => {}
        other => panic!("配对没能起步：{other:?}"),
    }

    // 等到网关确认已经真的收到一次轮询、正堵在那儿——不靠猜时间。
    gw.wait_for_poll_inflight(Duration::from_secs(10));

    match d.call(Request::PairCancel {
        profile: "dc".into(),
    }) {
        Response::Ok => {}
        other => panic!("取消应该总是 Ok：{other:?}"),
    }

    // 放行那个卡住的轮询，让它收到 approved——如果两道门没守住，
    // 轮询线程接下来就会把这把钥匙落盘。
    gw.approve();

    // 给后台线程留足够的时间——万一它没被真的停掉，这段时间够它把
    // apply 跑完、把钥匙写下去。
    std::thread::sleep(Duration::from_secs(1));

    assert!(
        !home.path().join("secrets.toml").exists()
            || !std::fs::read_to_string(home.path().join("secrets.toml"))
                .unwrap()
                .contains("sk-live-should-never-land-on-disk"),
        "取消之后落盘了，说明后台线程没停"
    );
    assert!(
        !home.path().join("pair-models.toml").exists(),
        "取消之后模型名不该落盘"
    );
    assert!(
        !home.path().join("config.toml").exists(),
        "取消之后 [llm] 更不该被自举出来"
    );
}

/// **勾选框取消掉之后，`[llm]` 一个字都不许被写出来。**
///
/// 这条测试钉的是那条链子的中段。`PairStart` 是在学生看见那行文案之前
/// 就发出去的（那一屏要先等网关回一串码），所以他按 `l` 取消勾选时，
/// 唯一还能把新答案送到 daemon 的东西是每一轮的 `PairPoll`。中段一旦断了
/// ——`PairPoll` 不带这个字段，或者 daemon 收下了却不用它——屏幕上那个框
/// 照样能勾能取消，磁盘上却永远按起步那一刻的值来。那种坏法在界面上
/// 完全看不出来，只有这里能抓住。
///
/// 用卡住的假网关是为了让取消勾选**落在批准之前**：轮询已经发出去、
/// 正堵在网络上，这时候界面捎来一个 `false`，随后网关才批准。
#[test]
fn unticking_the_box_before_approval_keeps_llm_out_of_the_config() {
    let gw = common::fake_gateway_gated();
    let home = tempfile::tempdir().unwrap();
    let d = common::daemon_with(home.path(), &gw.origin());

    // 起步时框还勾着——那是默认值，学生还没看见那一屏。
    match d.call(Request::PairStart {
        profile: "dc".into(),
        opt_in_llm: true,
    }) {
        Response::PairStarted(Ok(_)) => {}
        other => panic!("配对没能起步：{other:?}"),
    }

    gw.wait_for_poll_inflight(Duration::from_secs(10));

    // 学生读完那行「会把终端上的报错原文发给训练营网关」，按了 `l`。
    // 界面下一轮的 `PairPoll` 就是这条。
    match d.call(Request::PairPoll {
        profile: "dc".into(),
        opt_in_llm: false,
    }) {
        Response::PairTick(_) => {}
        other => panic!("轮询该照常回一个 tick：{other:?}"),
    }

    gw.approve();
    let tick = common::wait_for_tick(&d, "dc", false, Duration::from_secs(10));
    assert!(
        matches!(tick, PairTick::Done { .. }),
        "配对本身要照常成功：{tick:?}"
    );

    // 钥匙照写——取消的是「AI 解释」这一件事，不是整条配对。
    let secrets = std::fs::read_to_string(home.path().join("secrets.toml")).unwrap();
    assert!(secrets.contains("dc"), "{secrets}");

    // 而 `[llm]` 一个字都不该有。
    match std::fs::read_to_string(home.path().join("config.toml")) {
        Err(_) => {}
        Ok(cfg) => assert!(
            !cfg.contains("[llm]"),
            "学生取消了勾选，配对却还是把 [llm] 写了出来：{cfg}"
        ),
    }
}

/// **取消勾选是粘的：这一次配对之内，`false` 之后再来的 `true` 不算数。**
///
/// 两条时钟对不齐。界面每 500ms 把勾选框的当前值捎在 `PairPoll` 上，
/// 轮询线程每 250ms 醒一次、网关一回 `approved` 就立刻落盘——落盘那一刻
/// 读到的值不保证是学生屏幕上的那个。两个方向都会抢输，但只有一个方向
/// 不能接受：默认勾着的那个抢输，学生停在他看见过也没反对过的值上；
/// 而取消勾选抢输，是他当面读完「会把终端上的报错原文发给训练营网关」
/// 之后明确拒绝了，然后这个拒绝输给了一次网络往返。
///
/// 所以 daemon 那一格只允许 true→false。这条测试走真守护进程、真协议：
/// 先捎一个 `false`（学生按了 `l`），再捎一个 `true`（他又按了一下，
/// 或者只是界面上一轮还没来得及更新的旧值），然后才批准——`[llm]` 一个
/// 字都不该被写出来。
#[test]
fn a_refusal_is_sticky_for_the_rest_of_the_pairing() {
    let gw = common::fake_gateway_gated();
    let home = tempfile::tempdir().unwrap();
    let d = common::daemon_with(home.path(), &gw.origin());

    match d.call(Request::PairStart {
        profile: "dc".into(),
        opt_in_llm: true,
    }) {
        Response::PairStarted(Ok(_)) => {}
        other => panic!("配对没能起步：{other:?}"),
    }

    gw.wait_for_poll_inflight(Duration::from_secs(10));

    // 学生读完那行文案，按 `l` 取消勾选。
    match d.call(Request::PairPoll {
        profile: "dc".into(),
        opt_in_llm: false,
    }) {
        Response::PairTick(_) => {}
        other => panic!("轮询该照常回一个 tick：{other:?}"),
    }
    // 随后又来了一条 `true`。这一条不许把上面那个「不」翻回去。
    match d.call(Request::PairPoll {
        profile: "dc".into(),
        opt_in_llm: true,
    }) {
        Response::PairTick(_) => {}
        other => panic!("轮询该照常回一个 tick：{other:?}"),
    }

    gw.approve();
    let tick = common::wait_for_tick(&d, "dc", true, Duration::from_secs(10));
    assert!(
        matches!(tick, PairTick::Done { .. }),
        "配对本身要照常成功：{tick:?}"
    );

    // 钥匙照写——学生拒绝的是「AI 解释」这一件事，不是整条配对。
    let secrets = std::fs::read_to_string(home.path().join("secrets.toml")).unwrap();
    assert!(secrets.contains("dc"), "{secrets}");

    match std::fs::read_to_string(home.path().join("config.toml")) {
        Err(_) => {}
        Ok(cfg) => assert!(
            !cfg.contains("[llm]"),
            "学生拒绝过一次，后面的 true 不该把它翻回来：{cfg}"
        ),
    }
}

use dct::proto::{Request, Response};

mod common;

#[test]
fn daemon_serves_create_list_and_stop() {
    let h = common::start_daemon();

    let workdir = tempfile::tempdir().unwrap();
    let mut c = h.client();

    let resp = c
        .call(Request::Create {
            dir: workdir.path().display().to_string(),
            profile: "shell".into(),
            remember: true,
        })
        .unwrap();
    let id = match resp {
        Response::Created { id } => id,
        other => panic!("预期 Created，实际 {other:?}"),
    };

    match c.call(Request::List).unwrap() {
        Response::Sessions(v) => {
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].id, id);
            assert_eq!(v[0].profile, "shell");
        }
        other => panic!("预期 Sessions，实际 {other:?}"),
    }

    assert!(matches!(
        c.call(Request::Stop { id }).unwrap(),
        Response::Ok
    ));
}

#[test]
fn screens_request_returns_batch_over_socket() {
    let h = common::start_daemon();
    let mut c = h.client();

    let create = |c: &mut dct::client::Client| -> u32 {
        let workdir = tempfile::tempdir().unwrap();
        match c
            .call(Request::Create {
                dir: workdir.path().display().to_string(),
                profile: "shell".into(),
                remember: true,
            })
            .unwrap()
        {
            Response::Created { id } => id,
            other => panic!("预期 Created，实际 {other:?}"),
        }
    };
    let id1 = create(&mut c);
    let id2 = create(&mut c);

    // 往第一个会话里打一句会回显的话。断言只有 id 对得上是不够的——九宫格
    // 靠的是「这一格的画面确实是这个会话的」，画面内容和 id 配错了，用户会
    // 对着甲会话的格子按 s 停掉乙会话。
    let probe = "dct-span-probe-42";
    c.call(Request::Input {
        id: id1,
        text: format!("echo {probe}\n"),
    })
    .unwrap();

    // shell 起来、回显、vt100 解析都要一点时间，轮询到出现为止
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        // 请求里混进一个不存在的 id：会话可能在两次轮询之间被停掉，守护进程
        // 必须跳过它而不是整批报错（否则一个刚没的会话会让整屏格子空掉）。
        let screens = match c
            .call(Request::Screens {
                ids: vec![id1, 999_999, id2],
            })
            .unwrap()
        {
            Response::Screens { screens } => screens,
            other => panic!("预期 Screens，实际 {other:?}"),
        };
        assert_eq!(screens.len(), 2, "不存在的 id 应当被跳过，不是报错");
        assert_eq!(screens[0].id, id1);
        assert_eq!(screens[1].id, id2);

        let text_of = |e: &dct::proto::ScreenEntry| -> String {
            e.lines
                .iter()
                .flat_map(|l| l.iter().map(|s| s.text.as_str()))
                .collect()
        };
        assert!(
            !text_of(&screens[1]).contains(probe),
            "打给 id1 的东西不能出现在 id2 的格子里"
        );
        if text_of(&screens[0]).contains(probe) {
            // 画面是按样式切成一个个 span 过 socket 的，不是一整行字符串——
            // 丢掉这层结构，Claude Code 那种靠颜色分区的输出在格子里会退化成
            // 一片单色。这里断言到 span 一级：探针字符串完整落在某一个 span 里，
            // 而且这个 span 带着自己的样式一起过来了。
            let span = screens[0]
                .lines
                .iter()
                .flatten()
                .find(|s| s.text.contains(probe))
                .expect("探针字符串必须完整落在某一个 span 里，不能被切碎");
            assert!(!span.style.bold, "回显的普通文本不该被标成粗体");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "十秒内没在 id1 的画面里看到 {probe}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn unknown_session_returns_error_not_panic() {
    let h = common::start_daemon();
    let mut c = h.client();
    match c.call(Request::Stop { id: 999 }).unwrap() {
        // 守护进程报的是**码**，不是句子——它不知道用户在用什么语言。
        // 句子由界面用 `i18n::msg::error` 组出来。
        Response::Error(code) => assert_eq!(code, dct::proto::ErrorCode::NoSuchSession(999)),
        other => panic!("预期 Error，实际 {other:?}"),
    }
}

use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::profile::Profile;
use crate::profile::{all_profiles, command_exists, profiles_dir_for_socket, status_of};
use crate::projects::{store_path_for_socket, Store};
use crate::proto::{ErrorCode, InstallPrompt, ProfileEntry, Request, Response, SecretPrompt};
use crate::secrets::{secrets_path_for_socket, SecretStore};
use crate::session::{recover, SessionManager};
use crate::verify::{send_probe, verify_with, VerifyOutcome};

pub fn run(socket: &Path) -> Result<()> {
    let mgr = SessionManager::new();
    // 生死簿只在真正的守护进程里落盘。单元测试自己 `new()` 一个 manager，
    // 拿到的是不记账的那种——绝不能去写用户真实的 `~/.dct/sessions.log`。
    mgr.journal
        .set_path(crate::proto::journal_path_for_socket(socket));
    run_with_manager(socket, Arc::new(mgr))
}

/// 供测试注入自定义 `SessionManager`（比如预先 `register_profile` 一个专供测试用的慢
/// profile），`run()` 只是用一个全新的 manager 调用它。
///
/// 这里不再包一层 `Mutex<SessionManager>`：`SessionManager` 自己已经是内部可变的
/// （见 `session.rs` 的注释），各方法自己负责该锁多细的粒度。如果外面再套一把大锁，
/// 就白做了那些细粒度设计——一个连接处理慢请求时又会把其它连接一起卡住。
pub fn run_with_manager(socket: &Path, mgr: Arc<SessionManager>) -> Result<()> {
    // 权限必须收紧到只有属主可访问。这个 socket 能开会话、能往会话里发任意
    // 输入——谁连得上，谁就能在这台机器上以你的身份执行任意命令。默认的 0755
    // 意味着同机器的其它账号都能连。
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;

    // 存放位置跟着 socket 走，测试把 socket 放临时目录就自动隔离，
    // 不会去动真实的 ~/.dct/projects.json / ~/.dct/secrets.toml / ~/.dct/profiles/。
    let store = Arc::new(Mutex::new(Store::load(&store_path_for_socket(socket))));
    let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path_for_socket(
        socket,
    ))));
    let profiles_dir = profiles_dir_for_socket(socket);

    // 出错解释要用的后端：进程一启动就 resolve 一次，不是每次会话失败才现查
    // ——`tick()` 绝不能在判失败的那一刻还去做「找后端」这种可能失败的活。
    // 抽成独立函数是为了能不起真实 socket/listener 就单测「没写 [llm] 就不该
    // 装后端」这条 Critical 修复本身，见下面 `install_llm_backend` 和它的测试。
    install_llm_backend(socket, &profiles_dir, &mgr);

    let tick_mgr = mgr.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        tick_mgr.tick();
    });

    for conn in listener.incoming() {
        let conn = conn?;
        let m = mgr.clone();
        let s = store.clone();
        let sec = secrets.clone();
        let pd = profiles_dir.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m, s, sec, pd) {
                eprintln!("连接处理失败: {e}");
            }
        });
    }
    Ok(())
}

/// **`cfg.llm` 是 `None` 就什么都不做**：不 resolve、不装后端、也不打印
/// 任何一行——这是绝大多数用户的正常状态（没写过 `[llm]`），不是一种
/// 「本来该配却没配好」的错误。见 `config.rs` 头注释：出错解释会把一个
/// 失败会话屏幕上的原始内容送给模型，这必须是用户自己写下 `[llm]` 才
/// 触发的动作，不能因为「什么都没配」就替他打开、把他终端里的东西发
/// 给第三方。只有用户确实写了 `[llm]` 却指向一个连不上的后端时，才值得
/// 在 stderr 上留一行——那时候他大概率是想用这功能的，只是配错了。
fn install_llm_backend(socket: &Path, profiles_dir: &Path, mgr: &SessionManager) {
    let Some(llm) =
        &crate::config::Config::load(&crate::config::config_path_for_socket(socket)).llm
    else {
        return;
    };
    let llm_secrets = SecretStore::load(&secrets_path_for_socket(socket));
    let (custom, _) = all_profiles(profiles_dir);
    let lookup = |n: &str| {
        custom
            .iter()
            .find(|p| p.name == n)
            .cloned()
            .or_else(|| Profile::builtin(n))
    };
    match crate::llm::resolve::resolve(llm, &lookup, &llm_secrets, &startup_oauth) {
        Ok(b) => {
            mgr.set_backend(Some(b));
            mgr.set_llm_problem(None);
        }
        Err(e) => {
            // stderr 这一行只有 `dct daemon` 前台跑的时候看得见：界面进程
            // 拉起守护进程时把 stderr 接到了 `/dev/null`（`spawn_daemon`——
            // 不然每一行都会糊在 TUI 上）。所以真正让用户看到的是记下来的
            // 这个码，`Request::Profiles` 会把它当成一条警告顶上去。
            eprintln!("dct: 出错解释开着，但连不上（{e:?}），会话照常跑");
            mgr.set_backend(None);
            mgr.set_llm_problem(Some(e));
        }
    }
}

/// 只有 claude/codex 有自己的 OAuth 关系，别的 provider 只能走用户自己填的
/// key（`resolve::resolve` 里 key 优先于 OAuth 那条顺序保证了这一点）。跟
/// `cli.rs::oauth_lookup` 是同一条规则，见那边的注释——**不要**把 kimi/glm/
/// deepseek/qwen-api 也映射到这两个查询上，那等于把用户的 Anthropic/OpenAI
/// 登录态发给几家跟它们毫无关系的第三方服务器。
///
/// 每份凭据都带着**出处**（`BorrowedFrom`）一起返回：按名字挑只是第一道关，
/// 名字是用户可以手写的，真正管用的是 `resolve::select_credential` 拿这个
/// 出处去比对目的地主机。
///
/// 单独写一份而不是复用 `cli.rs::oauth_lookup`：那边把两个查询做成了可注入
/// 的闭包参数，是为了单测能绕开真实 Keychain / `auth.json`；守护进程启动
/// 只跑一次真实查询，没有这个诉求，硬套那个签名反而要在这里现造两个闭包。
fn startup_oauth(name: &str) -> Option<crate::llm::creds::Borrowed> {
    use crate::llm::creds::{BorrowedFrom, Credential};
    match name {
        "claude" => crate::llm::creds::read_claude_oauth()
            .map(|t| (BorrowedFrom::ClaudeCli, Credential::Bearer(t))),
        "codex" => crate::llm::creds::read_codex_auth().map(|c| (BorrowedFrom::CodexCli, c)),
        _ => None,
    }
}

fn serve(
    stream: UnixStream,
    mgr: Arc<SessionManager>,
    store: Arc<Mutex<Store>>,
    secrets: Arc<Mutex<SecretStore>>,
    profiles_dir: PathBuf,
) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, &mgr, &store, &secrets, &profiles_dir),
            Err(e) => Response::Error(ErrorCode::BadRequest(e.to_string())),
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
        out.flush()?;
    }
    Ok(())
}

fn handle(
    req: Request,
    mgr: &Arc<SessionManager>,
    store: &Arc<Mutex<Store>>,
    secrets: &Arc<Mutex<SecretStore>>,
    profiles_dir: &Path,
) -> Response {
    let r: anyhow::Result<Response> = match req {
        // 不碰任何状态，也不该失败：界面拿它判断「我该不该跟你说话」。
        Request::Hello => Ok(Response::Hello {
            protocol: crate::proto::PROTOCOL_VERSION,
        }),
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles { lang } => {
            let (all, mut warnings) = all_profiles(profiles_dir);
            let sec = recover(secrets.lock());
            if let Some(e) = sec.load_error() {
                // 密钥文件读不了要顶到界面上。静默的话用户会以为密钥丢了，
                // 而且这时候所有写入都被拒，他改什么都没反应。
                //
                // IMPORTANT 4（最终整分支 code review）：以前这里无条件拼一句
                // 「检查一下 {path}」——对权限错误说得通（去看看那个文件），
                // 但套在密钥文件损坏的情形上就是让用户去手改一个 README 明说
                // 不支持手改的文件。`load_error()` 现在返回的已经是一句自足、
                // 说清楚该干什么的中文（见 `SecretStore::load` 的注释），这里
                // 只负责把路径带上，不再叠加任何暗示"去编辑它"的措辞。
                warnings.insert(0, e.clone());
            }
            // 用户开了出错解释却接不上，这条原因只有这一条路能走到他眼前
            // （守护进程的 stderr 被丢弃了）。排在密钥/profile 那些警告后面：
            // 那几条是「你现在要用的东西坏了」，这条是「一个增强功能没生效」。
            if let Some(p) = mgr.llm_problem() {
                warnings.push(crate::proto::WarningCode::LlmUnavailable(p));
            }
            let entries = all
                .iter()
                .map(|p| {
                    // 只查一次，分别喂给 status_of（装没装排在密钥前面，见
                    // profile.rs 的注释）和 has_secret（密钥页要的是这个事实
                    // 本身，不能从 status 反推——见 ProfileEntry::has_secret
                    // 的注释）。两处用同一次查询结果，不会因为中间密钥文件
                    // 被并发改过而看到两个不一致的答案。
                    let has_secret = sec.get(&p.name).is_some();
                    ProfileEntry {
                        name: p.name.clone(),
                        label: p.display_label(lang),
                        note: p.display_note(lang),
                        status: status_of(p, &all, has_secret, &command_exists, lang),
                        secret: p.secret.as_ref().map(|s| SecretPrompt {
                            hint: s.hint.get(lang).unwrap_or("").to_string(),
                            url: s.url.clone(),
                        }),
                        install: p.install.as_ref().map(|i| InstallPrompt {
                            command: i.command.clone(),
                            note: i.note.get(lang).unwrap_or("").to_string(),
                        }),
                        has_secret,
                    }
                })
                .collect();
            Ok(Response::Profiles { entries, warnings })
        }
        Request::Projects => Ok(Response::Projects(recover(store.lock()).list())),
        Request::Create {
            dir,
            profile,
            remember,
        } => {
            let dir = PathBuf::from(dir);
            // 只借一眼这一个 profile 对应的密钥，锁拿完立刻放：`create()` 接下来要
            // 起 PTY 子进程，agent profile 还要跑一次 git checkpoint，这些都是慢
            // 操作（同样的原则见 session.rs::create 顶上的注释和它引用的
            // 「以下全是慢操作」那段）。锁如果跟着这段慢操作一起持有，Task 8 加的
            // SetSecret/DeleteSecret 就会被一个正在建的慢会话挡在门外——
            // 而这两个操作本身其实只需要极短时间。
            let secret = recover(secrets.lock()).get(&profile).map(str::to_string);
            // resolve_profile 要认得磁盘上的自定义 profile，不止内置那九个——
            // 否则「UI 上看着能用」和「create() 说没这个 profile」会对不上。
            let (all, _) = all_profiles(profiles_dir);
            let r = mgr
                .create(&dir, &profile, secret.as_deref(), &all)
                .map(|id| Response::Created { id });
            // 只有建成功了才记账。建失败的目录进了「最近项目」，
            // 下次还会被选中、还会失败。这把 store 锁跟上面的 secrets 锁完全无关，
            // 特意没有嵌套在一起拿，理由同上：不能让一把锁的持有时间绑架另一把。
            if r.is_ok() {
                let mut st = recover(store.lock());
                st.touch(&dir);
                // remember=false 是「帮你装 CLI」那条路径：它开的 shell 会话
                // 不是用户选的 agent，记了下次按 n 会掉进命令行。
                if remember {
                    st.set_last_profile(&profile);
                }
            }
            r
        }
        Request::Input { id, text } => mgr.send_input(id, &text).map(|_| Response::Ok),
        Request::Screen { id } => mgr.screen(id).map(|snap| Response::Screen {
            lines: snap.lines,
            cursor: snap.cursor,
            state: snap.state,
            scroll: snap.scroll,
        }),
        Request::Screens { ids } => Ok(Response::Screens {
            screens: mgr.screens(&ids),
        }),
        Request::Resize { id, rows, cols } => mgr.resize(id, rows, cols).map(|_| Response::Ok),
        Request::Scroll { id, by } => mgr.scroll(id, by).map(Response::Scrolled),
        Request::Mouse { id, event } => mgr.forward_mouse(id, event).map(|_| Response::Ok),
        Request::Stop { id } => mgr.stop(id).map(|_| Response::Ok),
        Request::Kill { id } => mgr.kill(id).map(|_| Response::Ok),
        Request::Prune => Ok(Response::Pruned(mgr.prune())),
        Request::Undo { id } => mgr.undo(id).map(|_| Response::Ok),
        Request::Diff { id } => mgr.diff(id).map(Response::Diff),
        Request::SetSecret { profile, value } => recover(secrets.lock())
            .set(&profile, &value)
            .map(|_| Response::Ok),
        Request::DeleteSecret { profile } => recover(secrets.lock())
            .remove(&profile)
            .map(|_| Response::Ok),
        Request::LastProfile => Ok(Response::LastProfile(
            recover(store.lock()).last_profile().map(str::to_string),
        )),
        // 永远不失败：没有解释（没配后端、还没算完、算失败了）跟「问不到」
        // 是同一件事，界面不用区分，统一显示今天就有的那句失败提示。
        Request::Explanation { id } => Ok(Response::Explanation(mgr.explanation(id))),
        Request::VerifySecret { profile, value } => {
            let (all, _) = all_profiles(profiles_dir);
            let spec = all
                .iter()
                .find(|p| p.name == profile)
                .and_then(|p| p.secret.as_ref())
                .and_then(|s| s.verify.as_ref());
            match spec {
                // 没声明 verify 的 profile 直接放行，不是错误
                None => Ok(Response::Verify(VerifyOutcome::Ok)),
                Some(v) => Ok(Response::Verify(verify_with(&v.url, &value, &send_probe))),
            }
        }
    };
    r.unwrap_or_else(|e| Response::Error(to_code(e)))
}

/// 把内部错误还原成给界面的码。`downcast` 拿不到码的，说明这条路径还没归类——
/// 照抄原文走 `Internal`，界面原样显示。这样迁移可以一条一条来，不必等到
/// 每一条都归好类才敢合并。
fn to_code(e: anyhow::Error) -> ErrorCode {
    match e.downcast::<crate::proto::CodedError>() {
        Ok(c) => c.0,
        Err(e) => ErrorCode::Internal(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 造一个文件足够多的仓库，让 agent 会话建立时的首次 git checkpoint 慢到能
    /// 测出来。手法照抄 `tests/concurrency.rs` 的 `init_big_repo`——那边已经验证过
    /// 8000 个文件在这台机器的规模下够慢、够稳。
    fn init_big_repo(path: &Path, n: usize) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::create_dir_all(path.join("files")).unwrap();
        for i in 0..n {
            std::fs::write(
                path.join("files").join(format!("f{i}.txt")),
                format!("{i}\n"),
            )
            .unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
    }

    // 用 cat 冒充 agent：能收输入、不会自己退出，is_agent = true 才会触发
    // create() 里的 git checkpoint。
    fn fake_agent() -> Profile {
        Profile {
            name: "daemon-lock-fake".into(),
            command: vec!["cat".into()],
            is_agent: true,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: None,
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    /// 回归测试，对应审查发现「原始 OS 报错会红字出现在选择器标题上」：
    /// `Request::Profiles` 拼 warning 时不能只把 `SecretStore::load_error()`
    /// 的文案接上就完事——那句话本身已经是人话了（见 secrets.rs），但这里
    /// 还要点名是哪个文件，且组装出来的整句不能再夹带任何英文系统原话。
    #[test]
    fn profiles_warning_names_the_broken_secrets_file_in_chinese() {
        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets_path = secrets_dir.path().join("secrets.toml");
        std::fs::write(&secrets_path, "这不是 TOML {{{").unwrap();

        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path)));
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::load(
            &store_dir.path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let resp = handle(
            Request::Profiles {
                lang: crate::i18n::Lang::Zh,
            },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
        );

        match resp {
            Response::Profiles { warnings, .. } => {
                // 守护进程报的是**码**。它点名了是哪个文件，但一个字的
                // 文案都不组——句子由界面用 `i18n::msg::warning` 组出来。
                let w = warnings
                    .iter()
                    .find(|w| matches!(w, crate::proto::WarningCode::SecretsCorrupt { .. }))
                    .expect("密钥文件读坏了必须有 warning");
                let crate::proto::WarningCode::SecretsCorrupt { path } = w else {
                    unreachable!()
                };
                assert_eq!(
                    path,
                    &secrets_path.display().to_string(),
                    "要点名是哪个文件"
                );

                // 组出来的那句话仍然要满足原来的两条约束：一行、不带
                // toml 库自带的图形化 Display。
                let line = crate::i18n::msg::warning(crate::i18n::Lang::Zh, w);
                assert!(!line.contains('\n'), "不能是多行栈追踪：{line}");
                assert!(
                    !line.contains("TOML parse error"),
                    "toml 库自带的图形化 Display 不能漏出来：{line}"
                );
            }
            other => panic!("期待 Response::Profiles，得到 {other:?}"),
        }
    }

    /// 守护进程不该替用户决定语言。它是常驻的、可能同时服务多个界面的进程，
    /// 「谁的语言是什么」不是它的状态——界面在请求里带上，它照着取就行。
    /// 以前这里硬编码 `Lang::Zh`，于是 profile 的 `en` 文案写了也永远没人读。
    #[test]
    fn profiles_are_labelled_in_the_language_the_client_asked_for() {
        let mgr = Arc::new(SessionManager::new());
        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &secrets_dir.path().join("secrets.toml"),
        )));
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::load(
            &store_dir.path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let labels = |lang| match handle(
            Request::Profiles { lang },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
        ) {
            Response::Profiles { entries, .. } => entries
                .into_iter()
                .map(|e| format!("{}|{}", e.label, e.note))
                .collect::<Vec<_>>()
                .join("  "),
            other => panic!("期待 Response::Profiles，得到 {other:?}"),
        };

        let zh = labels(crate::i18n::Lang::Zh);
        let en = labels(crate::i18n::Lang::En);
        assert!(
            zh.contains("命令行"),
            "中文下 shell 的名字是「命令行」：{zh}"
        );
        assert_ne!(zh, en, "换了语言，菜单文案必须真的跟着变");
    }

    /// 回归测试，对应审查发现「密钥仓的锁被握过了整个 create()」：以前 `handle()`
    /// 会在建会话的整段慢流程（PTY 起进程、agent 场景下的 git checkpoint）期间
    /// 一直攥着 secrets 锁。Task 8 加了 SetSecret/DeleteSecret，这两个本该极快的
    /// 操作绝不能被一个正在建的慢会话堵住排队。
    ///
    /// 直接调用 `handle()` 而不是走真实 socket，是为了最直接地量最本质的东西：
    /// 慢 `Create` 跑在一个线程时，另一个线程单纯去锁 `secrets` 这把 `Mutex`
    /// 本身，应该几乎立即拿到，不必等 `Create` 收工。
    #[test]
    fn create_does_not_hold_the_secrets_lock_across_the_slow_work() {
        let repo = tempfile::tempdir().unwrap();
        init_big_repo(repo.path(), 8000);

        let mgr = Arc::new(SessionManager::new());
        mgr.register_profile(fake_agent());

        let secrets_dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &secrets_dir.path().join("secrets.toml"),
        )));
        let store_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(Store::load(
            &store_dir.path().join("projects.json"),
        )));
        // 空目录：这条测试不关心磁盘 profile，`daemon-lock-fake` 只在
        // `mgr` 的 `extra_profiles` 里注册过。
        let profiles_dir = tempfile::tempdir().unwrap();

        let mgr2 = mgr.clone();
        let store2 = store.clone();
        let secrets2 = secrets.clone();
        let repo_path = repo.path().display().to_string();
        let profiles_dir_path = profiles_dir.path().to_path_buf();
        let create_handle = std::thread::spawn(move || {
            let t = Instant::now();
            let resp = handle(
                Request::Create {
                    dir: repo_path,
                    profile: "daemon-lock-fake".into(),
                    remember: true,
                },
                &mgr2,
                &store2,
                &secrets2,
                &profiles_dir_path,
            );
            (t.elapsed(), resp)
        });

        // 给慢 Create 一点时间真正进到 git checkpoint 里
        std::thread::sleep(Duration::from_millis(150));

        let t = Instant::now();
        drop(recover(secrets.lock()));
        let lock_wait = t.elapsed();

        let (create_elapsed, create_resp) = create_handle.join().unwrap();

        assert!(
            matches!(create_resp, Response::Created { .. }),
            "Create 应该最终成功，实际 {create_resp:?}"
        );
        assert!(
            create_elapsed > Duration::from_millis(300),
            "场景失真：Create 耗时应显著大于 300ms 才能验证不阻塞，实际 {create_elapsed:?}"
        );
        assert!(
            lock_wait < Duration::from_millis(100),
            "secrets 锁被慢 Create 攥着不放：等了 {lock_wait:?}（同期 Create 耗时 {create_elapsed:?}）"
        );
    }

    // 冒充一个会报错的 agent：echo BOOM 之后常驻，好让 tick() 判成 Failed
    // 而不是 Stopped（同 session.rs::tests::failing_agent）。
    fn failing_agent() -> Profile {
        Profile {
            name: "daemon-explain-fake".into(),
            command: vec!["/bin/sh".into(), "-c".into(), "echo BOOM; sleep 5".into()],
            is_agent: true,
            idle_pattern: None,
            busy_pattern: None,
            error_pattern: Some("BOOM".into()),
            env: Default::default(),
            secret: None,
            install: None,
            headless: None,
            api: None,
            label: Default::default(),
            note: Default::default(),
        }
    }

    struct FixedAnswer;
    impl crate::llm::Backend for FixedAnswer {
        fn complete(&self, _p: &crate::llm::Prompt) -> Result<String, crate::llm::LlmError> {
            Ok("这个命令没配好，重开一次就行。".into())
        }
    }

    /// 回归测试：`Request::Explanation` 真的接到了 `mgr.explanation()`，不是
    /// 一条只在类型上存在、`handle()` 里没人接的死请求。
    #[test]
    fn explanation_request_is_wired_to_the_session_manager() {
        let repo = tempfile::tempdir().unwrap();
        {
            let run = |args: &[&str]| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .output()
                    .unwrap();
            };
            run(&["init", "-q"]);
            run(&["config", "user.email", "t@example.com"]);
            run(&["config", "user.name", "t"]);
            std::fs::write(repo.path().join("a.txt"), "hi\n").unwrap();
            run(&["add", "-A"]);
            run(&["commit", "-q", "-m", "init"]);
        }

        let mgr = Arc::new(SessionManager::new());
        mgr.register_profile(failing_agent());
        mgr.set_backend(Some(Arc::new(FixedAnswer)));
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let id = mgr
            .create(repo.path(), "daemon-explain-fake", None, &[])
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            mgr.tick();
            let resp = handle(
                Request::Explanation { id },
                &mgr,
                &store,
                &secrets,
                profiles_dir.path(),
            );
            if let Response::Explanation(Some(text)) = resp {
                assert_eq!(text, "这个命令没配好，重开一次就行。");
                return;
            }
            assert!(Instant::now() < deadline, "Explanation 请求一直没接到解释");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// 没有对应会话时，`Explanation` 也要老老实实回 `None`，不能 panic
    /// 或者报错——「问不到」跟「没配后端」在界面眼里是同一件事。
    #[test]
    fn explanation_for_an_unknown_session_is_none_not_an_error() {
        let mgr = Arc::new(SessionManager::new());
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &tempfile::tempdir().unwrap().path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(
            &tempfile::tempdir().unwrap().path().join("projects.json"),
        )));
        let profiles_dir = tempfile::tempdir().unwrap();

        let resp = handle(
            Request::Explanation { id: 9999 },
            &mgr,
            &store,
            &secrets,
            profiles_dir.path(),
        );
        assert!(matches!(resp, Response::Explanation(None)));
    }

    /// **Critical 回归测试.** 没写 `[llm]`（这里就是没有 `config.toml` 这个
    /// 文件——「不存在」和「写了但没这一段」在 `config.rs` 里是同一件事）
    /// 是绝大多数用户的正常状态，出错解释必须整个关着：`install_llm_backend`
    /// 压根不能调 `resolve()`，更不能装上一个会把终端内容发出去的后端。
    ///
    /// 断言的是 `backend_is_set()` 这个布尔值，不是「问一次真实网络/CLI
    /// 会不会成功」——默认 provider 是 `claude` + `Transport::Cli`，如果
    /// Critical 那个 bug 还在，这条路径的 `resolve()` 本来就会成功（`Cli`
    /// 传输不需要凭据），间接测法（等一个 explanation 出现）反而会被「这台
    /// 机器上到底装没装 claude CLI」这种环境噪音污染，钉不住真正的问题。
    #[test]
    fn no_llm_section_means_no_backend_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock"); // 旁边故意不写 config.toml
        let mgr = SessionManager::new();

        install_llm_backend(&socket, &dir.path().join("profiles"), &mgr);

        assert!(
            !mgr.backend_is_set(),
            "没写 [llm] 就不该装后端——这是隐私边界，不是默认值的事"
        );
    }

    /// 反过来钉住「写了就真的开」：不能为了堵上面那条回归，把功能焊死关掉。
    /// `[llm]` 段里什么字段都不填，靠的是 `LlmConfig` 自己的默认值
    /// （provider claude、transport Cli），这条路径不需要任何真实凭据就该
    /// 成功——`Transport::Cli` 只是记下命令，不在这一步真的起子进程。
    #[test]
    fn a_bare_llm_section_does_install_a_backend() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        std::fs::write(crate::config::config_path_for_socket(&socket), "[llm]\n").unwrap();
        let mgr = SessionManager::new();

        install_llm_backend(&socket, &dir.path().join("profiles"), &mgr);

        assert!(
            mgr.backend_is_set(),
            "写了 [llm]（哪怕是空的）就该是一次显式的开——这条不能被上一条回归测试误伤"
        );
        assert!(mgr.llm_problem().is_none(), "接上了就不该留着一条抱怨");
    }

    /// 用户开了出错解释、却配错了，**这件事必须走得到他眼前**。
    ///
    /// 守护进程那句 `eprintln!` 是看不见的：界面进程拉起它的时候把 stderr
    /// 接到了 `/dev/null`（`client::spawn_daemon`，不然每一行都会糊在 TUI
    /// 上）。所以原因要记在守护进程上，并且跟着 `Request::Profiles` 一起
    /// 顶到界面的警告栏——这条测试钉的就是这条通路，从「配错了」一直到
    /// 「界面拿到一条能读的警告」。
    #[test]
    fn a_broken_llm_setting_reaches_the_user_instead_of_going_silent() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        std::fs::write(
            crate::config::config_path_for_socket(&socket),
            "[llm]\nprovider = \"根本没有这个\"\n",
        )
        .unwrap();
        let mgr = Arc::new(SessionManager::new());
        let profiles_dir = dir.path().join("profiles");

        install_llm_backend(&socket, &profiles_dir, &mgr);

        assert!(!mgr.backend_is_set(), "连不上就不该装后端");
        let secrets = Arc::new(Mutex::new(SecretStore::load(
            &dir.path().join("secrets.toml"),
        )));
        let store = Arc::new(Mutex::new(Store::load(&dir.path().join("projects.json"))));
        let resp = handle(
            Request::Profiles {
                lang: crate::i18n::Lang::Zh,
            },
            &mgr,
            &store,
            &secrets,
            &profiles_dir,
        );
        let Response::Profiles { warnings, .. } = resp else {
            panic!("期待 Response::Profiles");
        };
        let w = warnings
            .iter()
            .find(|w| matches!(w, crate::proto::WarningCode::LlmUnavailable(_)))
            .expect("配错了却一个警告都没有——用户会以为功能坏了却查不到任何线索");
        let line = crate::i18n::msg::warning(crate::i18n::Lang::Zh, w);
        assert!(
            line.contains("根本没有这个"),
            "要点名是设置里的哪个名字写错了：{line}"
        );
    }
}

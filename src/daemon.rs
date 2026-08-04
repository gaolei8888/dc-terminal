use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(test)]
use crate::profile::Profile;
use crate::profile::{all_profiles, command_exists, profiles_dir_for_socket, status_of, Lang};
use crate::projects::{store_path_for_socket, Store};
use crate::proto::{InstallPrompt, ProfileEntry, Request, Response, SecretPrompt};
use crate::secrets::{secrets_path_for_socket, SecretStore};
use crate::session::{recover, SessionManager};

pub fn run(socket: &Path) -> Result<()> {
    run_with_manager(socket, Arc::new(SessionManager::new()))
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
            Err(e) => Response::Error(format!("请求解析失败: {e}")),
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
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles => {
            let (all, mut warnings) = all_profiles(profiles_dir);
            let sec = recover(secrets.lock());
            if let Some(e) = sec.load_error() {
                // 密钥文件读不了要顶到界面上。静默的话用户会以为密钥丢了，
                // 而且这时候所有写入都被拒，他改什么都没反应。
                warnings.insert(0, format!("密钥文件读不了：{e}"));
            }
            let entries = all
                .iter()
                .map(|p| ProfileEntry {
                    name: p.name.clone(),
                    label: p.display_label(Lang::Zh),
                    note: p.display_note(Lang::Zh),
                    status: status_of(
                        p,
                        &all,
                        sec.get(&p.name).is_some(),
                        &command_exists,
                        Lang::Zh,
                    ),
                    secret: p.secret.as_ref().map(|s| SecretPrompt {
                        hint: s.hint.get(Lang::Zh).unwrap_or("").to_string(),
                        url: s.url.clone(),
                    }),
                    install: p.install.as_ref().map(|i| InstallPrompt {
                        command: i.command.clone(),
                        note: i.note.get(Lang::Zh).unwrap_or("").to_string(),
                    }),
                })
                .collect();
            Ok(Response::Profiles {
                entries,
                warning: if warnings.is_empty() {
                    None
                } else {
                    Some(warnings.join("；"))
                },
            })
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
        Request::Screen { id } => mgr
            .screen(id)
            .map(|(lines, cursor)| Response::Screen { lines, cursor }),
        Request::Resize { id, rows, cols } => mgr.resize(id, rows, cols).map(|_| Response::Ok),
        Request::Stop { id } => mgr.stop(id).map(|_| Response::Ok),
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
    };
    r.unwrap_or_else(|e| Response::Error(e.to_string()))
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
            env: Default::default(),
            secret: None,
            install: None,
            label: Default::default(),
            note: Default::default(),
        }
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
}

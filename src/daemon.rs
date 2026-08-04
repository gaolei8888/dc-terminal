use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::profile::Profile;
use crate::projects::{store_path_for_socket, Store};
use crate::proto::{Request, Response};
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
    // 不会去动真实的 ~/.dct/projects.json / ~/.dct/secrets.toml。
    let store = Arc::new(Mutex::new(Store::load(&store_path_for_socket(socket))));
    let secrets = Arc::new(Mutex::new(SecretStore::load(&secrets_path_for_socket(
        socket,
    ))));

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
        std::thread::spawn(move || {
            if let Err(e) = serve(conn, m, s, sec) {
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
) -> Result<()> {
    let mut out = stream.try_clone()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => handle(req, &mgr, &store, &secrets),
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
) -> Response {
    let r: anyhow::Result<Response> = match req {
        Request::List => Ok(Response::Sessions(mgr.list())),
        Request::Profiles => Ok(Response::Profiles(
            Profile::builtin_names()
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )),
        Request::Projects => Ok(Response::Projects(recover(store.lock()).list())),
        Request::Create { dir, profile } => {
            let dir = PathBuf::from(dir);
            // 只借一眼这一个 profile 对应的密钥，锁拿完立刻放：`create()` 接下来要
            // 起 PTY 子进程，agent profile 还要跑一次 git checkpoint，这些都是慢
            // 操作（同样的原则见 session.rs::create 顶上的注释和它引用的
            // 「以下全是慢操作」那段）。锁如果跟着这段慢操作一起持有，Task 8 加的
            // SetSecret/DeleteSecret 就会被一个正在建的慢会话挡在门外——
            // 而这两个操作本身其实只需要极短时间。
            let secret = recover(secrets.lock()).get(&profile).map(str::to_string);
            let r = mgr
                .create(&dir, &profile, secret.as_deref())
                .map(|id| Response::Created { id });
            // 只有建成功了才记账。建失败的目录进了「最近项目」，
            // 下次还会被选中、还会失败。这把 store 锁跟上面的 secrets 锁完全无关，
            // 特意没有嵌套在一起拿，理由同上：不能让一把锁的持有时间绑架另一把。
            if r.is_ok() {
                recover(store.lock()).touch(&dir);
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
    /// 一直攥着 secrets 锁。Task 8 要加 SetSecret/DeleteSecret，到时候这两个本该
    /// 极快的操作就会被一个正在建的慢会话堵住排队。
    ///
    /// 这里没法走真实的 wire protocol 测（`SetSecret` 请求还不存在，Task 8 才加），
    /// 所以直接量最本质的东西：慢 `Create` 跑在一个线程时，另一个线程单纯去锁
    /// `secrets` 这把 `Mutex` 本身，应该几乎立即拿到，不必等 `Create` 收工。
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

        let mgr2 = mgr.clone();
        let store2 = store.clone();
        let secrets2 = secrets.clone();
        let repo_path = repo.path().display().to_string();
        let create_handle = std::thread::spawn(move || {
            let t = Instant::now();
            let resp = handle(
                Request::Create {
                    dir: repo_path,
                    profile: "daemon-lock-fake".into(),
                },
                &mgr2,
                &store2,
                &secrets2,
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

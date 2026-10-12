//! `dct do` 接真东西：配置、平台表、活动、dco、dcv、[judge]、[llm]、Touch ID 批准。
//! 只在 unix 上连 dco；别的平台只说一句「要 dco」就退出（退出码 2）。
#[cfg(not(unix))]
pub use not_unix::run;
#[cfg(unix)]
pub use unix::run;

#[cfg(not(unix))]
mod not_unix {
    use crate::explore_cli::DoArgs;
    pub fn run(_: &DoArgs, say: &dyn Fn(i32, &str) -> i32) -> i32 {
        say(
            2,
            "dct do 要连本机的 dco（现在只有 macOS 版），这台电脑上用不了",
        )
    }
}

#[cfg(unix)]
mod unix {
    use crate::config::Config;
    use crate::explore_cli::*;
    use crate::explore_model::{LlmModel, DEFAULT_TIMEOUT};
    use crate::procedures::{Approvals, RealDcv};
    use dct_explore::app_profile::Table;
    use dct_explore::ladder::NoModel;

    struct SystemClock;
    impl Clock for SystemClock {
        fn now_ms(&self) -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        }
    }

    fn platforms(dct_dir: &std::path::Path) -> Result<Table, String> {
        let path = dct_dir.join("platforms.toml");
        match std::fs::read_to_string(&path) {
            Ok(t) => Table::seed()
                .with_override(&t)
                .map_err(|e| format!("{}：{e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Table::seed()),
            Err(e) => Err(format!("读不了 {}：{e}", path.display())),
        }
    }

    /// `[llm]` → 后端；没配或连不上 → None（用户永远是最后一级）。
    fn llm(
        cfg: &Config,
        socket: &std::path::Path,
        say: &RefCell<dyn Say + '_>,
    ) -> Option<Arc<dyn crate::llm::Backend>> {
        let llm = cfg.llm.as_ref()?;
        match crate::explore_model::backend_from_config(llm, socket) {
            Ok(b) => Some(b),
            Err(e) => {
                say.borrow_mut().tell(
                    "note",
                    &format!("[llm] 连不上（{e:?}），这次不问大模型"),
                    json!({}),
                );
                None
            }
        }
    }

    fn rand4() -> u16 {
        let mut b = [0u8; 2];
        let _ = getrandom::getrandom(&mut b);
        u16::from_le_bytes(b)
    }

    pub fn run(a: &DoArgs, early: &dyn Fn(i32, &str) -> i32) -> i32 {
        let socket = crate::proto::socket_path();
        let dct_dir = socket.parent().map(PathBuf::from).unwrap_or_default();
        // 配置坏了要说出来（退出码 2），不能像 Config::load 那样悄悄当没配。
        let cfg = match load_config(&crate::config::config_path_for_socket(&socket)) {
            Ok(c) => c,
            Err(m) => return early(2, &m),
        };
        let table = match platforms(&dct_dir) {
            Ok(t) => t,
            Err(m) => return early(2, &m),
        };
        let Some(p) = table.get(&a.platform).cloned() else {
            let names: Vec<&str> = table.rows.iter().map(|r| r.name.as_str()).collect();
            return early(
                2,
                &format!("平台表里没有 {}（有：{}）", a.platform, names.join("、")),
            );
        };
        // 只用批准时按过指纹签的文字：没签、签名对不上、文件被改过，都在碰 dco 之前拒绝。
        let trusted =
            match crate::keys::KeyStore::at(crate::keys::KeyStore::default_dir()).trusted() {
                Ok(t) => t,
                Err(e) => return early(2, &format!("核对活动文字的指纹签名要用配对过的钥匙：{e}")),
            };
        let campaign =
            match crate::campaign::load(&crate::campaign::default_dir(), &a.campaign, &trusted) {
                Ok(c) => c,
                Err(m) => return early(2, &m),
            };
        let params = match params_for(&campaign, &p.name) {
            Ok(v) => v,
            Err(m) => return early(2, &m),
        };
        let Some(home) = crate::sys::home() else {
            return early(1, "找不到家目录");
        };
        let client = match dct_dco::DcoClient::connect_with_timeout(
            &home.join(".dco"),
            dct_dco::RUN_TIMEOUT,
        ) {
            Ok(c) => c,
            Err(e) => return early(1, &dco_text(&e)),
        };
        let mut hands = dct_explore::hands::DcoHands::new(client);
        let clock = SystemClock;
        let data_dir = dct_dir.join("explore");
        let run_id = dct_explore::trace::new_run_id(&p.name, clock.now_ms(), rand4());
        let stdin = std::io::stdin();
        let talk = RefCell::new(Talk::new(
            stdin.lock(),
            std::io::stdout(),
            a.json,
            Some(data_dir.join("runs").join(&run_id)),
        ));
        let say: &RefCell<dyn Say + '_> = &talk;
        // 每处模型都有退路：没配 [judge] → 不问 Jeff；没配 [llm] → 不问大模型、草稿不修。用户永远是最后一级。
        let (jeff, threshold): (Box<dyn Jeff>, f64) = match &cfg.judge {
            Some(jc) => match crate::judge::from_config(jc) {
                Ok(b) => (Box::new(JudgeJeff(b)), jc.threshold),
                Err(e) => {
                    say.borrow_mut().tell(
                        "note",
                        &format!("[judge] 用不了（{e}），这次不问 Jeff"),
                        json!({}),
                    );
                    (Box::new(NoJeff), jc.threshold)
                }
            },
            None => (Box::new(NoJeff), DEFAULT_THRESHOLD),
        };
        let llm_model = llm(&cfg, &socket, say)
            .map(|b| LlmModel::new(b, DEFAULT_TIMEOUT).with_never(p.never_tap.clone()));
        let (model, fixer): (&dyn Model, &dyn Fixer) = match &llm_model {
            Some(m) => (m, m),
            None => (&NoModel, &NoFix),
        };
        let keys = crate::keys::KeyStore::at(crate::keys::KeyStore::default_dir());
        let se = crate::keys::platform_enclave();
        let approvals = Approvals::at(Approvals::default_dir());
        // 只批准卡片上给用户看的那一版（正文指纹对得上才弹 Touch ID）。
        let mut approve_proc = |name: &str, shown_sha: &str| -> anyhow::Result<()> {
            let now = clock.now_ms() / 1000;
            crate::procedures::approve_expecting(
                &RealDcv,
                &keys,
                se.as_ref(),
                &approvals,
                name,
                &[],
                now,
                shown_sha,
            )
            .map(|_| ())
        };
        drive(
            a,
            &p,
            params,
            Env {
                hands: &mut hands,
                dcv: &RealDcv,
                jeff: jeff.as_ref(),
                model,
                fixer,
                threshold,
                approve_proc: &mut approve_proc,
                clock: &clock,
                data_dir,
                run_id,
                say,
            },
        )
    }
}

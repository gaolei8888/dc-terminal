//! 起中转。
//!
//! **第一期只监听环回地址，而且是硬性的。** 计划里那句「srv 只监听内网地址」
//! 是任务 7 的验收条件，但把它推迟到任务 7 才写是在赌中间这几天没人手滑：
//! 现在 `token` 根本没人验（任务 5 才接 dc_classroom），这个服务对公网开口
//! 的那一刻，任何人都能冒充任何一台设备收发信封。加密也还没有（第二期）。
//!
//! 所以拒绝绑非环回地址的判断写在这里，不写在文档里。等任务 5 和第二期落地，
//! 再把它换成一个明确的、要人动手打开的开关。

use std::sync::Arc;

use dct_srv::{Config, Live, Relay, Routes};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = dct_srv::parse_cli(&args)?;
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    let load_or_new = |file: &std::path::Path| {
        if file.exists() {
            dct_srv::keys::PublishKeys::load(file)
        } else {
            Ok(dct_srv::keys::PublishKeys::default())
        }
    };
    let (addr, routes, publish_keys) = match cli {
        dct_srv::Cli::KeyAdd { name, file } => {
            let mut k = load_or_new(&file)?;
            let key = k.add(&name, now())?;
            k.save(&file)?;
            println!("{key}");
            eprintln!("这把密钥只显示这一次。文件里只存了它的摘要。");
            return Ok(());
        }
        dct_srv::Cli::KeyRevoke { name, file } => {
            let mut k = dct_srv::keys::PublishKeys::load(&file)?;
            if !k.revoke(&name) {
                return Err(format!("没有叫「{name}」的密钥").into());
            }
            k.save(&file)?;
            eprintln!("已吊销「{name}」。运行中的中转最多 10 秒后生效。");
            return Ok(());
        }
        dct_srv::Cli::KeyList { file } => {
            for e in dct_srv::keys::PublishKeys::load(&file)?.keys {
                println!("{}\t创建于 {}", e.name, e.created);
            }
            return Ok(());
        }
        dct_srv::Cli::Takedown { id, file } => {
            let mut k = load_or_new(&file)?;
            k.block(&id);
            k.save(&file)?;
            eprintln!("已下线「{id}」。运行中的中转最多 10 秒后生效。");
            return Ok(());
        }
        dct_srv::Cli::Serve {
            addr,
            with_link,
            publish_keys,
        } => (
            addr,
            if with_link {
                Routes::WithLink
            } else {
                Routes::LiveOnly
            },
            publish_keys,
        ),
    };
    // 首次打开就坏的密钥文件：拒绝启动，而不是悄悄当成「没开公开功能」。
    let publish_keys = publish_keys.map(dct_srv::keys::KeyFile::open).transpose()?;
    let _ = publish_keys;

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    if let Err(why) = dct_srv::must_be_loopback(local) {
        // 已经绑上了才发现——那就关掉。宁可启动失败，也不要一个没鉴权的
        // 中转在公网上多活一秒。
        drop(listener);
        return Err(why.into());
    }

    println!(
        "dct-srv 在 http://{local} 上，只收本机的连接{}",
        if routes == Routes::WithLink {
            "（已打开没有鉴权的 /link/*，只许本机开发用）"
        } else {
            ""
        }
    );
    dct_srv::serve(
        listener,
        Arc::new(Relay::new(Config::default())),
        Arc::new(Live::new()),
        routes,
    )
    .await?;
    Ok(())
}

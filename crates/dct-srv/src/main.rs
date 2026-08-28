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

use dct_srv::{Config, Relay};

const DEFAULT_ADDR: &str = "127.0.0.1:8787";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_ADDR.into());

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local = listener.local_addr()?;
    if let Err(why) = dct_srv::must_be_loopback(local) {
        // 已经绑上了才发现——那就关掉。宁可启动失败，也不要一个没鉴权的
        // 中转在公网上多活一秒。
        drop(listener);
        return Err(why.into());
    }

    println!("dct-srv 在 http://{local} 上，只收本机的连接");
    dct_srv::serve(listener, Arc::new(Relay::new(Config::default()))).await?;
    Ok(())
}

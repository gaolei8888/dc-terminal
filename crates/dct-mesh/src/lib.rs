//! 多机之间互相认识需要的纯逻辑：机器 id 怎么从公钥算出来、配对时两台电脑
//! 各自读出的 6 位核对码、一台电脑的签名/加密钥匙对、以及组名单（谁在组
//! 里、各自的公钥）怎么签名和怎么校验一份新名单能不能取代旧的。
//!
//! **这个 crate 不碰网络、磁盘、环境变量，也不自己生成随机数**——种子、要签的
//! 消息、收到的名单，全部由调用方（`dct` 里的 `mesh` 模块）传进来。这样它才能
//! 被单元测试跑穷尽，不用起进程、不用碰真文件系统。
pub mod canon;
pub mod id;
pub mod invite;
pub mod keys;
pub mod relay_token;
pub mod roster;
pub mod sas;
pub mod seal;
pub mod wire;

pub use id::{AddrError, Address};
pub use keys::{KeyError, MachineKeys};
pub use roster::{Member, Roster, RosterError, SignedRoster};

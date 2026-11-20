//! 远程操作：一台电脑去操作同组另一台电脑上的会话。设计见
//! `docs/superpowers/specs/2026-11-19-dct-remote-control-design.md` 第 1、2 节。
//!
//! 这里只放「线上长什么样」和「这条请求让不让做」两件事，不碰网络、不碰会话：
//! - `CallBody` / `ReplyBody`：`Kind::RemoteCall` / `RemoteReply` 的 body；
//! - `capability_of`：请求 → 需要哪种能力。**白名单**，不在表里的一律 `None`；
//! - `allowed`：发件电脑 + 授权表 → 让不让做；
//! - `grants_for`：开关 → 授权表（本期开关是唯一来源）。
use std::path::{Path, PathBuf};

use dct_mesh::roster::Roster;
use serde::{Deserialize, Serialize};

use crate::proto::{ErrorCode, Request, Response};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    /// 看：名册、画面、项目列表。
    View,
    /// 动手：往会话里敲字、按键、滚动。
    Operate,
    /// 管：开会话、停会话、撤销、看改动。
    Manage,
}

/// 谁（发件电脑 endpoint）、哪个项目目录（`None` = 全部）、能做什么。
///
/// 第二期往表里加别人、加项目范围，`allowed` 不用改。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub who: String,
    pub project: Option<PathBuf>,
    pub caps: Vec<Capability>,
}

/// `RemoteCall` 的 body。`seq` 是操作方电脑上每次调用递增的序号：超时重试用
/// 同一个 `seq`，被操作方据此只执行一次（见设计第 1 节）。
#[derive(Debug, Serialize, Deserialize)]
pub struct CallBody {
    pub seq: u64,
    pub req: Request,
}

/// `RemoteReply` 的 body，`seq` 跟着它答的那条 `CallBody` 走。
#[derive(Debug, Serialize, Deserialize)]
pub struct ReplyBody {
    pub seq: u64,
    pub resp: Response,
}

/// 这条请求远程要哪种能力；`None` = 远程不可用。
///
/// **穷尽 `match`、不写 `_ =>`**：新加一个 `Request` 变体，这里编译不过，逼着
/// 加的人当场表态「远程能不能用」，而不是悄悄落进某个兜底分支。
/// 明确不给的：`Kill`（看不清对方状态时强杀代价大）、密钥、配对、手机、
/// 网页、直播、`Mesh*`、`Drain*`、`Pin*`（本机看板偏好），以及两条远程请求本身
/// （不许套娃，也不许别人替这台电脑开门）。
pub fn capability_of(req: &Request) -> Option<Capability> {
    use Capability::*;
    match req {
        Request::Hello
        | Request::List
        | Request::Screen { .. }
        | Request::Screens { .. }
        | Request::Explanation { .. }
        | Request::Projects
        | Request::Profiles { .. }
        // 开机编号：操作方靠它发现这台重启过、会话号重新数过了（`ui::remote::Ids`），
        // 不然它手里的旧号会指到别的会话上去。只读、不碰任何状态。
        | Request::Instance => Some(View),

        Request::Input { .. }
        | Request::Submit { .. }
        | Request::Key { .. }
        | Request::Mouse { .. }
        | Request::Scroll { .. }
        | Request::Resize { .. } => Some(Operate),

        Request::Create { .. }
        | Request::Stop { .. }
        | Request::Undo { .. }
        | Request::Diff { .. }
        | Request::LastProfile { .. } => Some(Manage),

        Request::Kill { .. }
        | Request::Prune
        | Request::PinProject { .. }
        | Request::UnpinProject { .. }
        | Request::SetSecret { .. }
        | Request::DeleteSecret { .. }
        | Request::VerifySecret { .. }
        | Request::PairStart { .. }
        | Request::PairPoll { .. }
        | Request::PairCancel { .. }
        | Request::PhoneStatus
        | Request::PhoneSetToken { .. }
        | Request::PhoneUnpair
        | Request::PhoneDisable
        | Request::WebStrings { .. }
        | Request::WebStatus
        | Request::WebEnable
        | Request::WebDisable
        | Request::LiveStart { .. }
        | Request::LiveRestage { .. }
        | Request::LiveStop
        | Request::LiveStatus
        | Request::LivePublish { .. }
        | Request::LiveUnpublish
        | Request::LivePublishGrant
        | Request::MeshStatus
        | Request::MeshLogin
        | Request::MeshJoin { .. }
        | Request::MeshInvite
        | Request::MeshInviteCancel
        | Request::MeshRemove { .. }
        | Request::MeshPeers
        | Request::MeshSend { .. }
        | Request::Drain { .. }
        | Request::DrainStatus { .. }
        | Request::DrainCancel
        | Request::RemoteCall { .. }
        | Request::RemoteControl { .. } => None,
    }
}

/// 发件电脑 `from` 能不能让这台电脑执行 `req`。
///
/// 错误码区分两种拒绝：根本没授权（开关关着、或发件人不在表里）报
/// `RemoteControlOff`——对方该去开开关；有授权但这件事不行（没有对应能力、
/// 不在白名单、`Create` 的目录不是已有项目）报 `RemoteForbidden`——开开关
/// 也没用，别误导人。
///
/// `Create` 的 `dir` 必须是 `projects`（这台电脑 `Projects` 里已有的项目）之一：
/// 远程不能在对方的任意目录里开会话。
pub fn allowed(
    req: &Request,
    grants: &[Grant],
    from: &str,
    projects: &[PathBuf],
) -> Result<(), ErrorCode> {
    let mine: Vec<&Grant> = grants.iter().filter(|g| g.who == from).collect();
    if mine.is_empty() {
        return Err(ErrorCode::RemoteControlOff);
    }
    let Some(cap) = capability_of(req) else {
        return Err(ErrorCode::RemoteForbidden);
    };
    // 本期授权表里只有 `project: None`。带项目范围的授权（第二期）只认
    // `Create`（按目录判）——按会话编号的请求这里没有「会话属于哪个项目」
    // 的信息，宁可拒绝也不放宽。
    let create_dir = match req {
        Request::Create { dir, .. } => Some(Path::new(dir)),
        _ => None,
    };
    let granted = mine.iter().any(|g| {
        g.caps.contains(&cap)
            && match (&g.project, create_dir) {
                (None, _) => true,
                (Some(p), Some(d)) => d.starts_with(p),
                (Some(_), None) => false,
            }
    });
    if !granted {
        return Err(ErrorCode::RemoteForbidden);
    }
    if let Some(dir) = create_dir {
        if !projects.iter().any(|p| p == dir) {
            return Err(ErrorCode::RemoteForbidden);
        }
    }
    Ok(())
}

/// 开关 → 授权表。开：名单上除自己（`me` 是自己的 endpoint）以外每台一条
/// 全能力、不限项目；关：空表，一切拒绝。
pub fn grants_for(switch_on: bool, roster: &Roster, me: &str) -> Vec<Grant> {
    if !switch_on {
        return Vec::new();
    }
    roster
        .members
        .iter()
        .filter(|m| m.endpoint != me)
        .map(|m| Grant {
            who: m.endpoint.clone(),
            project: None,
            caps: vec![Capability::View, Capability::Operate, Capability::Manage],
        })
        .collect()
}

/// 这条请求算不算「对方正在用我这台电脑」。看板为了画远程分组每 3 秒取一次
/// `List`，**不算**——否则对方只要开着 dct，这边的提示就永远亮着，人很快就不
/// 看了。真在用 = 取某个会话的画面，或任何会改东西的请求。
pub fn counts_as_presence(req: &Request) -> bool {
    matches!(req, Request::Screen { .. } | Request::Screens { .. })
        || matches!(
            capability_of(req),
            Some(Capability::Operate | Capability::Manage)
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ScrollBy;
    use dct_mesh::roster::Member;

    fn member(name: &str, endpoint: &str) -> Member {
        Member {
            name: name.into(),
            endpoint: endpoint.into(),
            sign_pub: String::new(),
            kx_pub: String::new(),
            added_at: 0,
        }
    }

    fn roster() -> Roster {
        Roster {
            group: "g".into(),
            version: 1,
            members: vec![member("mac", "c-mac"), member("pc", "c-pc")],
        }
    }

    fn create(dir: &str) -> Request {
        Request::Create {
            dir: dir.into(),
            profile: "p".into(),
            remember: false,
        }
    }

    #[test]
    fn every_whitelisted_request_maps_to_its_capability() {
        use Capability::*;
        let table: Vec<(Request, Capability)> = vec![
            (Request::Hello, View),
            (Request::List, View),
            (Request::Screen { id: 1 }, View),
            (Request::Screens { ids: vec![1] }, View),
            (Request::Explanation { id: 1 }, View),
            (Request::Projects, View),
            (Request::Instance, View),
            (
                Request::Profiles {
                    lang: crate::i18n::Lang::Zh,
                },
                View,
            ),
            (
                Request::Input {
                    id: 1,
                    text: "t".into(),
                },
                Operate,
            ),
            (
                Request::Submit {
                    id: 1,
                    text: "t".into(),
                },
                Operate,
            ),
            (
                Request::Key {
                    id: 1,
                    name: "Up".into(),
                },
                Operate,
            ),
            (
                Request::Mouse {
                    id: 1,
                    event: crate::proto::MouseForward {
                        col: 1,
                        row: 1,
                        kind: crate::proto::MouseForwardKind::WheelUp,
                        shift: false,
                        alt: false,
                        ctrl: false,
                    },
                },
                Operate,
            ),
            (
                Request::Scroll {
                    id: 1,
                    by: ScrollBy::Rows(3),
                },
                Operate,
            ),
            (
                Request::Resize {
                    id: 1,
                    rows: 2,
                    cols: 3,
                },
                Operate,
            ),
            (create("/p"), Manage),
            (Request::Stop { id: 1 }, Manage),
            (Request::Undo { id: 1 }, Manage),
            (Request::Diff { id: 1 }, Manage),
            (Request::LastProfile { dir: "/p".into() }, Manage),
        ];
        for (req, cap) in table {
            assert_eq!(capability_of(&req), Some(cap), "{req:?}");
        }
    }

    #[test]
    fn requests_off_the_list_are_refused() {
        let off: Vec<Request> = vec![
            Request::Kill { id: 1 },
            Request::Prune,
            Request::SetSecret {
                profile: "p".into(),
                value: "v".into(),
            },
            Request::PairStart {
                profile: "p".into(),
                opt_in_llm: false,
            },
            Request::PhoneUnpair,
            Request::WebEnable,
            Request::LiveStart {
                ids: vec![1],
                names: vec!["n".into()],
            },
            Request::MeshInvite,
            Request::Drain {
                wait_s: 1,
                nudge_wait_s: 1,
                initiator: None,
                now: false,
                exe: None,
            },
            Request::PinProject { dir: "/p".into() },
            Request::RemoteControl { set: Some(true) },
            Request::RemoteCall {
                peer: "pc".into(),
                req: Box::new(Request::List),
            },
        ];
        for req in off {
            assert_eq!(capability_of(&req), None, "{req:?}");
        }
    }

    #[test]
    fn switch_off_refuses_everything() {
        let grants = grants_for(false, &roster(), "c-mac");
        assert!(grants.is_empty());
        assert_eq!(
            allowed(&Request::List, &grants, "c-pc", &[]),
            Err(ErrorCode::RemoteControlOff)
        );
    }

    #[test]
    fn switch_on_grants_everyone_but_me() {
        let grants = grants_for(true, &roster(), "c-mac");
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].who, "c-pc");
        assert_eq!(allowed(&Request::List, &grants, "c-pc", &[]), Ok(()));
        assert_eq!(
            allowed(&Request::Stop { id: 1 }, &grants, "c-pc", &[]),
            Ok(())
        );
    }

    #[test]
    fn a_sender_not_on_the_roster_is_refused() {
        let grants = grants_for(true, &roster(), "c-mac");
        assert_eq!(
            allowed(&Request::List, &grants, "c-stranger", &[]),
            Err(ErrorCode::RemoteControlOff)
        );
        // 自己不给自己授权。
        assert_eq!(
            allowed(&Request::List, &grants, "c-mac", &[]),
            Err(ErrorCode::RemoteControlOff)
        );
    }

    #[test]
    fn off_the_list_or_missing_capability_is_forbidden() {
        let grants = grants_for(true, &roster(), "c-mac");
        assert_eq!(
            allowed(&Request::Kill { id: 1 }, &grants, "c-pc", &[]),
            Err(ErrorCode::RemoteForbidden)
        );
        let view_only = vec![Grant {
            who: "c-pc".into(),
            project: None,
            caps: vec![Capability::View],
        }];
        assert_eq!(
            allowed(
                &Request::Input {
                    id: 1,
                    text: "x".into()
                },
                &view_only,
                "c-pc",
                &[]
            ),
            Err(ErrorCode::RemoteForbidden)
        );
        assert_eq!(allowed(&Request::List, &view_only, "c-pc", &[]), Ok(()));
    }

    #[test]
    fn create_outside_the_peers_projects_is_forbidden() {
        let grants = grants_for(true, &roster(), "c-mac");
        let projects = vec![PathBuf::from("/work/a")];
        assert_eq!(
            allowed(&create("/etc"), &grants, "c-pc", &projects),
            Err(ErrorCode::RemoteForbidden)
        );
        assert_eq!(
            allowed(&create("/work/a"), &grants, "c-pc", &projects),
            Ok(())
        );
    }

    #[test]
    fn listing_is_not_presence_but_watching_and_typing_are() {
        assert!(!counts_as_presence(&Request::List));
        assert!(!counts_as_presence(&Request::Projects));
        assert!(!counts_as_presence(&Request::Hello));
        assert!(!counts_as_presence(&Request::Instance));
        assert!(counts_as_presence(&Request::Screen { id: 1 }));
        assert!(counts_as_presence(&Request::Screens { ids: vec![1] }));
        assert!(counts_as_presence(&Request::Input {
            id: 1,
            text: "x".into()
        }));
        assert!(counts_as_presence(&Request::Stop { id: 1 }));
    }

    #[test]
    fn call_and_reply_bodies_round_trip_through_json() {
        let call = CallBody {
            seq: 7,
            req: Request::Stop { id: 3 },
        };
        let back: CallBody = serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        assert_eq!(back.seq, 7);
        assert!(matches!(back.req, Request::Stop { id: 3 }));

        let reply = ReplyBody {
            seq: 7,
            resp: Response::Error(ErrorCode::RemoteForbidden),
        };
        let back: ReplyBody =
            serde_json::from_str(&serde_json::to_string(&reply).unwrap()).unwrap();
        assert_eq!(back.seq, 7);
        assert!(matches!(
            back.resp,
            Response::Error(ErrorCode::RemoteForbidden)
        ));
    }
}

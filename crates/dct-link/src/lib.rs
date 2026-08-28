//! 中转服务看得懂的**全部东西**。
//!
//! 这个 crate 的存在只有一个理由：`dct`（你笔记本上的守护进程）和 `dct-srv`
//! （公网上的中转）必须对同一个信封形状达成一致，而它俩谁也不该依赖谁。
//! 信封改一处，编译器把两边一起拦住。
//!
//! **这里没有 `Request` / `Response`。** spec 决定一说中转不解析它们——既然
//! 不解析，它就不需要这些类型，`src/proto.rs` 因此一行不动。中转对 `payload`
//! 的全部知识是「有这么多字节」。任何一天你在 `dct-srv` 里看见
//! `from_slice::<Request>`，那天这个设计就已经破了。
//!
//! 也因此这里**没有一句给人看的话**。错误是码，不是文案：中转不知道对面
//! 的人说中文还是英文，把「你的电脑离线了」写在这里，就等于把 i18n 搬到了
//! 一个看不见用户的进程里。翻译是 `dct` 和网页的事。

use serde::{Deserialize, Serialize};

/// 信封的线上契约版本。**改了信封就要加一。**
///
/// 跟 `PROTOCOL_VERSION` 是两个独立的数字，故意的：那个管界面和守护进程之间，
/// 这个管守护进程和中转之间。中转升级的时候，全世界的笔记本上跑着的是上周
/// 装的 dct；两边的版本对不上，`AuthFrame` 那一下就要说清楚，而不是等到某个
/// 字段解不出来才炸。
pub const LINK_VERSION: u32 = 1;

/// 一个 payload 最多多少字节。
///
/// 中转要在**解 base64 之前**就能拒绝，所以这条界限是给两边共用的常量而不是
/// 中转的私有配置。整屏画面带样式的 JSON 实测几十 KB，1 MiB 已经宽到不可能
/// 挡住正常使用；它挡的是「一条请求把中转的内存吃光」。
pub const MAX_PAYLOAD: usize = 1024 * 1024;

/// 端点 id 最长多少字符。见 `EndpointId` 上的注释。
pub const MAX_ENDPOINT_LEN: usize = 64;

/// 谁跟谁说话。**这是中转的路由键**，所以它是从网络上来的、不可信的字符串。
///
/// 长度和字符集都要卡住，理由不是洁癖：
///
/// - 不限长 = 设备表里的 key 可以任意长，一个循环脚本就能把中转的内存吃光。
/// - 不限字符集 = 控制字符和换行进得来。它今天只当 HashMap 的 key 没事，
///   明天有人把它写进日志、写进 HTTP 头、写进指标标签，那天就出事。**在
///   构造的地方拦住，比指望将来每一处使用都记得转义要可靠得多。**
///
/// 具体叫什么名字（设备是不是 uuid、手机那边怎么编）由 `dct-srv` 决定，
/// 这里只管「它是个合法的路由键」。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EndpointId(String);

/// 端点 id 不合法。**不带原文**：这个字符串是攻击者能控制的，往错误里一放
/// 就会跟着日志和响应到处跑。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadEndpoint {
    Empty,
    TooLong,
    /// 出现了 `[A-Za-z0-9_.:-]` 之外的字符。
    BadChar,
}

impl std::fmt::Display for BadEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BadEndpoint::Empty => "endpoint id is empty",
            BadEndpoint::TooLong => "endpoint id is too long",
            BadEndpoint::BadChar => "endpoint id has a character outside [A-Za-z0-9_.:-]",
        })
    }
}

impl std::error::Error for BadEndpoint {}

impl EndpointId {
    pub fn new(s: impl Into<String>) -> Result<Self, BadEndpoint> {
        let s = s.into();
        if s.is_empty() {
            return Err(BadEndpoint::Empty);
        }
        // 按字符数而不是字节数——虽然合法字符集全是 ASCII 两者相等，但先查
        // 长度再查字符集的话，一串多字节垃圾会先被判成 TooLong，报错就指错了
        // 地方。`chars().count()` 让两条判断各说各的。
        if s.chars().count() > MAX_ENDPOINT_LEN {
            return Err(BadEndpoint::TooLong);
        }
        if !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
        {
            return Err(BadEndpoint::BadChar);
        }
        Ok(EndpointId(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EndpointId {
    type Error = BadEndpoint;
    fn try_from(s: String) -> Result<Self, BadEndpoint> {
        EndpointId::new(s)
    }
}

impl From<EndpointId> for String {
    fn from(id: EndpointId) -> String {
        id.0
    }
}

impl std::fmt::Display for EndpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 第二期用：这份 payload 加密给了谁。第一期恒空。
///
/// **现在就留位置**，理由见 spec 决定二末尾：将来「让老师也能看学生的屏幕」
/// 如果发现信封只能有一个收件人，唯一的出路就是把加密关掉。多收件人这件事
/// 必须在结构里从第一天就成立。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PubKey(pub String);

/// 中转要转发的东西。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub from: EndpointId,
    pub to: EndpointId,
    /// 请求和答复配对用；同一个 `from` 上单调递增。
    pub seq: u64,
    /// 第一期是明文 JSON，第二期换成密文。**中转两期都不看它。**
    #[serde(with = "payload_b64")]
    pub payload: Vec<u8>,
    /// 第一期恒空。见 `PubKey`。
    #[serde(default)]
    pub recipients: Vec<PubKey>,
}

/// 一个端点是什么。
///
/// 中转拿它决定**用哪个验证器**去验 token：笔记本带的是配对 token，手机带的
/// 是 dc_classroom 的登录 token，两者的验法完全不同。不写这个字段的话中转
/// 只能两个验证器挨个试——拿凭据去猜，猜错一次就是一条无谓的失败验证。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointKind {
    /// 跑着 daemon 的那台电脑。
    Computer,
    /// 浏览器。
    Phone,
}

/// 接上中转时的第一帧：我是谁，我凭什么。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthFrame {
    /// 发这一帧的人以为的信封版本。对不上就让中转说清楚，别等某个字段解不
    /// 出来才炸。
    pub version: u32,
    pub kind: EndpointKind,
    pub endpoint: EndpointId,
    pub token: String,
}

/// 中转能回的坏消息，**全集**。
///
/// 是码不是话：翻译在 `dct` 和网页那边。这个枚举没有「其他错误」那一项，
/// 故意的——真出了没预料到的情况，中转应该多加一个变体（于是两边一起被
/// 编译器拦住），而不是把细节塞进一个自由文本里发到公网上。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkError {
    /// token 不对，或者这台设备已经被撤销了。
    Unauthorized,
    /// `AuthFrame.version` 跟中转的 `LINK_VERSION` 对不上。
    VersionMismatch,
    /// 收件人不在线。**不排队、不落盘**（spec「不做什么」第一条）。
    Offline,
    /// payload 超过 `MAX_PAYLOAD`。
    TooBig,
    /// 这个账号今天的额度用完了（任务 7）。
    QuotaExceeded,
    /// 想投给一个不属于自己账号的设备。
    NotYours,
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 给日志和 `Box<dyn Error>` 用的英文短码，不是给用户看的文案。
        f.write_str(match self {
            LinkError::Unauthorized => "unauthorized",
            LinkError::VersionMismatch => "link version mismatch",
            LinkError::Offline => "peer offline",
            LinkError::TooBig => "payload too big",
            LinkError::QuotaExceeded => "quota exceeded",
            LinkError::NotYours => "device belongs to another account",
        })
    }
}

impl std::error::Error for LinkError {}

/// `Vec<u8>` ⇄ base64 字符串。理由见 `Cargo.toml` 里 base64 那条注释。
mod payload_b64 {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        // 超长的先拦掉再解码：解码本身会分配 3/4 长度的内存，拿一条几百 MB
        // 的 base64 过来的话，等解完再判就已经晚了。
        if s.len() > super::MAX_PAYLOAD * 2 {
            return Err(serde::de::Error::custom("payload too big"));
        }
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Envelope {
        Envelope {
            from: EndpointId::new("laptop-1").unwrap(),
            to: EndpointId::new("phone:7").unwrap(),
            seq: 3,
            payload: b"hi".to_vec(),
            recipients: vec![],
        }
    }

    /// 信封的线上形状被钉在 `LINK_VERSION` 上。
    ///
    /// 这条测试是 `proto.rs::the_request_shape_is_pinned_to_the_protocol_version`
    /// 的同一招，理由也一样，而且这次更凶：中转升级和笔记本升级之间隔着的
    /// 不是「用户什么时候重启守护进程」，是「用户什么时候想起来更新 dct」。
    /// 形状变了而版本号没变，症状会是某个人的手机某天开始解不出画面，而他
    /// 那台电脑上的 dct 是三周前装的。
    #[test]
    fn the_envelope_shape_is_pinned_to_the_link_version() {
        let shape = serde_json::to_string(&sample()).unwrap();
        assert_eq!(
            (LINK_VERSION, shape.as_str()),
            (
                1,
                r#"{"from":"laptop-1","to":"phone:7","seq":3,"payload":"aGk=","recipients":[]}"#
            ),
            "信封的线上形状变了。把 LINK_VERSION 加一，再把这里的期望值更新成新的形状。"
        );
    }

    /// 鉴权帧和错误码同样是契约。错误码尤其是：中转发一个 `Offline` 出去，
    /// 手机上要显示「你的电脑离线了」——中间任何一个字母对不上，用户看到的
    /// 就是一句解析失败。
    #[test]
    fn the_auth_frame_and_the_error_codes_are_pinned_too() {
        let auth = AuthFrame {
            version: LINK_VERSION,
            kind: EndpointKind::Computer,
            endpoint: EndpointId::new("laptop-1").unwrap(),
            token: "t".into(),
        };
        assert_eq!(
            serde_json::to_string(&auth).unwrap(),
            r#"{"version":1,"kind":"Computer","endpoint":"laptop-1","token":"t"}"#
        );
        assert_eq!(
            serde_json::to_string(&EndpointKind::Phone).unwrap(),
            r#""Phone""#
        );

        // 每个变体都要在这里出现一次。
        let all = vec![
            LinkError::Unauthorized,
            LinkError::VersionMismatch,
            LinkError::Offline,
            LinkError::TooBig,
            LinkError::QuotaExceeded,
            LinkError::NotYours,
        ];
        assert_eq!(
            serde_json::to_string(&all).unwrap(),
            r#"["Unauthorized","VersionMismatch","Offline","TooBig","QuotaExceeded","NotYours"]"#
        );
    }

    #[test]
    fn an_envelope_survives_the_round_trip() {
        let e = sample();
        let back: Envelope = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }

    /// payload 是**字节**，不是文本。第二期的密文里什么都有，包括 0 和不是
    /// 合法 UTF-8 的序列——这条测试是「别哪天顺手把它改成 String」的锁。
    #[test]
    fn the_payload_carries_bytes_that_are_not_text() {
        let e = Envelope {
            payload: vec![0, 0xff, 0xfe, b'\n'],
            ..sample()
        };
        let back: Envelope = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.payload, vec![0, 0xff, 0xfe, b'\n']);
    }

    /// 路由键是从网络上来的。构造的地方不拦，就等着将来每一处使用都记得转义。
    #[test]
    fn a_routing_key_that_could_hurt_someone_is_refused() {
        assert_eq!(EndpointId::new(""), Err(BadEndpoint::Empty));
        assert_eq!(
            EndpointId::new("a".repeat(MAX_ENDPOINT_LEN + 1)),
            Err(BadEndpoint::TooLong)
        );
        assert!(EndpointId::new("a".repeat(MAX_ENDPOINT_LEN)).is_ok());
        for bad in ["a\nb", "a b", "a/b", "a\0b", "设备"] {
            assert_eq!(
                EndpointId::new(bad),
                Err(BadEndpoint::BadChar),
                "{bad:?} 不该被当成合法路由键"
            );
        }
        assert!(EndpointId::new("phone:7").is_ok());
        assert!(EndpointId::new("laptop-1_a.b").is_ok());
    }

    /// **解 JSON 也得走同一道检查。** 校验只写在 `new()` 上是不够的——中转
    /// 拿到的每一个 id 都是 serde 造出来的，那条路绕过校验的话，上面那条
    /// 测试就是在保护一个没人走的入口。
    #[test]
    fn the_same_check_applies_to_ids_that_arrive_as_json() {
        let json = r#"{"from":"a b","to":"x","seq":0,"payload":"","recipients":[]}"#;
        assert!(serde_json::from_str::<Envelope>(json).is_err());

        let long = format!(
            r#"{{"from":"{}","to":"x","seq":0,"payload":"","recipients":[]}}"#,
            "a".repeat(MAX_ENDPOINT_LEN + 1)
        );
        assert!(serde_json::from_str::<Envelope>(&long).is_err());
    }

    /// 旧的中转发来的信封没有 `recipients`（第一期它恒空，很可能有人图省事
    /// 不发）。这个字段带 `#[serde(default)]`，所以缺了要补成空表而不是解析
    /// 失败。
    #[test]
    fn an_envelope_without_recipients_still_parses() {
        let e: Envelope =
            serde_json::from_str(r#"{"from":"a","to":"b","seq":1,"payload":"aGk="}"#).unwrap();
        assert!(e.recipients.is_empty());
        assert_eq!(e.payload, b"hi");
    }

    /// 一条几百 MB 的 base64 不该先被解出来再被判定太大。
    #[test]
    fn an_absurd_payload_is_refused_before_it_is_decoded() {
        let huge = format!(
            r#"{{"from":"a","to":"b","seq":1,"payload":"{}"}}"#,
            "A".repeat(MAX_PAYLOAD * 2 + 4)
        );
        assert!(serde_json::from_str::<Envelope>(&huge).is_err());
    }
}

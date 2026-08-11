#[cfg(test)]
mod tests {
    use super::*;

    /// 真实回包的形状。少一个字段就该是 Malformed，不该 panic、不该猜。
    #[test]
    fn parses_a_normal_update() {
        let body = r#"{"ok":true,"result":[{"update_id":1,"message":{
            "message_id":42,"chat":{"id":777},"text":"先跑完"}}]}"#;
        let got = parse_updates(body).expect("正常回包该解得出来");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "先跑完");
        assert_eq!(got[0].chat_id, 777);
        assert_eq!(got[0].reply_to, None);
    }

    #[test]
    fn picks_up_the_message_being_replied_to() {
        let body = r#"{"ok":true,"result":[{"update_id":2,"message":{
            "message_id":43,"chat":{"id":777},"text":"就第二个",
            "reply_to_message":{"message_id":42}}}]}"#;
        let got = parse_updates(body).unwrap();
        assert_eq!(got[0].reply_to, Some(42));
    }

    /// 没有 text 的更新（图片、贴纸、有人进群）不是错误，是「这条没什么可读的」。
    /// 当成 Malformed 会让一张图片害得整轮轮询失败。
    #[test]
    fn updates_without_text_are_skipped_not_errors() {
        let body = r#"{"ok":true,"result":[{"update_id":3,"message":{
            "message_id":44,"chat":{"id":777}}}]}"#;
        assert_eq!(parse_updates(body).unwrap().len(), 0);
    }

    /// 令牌被撤销时 Telegram 回 ok:false + 401。**必须区分出 BadToken**，
    /// 否则退避重试会永远转下去，用户永远等不到「去重填令牌」这句话。
    #[test]
    fn a_revoked_token_is_bad_token_not_unreachable() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(parse_updates(body), Err(ChannelError::BadToken));
    }

    #[test]
    fn garbage_is_malformed() {
        assert_eq!(
            parse_updates("not json at all"),
            Err(ChannelError::Malformed)
        );
    }

    #[test]
    fn reads_the_new_message_id_back() {
        let body = r#"{"ok":true,"result":{"message_id":99,"chat":{"id":777}}}"#;
        assert_eq!(parse_send_result(body), Ok(99));
    }

    /// getMe 用来验证令牌，同时拿到 bot 用户名——界面上要显示
    /// 「在 Telegram 里搜 @your_bot」，没有这个名字那句话就没法写。
    #[test]
    fn get_me_returns_the_bot_username() {
        let body = r#"{"ok":true,"result":{"id":1,"is_bot":true,"username":"my_dct_bot"}}"#;
        assert_eq!(parse_get_me(body).unwrap(), "my_dct_bot");
    }

    #[test]
    fn get_me_with_a_bad_token_says_bad_token() {
        let body = r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(parse_get_me(body), Err(ChannelError::BadToken));
    }
}

//! M3b 要用的标签函数从 crate 外可达（M3a.1）。
use crewd_collab::render::{safe_name, sender_label};

#[test]
fn safe_name_and_sender_label_are_public() {
    assert_eq!(safe_name("  【甲】\n乙\u{202e}  "), "「甲」 乙");
    let _: fn(&rusqlite::Connection, &crewd_proto::Message, crewd_collab::Locale) -> _ =
        sender_label;
}

/// M3b 的机长工具要用同一个权限判断（N1b：总机组的机长在每个机组都按机长算），所以它从 crate 外可达；
/// 不认识的 session / 机组 = 没有权限，不是错误。
#[test]
fn acts_as_captain_of_is_public_and_unknowns_are_not_captains() {
    let store = crewd_store::Store::open_in_memory().unwrap();
    assert!(!crewd_collab::crews::acts_as_captain_of(store.conn(), "nobody", "no-crew").unwrap());
}

/// M3b `start_session` refuses with collab's own title rule before it starts anything (Q2): the
/// rule `sessions::register` applies is reachable from outside the crate, codes included.
#[test]
fn resolve_title_is_public_and_is_registers_rule() {
    use crewd_collab::CollabError;
    use crewd_collab::refusal::Code;
    use crewd_collab::sessions::resolve_title;
    let code = |r: crewd_collab::Result<String>| match r {
        Err(CollabError::Invalid(r)) => r.code,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        resolve_title(Some(" 修\u{200B}滚动 "), None).unwrap(),
        "修滚动"
    );
    assert_eq!(
        resolve_title(
            None,
            Some("\n补齐示例模块的单元测试，覆盖边界情况和错误路径\n第二行")
        )
        .unwrap(),
        "补齐示例模块的单元测试，覆盖边界情况"
    );
    assert_eq!(
        code(resolve_title(Some(&"长".repeat(19)), Some("x"))),
        Code::SessionTitleTooLong
    );
    assert_eq!(
        code(resolve_title(None, Some(" \n "))),
        Code::SessionTitleOrBriefRequired
    );
    assert_eq!(
        code(resolve_title(Some("人类"), None)),
        Code::SessionTitleIsSenderLabel
    );
}

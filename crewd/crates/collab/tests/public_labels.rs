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

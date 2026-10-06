//! M3b 要用的标签函数从 crate 外可达（M3a.1）。
use crewd_collab::render::{safe_name, sender_label};

#[test]
fn safe_name_and_sender_label_are_public() {
    assert_eq!(safe_name("  【甲】\n乙\u{202e}  "), "「甲」 乙");
    let _: fn(&rusqlite::Connection, &crewd_proto::Message, crewd_collab::Locale) -> _ =
        sender_label;
}

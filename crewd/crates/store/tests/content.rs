use crewd_proto::{
    Attachment, Crew, Ledger, Mention, MentionKind, Message, SenderKind, Todo, TodoResponse,
};
use crewd_store::channels::{ensure_lobby, lobby_id};
use crewd_store::{Store, archive, crews, messages, todos};

fn store_with_crew() -> Store {
    let s = Store::open_in_memory().unwrap();
    crews::upsert(
        s.conn(),
        &Crew {
            id: "a".into(),
            title: "A".into(),
            number: None,
            org_root: false,
            parent_ids: vec![],
            working_directory: None,
            captain_executor: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    ensure_lobby(s.conn(), "a", "2026-10-01T00:00:00.000Z").unwrap();
    s
}

fn msg(id: &str, text: &str) -> Message {
    Message {
        seq: 0,
        id: id.into(),
        channel_id: lobby_id("a"),
        sender_kind: SenderKind::Human,
        sender_session_id: None,
        sender_name: Some("我".into()),
        text: text.into(),
        category: None,
        in_reply_to: None,
        mentions: vec![Mention {
            kind: MentionKind::Broadcast,
            target_id: None,
        }],
        attachments: vec![Attachment {
            id: "f1".into(),
            mime: "text/plain".into(),
            size: Some(3),
            path: "/x/a.txt".into(),
            filename: None,
        }],
        created_at: "2026-10-01T00:00:00.000Z".into(),
    }
}

#[test]
fn messages_insert_assigns_increasing_seq_and_ignores_duplicates() {
    let s = store_with_crew();
    let a = messages::insert(s.conn(), &msg("m1", "一"), None)
        .unwrap()
        .unwrap();
    let b = messages::insert(s.conn(), &msg("m2", "二"), None)
        .unwrap()
        .unwrap();
    assert!(b > a);
    assert_eq!(
        messages::insert(s.conn(), &msg("m1", "重复"), None).unwrap(),
        None
    );
}

#[test]
fn messages_list_after_seq_with_limit_roundtrips_unicode() {
    let s = store_with_crew();
    let text = "中文 + emoji 👋🏽 + 换行\n第二行";
    let first = messages::insert(s.conn(), &msg("m1", text), None)
        .unwrap()
        .unwrap();
    messages::insert(s.conn(), &msg("m2", "二"), None).unwrap();
    messages::insert(s.conn(), &msg("m3", "三"), None).unwrap();
    let all = messages::list(s.conn(), &lobby_id("a"), 0, None, 10).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].text, text);
    assert_eq!(all[0].seq, first);
    assert_eq!(all[0].mentions[0].kind, MentionKind::Broadcast);
    assert_eq!(all[0].attachments[0].size, Some(3));
    let tail = messages::list(s.conn(), &lobby_id("a"), first, None, 1).unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].id, "m2");
    assert!(
        messages::list(s.conn(), &lobby_id("other"), 0, None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn todos_insert_and_list_with_responses_in_order() {
    let s = store_with_crew();
    let t = Todo {
        id: "t1".into(),
        crew_id: "a".into(),
        ledger: Ledger::Agent,
        number: 2,
        text: "发版".into(),
        status: "in_progress".into(),
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: None,
        deleted_at: None,
        attachments: vec![],
        responses: vec![
            TodoResponse {
                id: "r1".into(),
                session_id: "captain-1".into(),
                sender_name: None,
                text: "收到".into(),
                status: Some("in_progress".into()),
                attachments: vec![],
                created_at: "2026-10-01T00:01:00.000Z".into(),
            },
            TodoResponse {
                id: "r2".into(),
                session_id: "worker-1".into(),
                sender_name: Some("w".into()),
                text: "做到一半".into(),
                status: None,
                attachments: vec![],
                created_at: "2026-10-01T00:02:00.000Z".into(),
            },
        ],
    };
    let mut earlier = t.clone();
    earlier.id = "t0".into();
    earlier.number = 1;
    earlier.responses.clear();
    earlier.deleted_at = Some("2026-10-02T00:00:00.000Z".into());
    todos::insert(s.conn(), &t, None).unwrap();
    todos::insert(s.conn(), &earlier, None).unwrap();
    let list = todos::list(s.conn(), "a", Ledger::Agent).unwrap();
    assert_eq!(list.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 2]);
    assert!(list[0].deleted_at.is_some());
    assert_eq!(
        list[1]
            .responses
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["r1", "r2"]
    );
    assert!(
        todos::list(s.conn(), "a", Ledger::Human)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn todos_duplicate_number_in_same_ledger_is_unique_violation() {
    let s = store_with_crew();
    let mk = |id: &str| Todo {
        id: id.into(),
        crew_id: "a".into(),
        ledger: Ledger::Human,
        number: 1,
        text: "x".into(),
        status: "pending".into(),
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: None,
        deleted_at: None,
        attachments: vec![],
        responses: vec![],
    };
    todos::insert(s.conn(), &mk("t1"), None).unwrap();
    let err = todos::insert(s.conn(), &mk("t2"), None).unwrap_err();
    match err {
        crewd_store::StoreError::Sql(e) => assert!(crewd_store::is_unique_violation(&e)),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn archive_roundtrip() {
    let s = store_with_crew();
    archive::put(s.conn(), "whiteboards/a.plan.json", b"[1]").unwrap();
    assert_eq!(
        archive::get(s.conn(), "whiteboards/a.plan.json")
            .unwrap()
            .unwrap(),
        b"[1]"
    );
    assert_eq!(archive::get(s.conn(), "nope").unwrap(), None);
    assert_eq!(
        archive::list_paths(s.conn()).unwrap(),
        ["whiteboards/a.plan.json"]
    );
}

#[test]
fn todos_insert_is_atomic_when_a_response_fails() {
    let s = store_with_crew();
    let resp = |id: &str| TodoResponse {
        id: id.into(),
        session_id: "s".into(),
        sender_name: None,
        text: "x".into(),
        status: None,
        attachments: vec![],
        created_at: "2026-10-01T00:01:00.000Z".into(),
    };
    let t = Todo {
        id: "t1".into(),
        crew_id: "a".into(),
        ledger: Ledger::Agent,
        number: 1,
        text: "x".into(),
        status: "pending".into(),
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: None,
        deleted_at: None,
        attachments: vec![],
        responses: vec![resp("dup"), resp("dup")],
    };
    assert!(todos::insert(s.conn(), &t, None).is_err());
    assert!(
        todos::list(s.conn(), "a", Ledger::Agent)
            .unwrap()
            .is_empty()
    );
    let n: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM todo_responses", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

/// 严格只管客户端参数（控制者裁定，Task 14 复审）：库里存的 @ / 附件多一个键，照样读得出来，不会让整个群列不出来。
#[test]
fn stored_mentions_and_attachments_with_extra_keys_still_list() {
    let s = store_with_crew();
    messages::insert(s.conn(), &msg("m1", "一"), None).unwrap();
    s.conn()
        .execute(
            r#"UPDATE messages SET mentions='[{"kind":"human","label":"@人"}]',
               attachments='[{"id":"f1","mime":"image/png","path":"/x/a.png","width":3}]' WHERE id='m1'"#,
            [],
        )
        .unwrap();
    let all = messages::list(s.conn(), &lobby_id("a"), 0, None, 10).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].mentions[0].kind, MentionKind::Human);
    assert_eq!(all[0].attachments[0].path, "/x/a.png");
}

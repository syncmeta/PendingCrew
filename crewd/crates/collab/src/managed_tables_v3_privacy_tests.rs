use super::*;
#[test]
fn history_links_revalidate_b15_without_revealing_hidden_target_identity_or_count() {
    let mut s = fixture();
    let channel = ok(&mut s, |tx| {
        crate::channels::create(
            tx,
            &crate::Sender::Human,
            crate::channels::NewChannel {
                crew_id: "crew-x".into(),
                kind: crewd_proto::ChannelKind::Topic,
                name: "Private evidence".into(),
                ..Default::default()
            },
        )
    });
    let mut messages = Vec::new();
    for text in ["Synthetic secret alpha", "Synthetic secret beta"] {
        let m = ok(&mut s, |tx| {
            crate::messages::post(
                tx,
                crate::Sender::Human,
                crate::messages::NewPost {
                    channel_id: channel.id.clone(),
                    text: text.into(),
                    ..Default::default()
                },
            )
        });
        messages.push(m.message.id);
    }
    let table = create(
        &mut s,
        HUMAN,
        "History source visibility",
        vec![
            json!({"kind":"text"}),
            json!({"kind":"link","target":{"kind":"message"},"many":true}),
        ],
        "ownerCrewWriters",
    );
    let t = get(s.conn(), HUMAN, &table, X).unwrap();
    let request = request(
        &s,
        json!({"kind":"addRow","tableId":table,"expectedSchemaRevision":t.schema_revision,"patches":[set("p1",json!({"type":"text","value":"Public source row"})),{"fieldId":"p2","change":{"kind":"replaceLinks","values":messages.iter().map(|id|json!({"kind":"reference","target":{"kind":"message","id":id}})).collect::<Vec<_>>(),"keepHiddenSetToken":null}}]}),
    );
    let committed = ok(&mut s, |tx| submit(tx, HUMAN, request, X));
    let Effect::RowAdded { row_id, .. } = &committed.effect else {
        panic!("row")
    };
    assert!(!crate::channels::readable_by(s.conn(), &channel.id, "w-a1").unwrap());
    let q = HistoryValue {
        source: HistorySource::Row { id: row_id.clone() },
        key: HistoryKey::Event {
            event_id: committed.event_id.clone().unwrap(),
        },
        path: HistoryPath::Links {
            field_id: "p2".into(),
            side: HistorySide::After,
        },
        cursor: None,
    };
    let before = database_snapshot(&s);
    let physical = s.conn().total_changes();
    let restricted = history_value(s.conn(), AUTHOR, q.clone(), X).unwrap();
    assert!(
        matches!(&restricted,ValueChunk::Links{visible,restricted_details:true,next_cursor:None,..}if visible.is_empty())
    );
    let encoded = serde_json::to_string(&restricted).unwrap();
    for id in &messages {
        assert!(!encoded.contains(id));
    }
    assert!(!encoded.contains("Synthetic secret"));
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), physical);
    ok(&mut s, |tx| {
        crate::channels::add_members(
            tx,
            &crate::Sender::Human,
            &channel.id,
            &[crewd_proto::MemberRef::session("w-a1")],
        )
    });
    let visible = history_value(s.conn(), AUTHOR, q.clone(), X).unwrap();
    assert!(
        matches!(&visible,ValueChunk::Links{visible,restricted_details:false,..}if visible.len()==2)
    );
    ok(&mut s, |tx| {
        crate::channels::remove_member(
            tx,
            &crate::Sender::Human,
            &channel.id,
            &crewd_proto::MemberRef::session("w-a1"),
        )
    });
    let before = database_snapshot(&s);
    let physical = s.conn().total_changes();
    let restricted = history_value(s.conn(), AUTHOR, q, X).unwrap();
    assert!(
        matches!(&restricted,ValueChunk::Links{visible,restricted_details:true,..}if visible.is_empty())
    );
    for id in &messages {
        assert!(!serde_json::to_string(&restricted).unwrap().contains(id));
    }
    assert_eq!(database_snapshot(&s), before);
    assert_eq!(s.conn().total_changes(), physical);
}

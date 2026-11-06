//! Manual Human tasks freeze with their crew; only actual Agent questions withdraw.
use crate::{
    Change, Sender, asks, crews,
    testkit::{NOW, count, database_snapshot, ok, ok_at},
    todo_attributes, wakes,
};
use crewd_proto::{
    ChannelWorkLinkParams, HumanTaskCreateParams, HumanTaskCreateResult, ManualTodoState,
    MemberRef, ResourceRequestLookupParams, TodoOrigin, TodoSetStateParams,
};
use crewd_store::Store;

fn manual(s: &mut Store, crew: &str) -> (HumanTaskCreateParams, HumanTaskCreateResult) {
    let p = HumanTaskCreateParams {
        request_id: uuid::Uuid::new_v4().to_string(),
        crew_id: crew.into(),
        channel_id: format!("{crew}/lobby"),
        text: format!("Synthetic Human task in {crew}"),
    };
    let result = ok(s, |tx| {
        todo_attributes::create_human(tx, &Sender::Human, p.clone())
    });
    (p, result)
}
fn agent_question(s: &mut Store, session: &str) -> String {
    ok(s, |tx| {
        asks::open(
            tx,
            session,
            asks::OpenAsk {
                question: "Synthetic Agent question".into(),
                default: None,
                resume_note: None,
            },
        )
    })
    .ask
}
fn core_task(s: &Store, crew: &str, id: &str) -> crewd_work::HumanAsk {
    let reference = asks::ask_reference(s.conn(), crew, id).unwrap();
    let core = crewd_store::work_organization::core_for_reference(
        s.conn(),
        crewd_store::object_refs::ObjectKind::HumanTask,
        reference.number,
    )
    .unwrap()
    .unwrap()
    .parse()
    .unwrap();
    asks::load_org(s.conn())
        .unwrap()
        .ask(core)
        .cloned()
        .unwrap()
}
fn receipt_bytes(s: &Store, id: &str) -> String {
    s.conn()
        .query_row(
            "SELECT result FROM resource_mutation_requests WHERE request_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}
fn state(s: &Store, task: &HumanTaskCreateResult) -> crewd_proto::TodoAttributes {
    todo_attributes::get(s.conn(), &Sender::Human, &task.todo.object).unwrap()
}
fn change_state(
    s: &mut Store,
    task: &HumanTaskCreateResult,
    value: ManualTodoState,
) -> crate::Result<(crewd_proto::TodoAttributes, Vec<Change>)> {
    let p = TodoSetStateParams {
        request_id: uuid::Uuid::new_v4().to_string(),
        object: task.todo.object.clone(),
        channel_id: task.channel_association.channel_id.clone(),
        expected_revision: state(s, task).revision,
        state: value,
    };
    crate::run(s, NOW + 400, |tx| {
        todo_attributes::set_state(tx, &Sender::Human, p)
    })
}
fn pending(s: &mut Store) -> i64 {
    match ok(s, |tx| {
        wakes::enqueue(
            tx,
            wakes::WakeRequest {
                session: "w-b1".into(),
                kind: wakes::WakeKind::Timer,
                source: wakes::source::timer("manual-archive-rollback"),
                priority: wakes::WakePriority::System,
                not_before_ms: None,
                note: None,
                origin: None,
            },
        )
    }) {
        wakes::Enqueued::New(id) => id,
        other => panic!("{other:?}"),
    }
}
fn wake_state(s: &Store, id: i64) -> wakes::WakeState {
    wakes::list(s.conn(), Some("w-b1"), 50)
        .unwrap()
        .into_iter()
        .find(|w| w.id == id)
        .unwrap()
        .state
}
#[test]
fn archive_freezes_manual_task_preserves_original_uuid_and_purge_only_removes_owned_sources() {
    let mut s = crate::testkit::store();
    let (p, h) = manual(&mut s, "crew-b");
    let (foreign_p, foreign) = manual(&mut s, "crew-x");
    let q = agent_question(&mut s, "w-b1");
    let foreign_q = agent_question(&mut s, "cap-x");
    let original = core_task(&s, "crew-b", &h.ask.ask);
    let foreign_original = core_task(&s, "crew-x", &foreign.ask.ask);
    assert!(original.human_task);
    assert_eq!(original.asked_by, crewd_work::Actor::Human);
    assert_eq!(original.state, crewd_work::AskState::Open);
    assert!(original.default.is_none());
    assert_eq!(h.todo.origin, TodoOrigin::HumanTask);
    assert_eq!(h.todo.owner, Some(MemberRef::Human));
    let bytes = receipt_bytes(&s, &p.request_id);
    let foreign_bytes = receipt_bytes(&s, &foreign_p.request_id);
    // A doomed channel references another crew's real task. Removing that
    // channel must never remove or resolve the shared foreign source.
    let relation = crate::channel_work::association(
        s.conn(),
        &Sender::Human,
        "crew-b/lobby",
        &foreign.todo.object,
    )
    .unwrap();
    ok(&mut s, |tx| {
        crate::channel_work::link(
            tx,
            &Sender::Human,
            ChannelWorkLinkParams {
                channel_id: "crew-b/lobby".into(),
                expected_link_revision: relation.link_revision,
                object: foreign.todo.object.clone(),
            },
            true,
        )
    });
    let wake = pending(&mut s);
    let (_, changes) = ok_at(&mut s, NOW + 100, |tx| crews::archive(tx, "crew-b"));
    assert!(crews::get(s.conn(), "crew-b").unwrap().removed_at.is_some());
    assert_eq!(core_task(&s, "crew-b", &h.ask.ask), original);
    assert_eq!(state(&s, &h), h.todo);
    assert_eq!(receipt_bytes(&s, &p.request_id), bytes);
    assert_eq!(
        asks::get(s.conn(), "crew-b", &q).unwrap().state,
        crewd_proto::AskStateView::Withdrawn
    );
    assert!(
        !changes
            .iter()
            .any(|c| matches!(c,Change::AskChanged{ask,..} if ask==&h.ask.ask))
    );
    assert!(
        changes
            .iter()
            .any(|c| matches!(c,Change::AskChanged{ask,..} if ask==&q))
    );
    assert_eq!(wake_state(&s, wake), wakes::WakeState::Dropped);
    let recovered = todo_attributes::lookup(
        s.conn(),
        &Sender::Human,
        ResourceRequestLookupParams {
            request_id: p.request_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(recovered.status, "committed");
    assert_eq!(
        recovered.result,
        Some(serde_json::from_str(&bytes).unwrap())
    );
    let frozen = database_snapshot(&s);
    assert!(change_state(&mut s, &h, ManualTodoState::Done).is_err());
    assert_eq!(database_snapshot(&s), frozen);
    ok_at(&mut s, NOW + 200, |tx| crews::unarchive(tx, "crew-b"));
    assert_eq!(core_task(&s, "crew-b", &h.ask.ask), original);
    assert_eq!(state(&s, &h), h.todo);
    assert_eq!(
        asks::get(s.conn(), "crew-b", &q).unwrap().state,
        crewd_proto::AskStateView::Withdrawn
    );
    assert_eq!(wake_state(&s, wake), wakes::WakeState::Dropped);
    // Restoration really restores editability, without answering the manual H.
    assert_eq!(
        change_state(&mut s, &h, ManualTodoState::Done)
            .unwrap()
            .0
            .manual_state,
        Some(ManualTodoState::Done)
    );
    change_state(&mut s, &h, ManualTodoState::Open).unwrap();
    assert_eq!(core_task(&s, "crew-b", &h.ask.ask), original);
    assert_eq!(receipt_bytes(&s, &p.request_id), bytes);
    let events = count(&s, "SELECT count(*) FROM organization_work_events");
    ok_at(&mut s, NOW + 500, |tx| crews::archive(tx, "crew-b"));
    assert_eq!(
        count(&s, "SELECT count(*) FROM organization_work_events"),
        events,
        "second archive must not withdraw the Agent question again"
    );
    ok_at(&mut s, NOW + 600, |tx| crews::purge(tx, "crew-b"));
    assert!(asks::get(s.conn(), "crew-b", &h.ask.ask).is_err());
    assert!(asks::get(s.conn(), "crew-b", &q).is_err());
    assert_eq!(
        todo_attributes::lookup(
            s.conn(),
            &Sender::Human,
            ResourceRequestLookupParams {
                request_id: p.request_id
            }
        )
        .unwrap()
        .status,
        "notFound"
    );
    assert_eq!(core_task(&s, "crew-x", &foreign.ask.ask), foreign_original);
    assert_eq!(state(&s, &foreign), foreign.todo);
    assert_eq!(receipt_bytes(&s, &foreign_p.request_id), foreign_bytes);
    assert_eq!(
        asks::get(s.conn(), "crew-x", &foreign_q).unwrap().state,
        crewd_proto::AskStateView::Open
    );
    assert!(
        crate::channel_work::association(
            s.conn(),
            &Sender::Human,
            "crew-x/lobby",
            &foreign.todo.object
        )
        .unwrap()
        .linked
    );
}
#[test]
fn archive_late_failure_rolls_back_real_agent_withdraw_manual_source_receipt_events_and_wake_drop()
{
    let mut s = crate::testkit::store();
    let (p, h) = manual(&mut s, "crew-b");
    let q = agent_question(&mut s, "w-b1");
    let wake = pending(&mut s);
    let original = core_task(&s, "crew-b", &h.ask.ask);
    let bytes = receipt_bytes(&s, &p.request_id);
    s.conn().execute_batch("CREATE TRIGGER synthetic_archive_fault BEFORE UPDATE OF archived_at ON crews WHEN NEW.id='crew-b' AND NEW.archived_at IS NOT NULL BEGIN SELECT RAISE(ABORT,'synthetic archive write fault'); END;").unwrap();
    let before = database_snapshot(&s);
    let error = crate::run(&mut s, NOW + 100, |tx| crews::archive(tx, "crew-b")).unwrap_err();
    assert!(
        format!("{error:?}").contains("synthetic archive write fault"),
        "must reach the failure after actual Agent withdrawal: {error:?}"
    );
    assert_eq!(
        database_snapshot(&s),
        before,
        "all staged facts, original receipts, events and queued-wake transitions must roll back"
    );
    assert_eq!(core_task(&s, "crew-b", &h.ask.ask), original);
    assert_eq!(receipt_bytes(&s, &p.request_id), bytes);
    assert_eq!(
        asks::get(s.conn(), "crew-b", &q).unwrap().state,
        crewd_proto::AskStateView::Open
    );
    assert_eq!(wake_state(&s, wake), wakes::WakeState::Pending);
    s.conn()
        .execute_batch("DROP TRIGGER synthetic_archive_fault")
        .unwrap();
    ok_at(&mut s, NOW + 200, |tx| crews::archive(tx, "crew-b"));
    assert_eq!(core_task(&s, "crew-b", &h.ask.ask), original);
    assert_eq!(wake_state(&s, wake), wakes::WakeState::Dropped);
    assert_eq!(
        asks::get(s.conn(), "crew-b", &q).unwrap().state,
        crewd_proto::AskStateView::Withdrawn
    );
}

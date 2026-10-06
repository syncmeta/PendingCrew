//! spec §1 验收判据的纯逻辑版：只用公开 API，按时间顺序走一遍「派 → 整理 → 检查点 → 卡在人身上
//! 只卡那一件 → 人回复直接叫醒负责人 → 带证据完成」，以及失联重派、重启（落库再读回）不丢到期检查。
use crewd_work::*;
use std::collections::BTreeSet;

const T0: Ts = Ts(1_700_000_000_000);

fn sid(s: &str) -> SessionId {
    SessionId::new(s)
}

fn go(next: Ts, note: &str) -> Report {
    Report {
        status: Some(Progress::Active),
        next_check_at: Some(next),
        note: note.into(),
    }
}

fn woken_owners(events: &[Event]) -> Vec<SessionId> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::BlockerCleared { owner, .. } => owner.clone(),
            _ => None,
        })
        .collect()
}

#[test]
fn todo_flows_from_dispatch_to_evidence_without_a_human_nudge() {
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    let a = Actor::worker("a");
    b.register_entry(T0, EntryNo(1)).unwrap();
    assert_eq!(
        b.entry_view(EntryNo(1)).unwrap().status,
        EntryStatus::Unclaimed
    );

    // 调度层派给 a：占位项「现在就查」。
    let t = T0.plus(MINUTE);
    let placeholder = b
        .claim_entry(
            &Actor::System,
            t,
            EntryNo(1),
            vec![NewItem::titled("整理 #1").owned_by("a").check_at(t)],
        )
        .unwrap()
        .value[0];
    let due = b.due_checkpoints(t);
    assert_eq!(
        due,
        [CheckpointDue {
            owner: sid("a"),
            items: vec![placeholder]
        }]
    );
    b.record_checkpoint_wake(&Actor::System, t, &[placeholder])
        .unwrap();

    // a 醒来，把入口整理成两项（前端依赖后端），占位项拆成它们的父项。
    let t = t.plus(MINUTE);
    let created = b
        .create_item(
            &a,
            t,
            NewItem::titled("后端")
                .under(ItemRef::Id(placeholder))
                .owned_by("a"),
        )
        .unwrap()
        .value;
    let backend = created;
    let frontend = b
        .create_item(
            &a,
            t,
            NewItem::titled("前端")
                .under(ItemRef::Id(placeholder))
                .after(ItemRef::Id(backend))
                .owned_by("a"),
        )
        .unwrap()
        .value;
    b.report(&a, t, placeholder, go(t.plus(2 * HOUR), "拆成后端、前端"))
        .unwrap();
    b.report(&a, t, backend, go(t.plus(HOUR), "先改 schema"))
        .unwrap();
    assert_eq!(b.item(placeholder).unwrap().missed_checks, 0, "回报清零");

    // 后端卡在人身上：只卡这一件，能先做的拆出来。
    let t = t.plus(10 * MINUTE);
    let blocked = b
        .block(
            &a,
            t,
            backend,
            vec![BlockOn::NewAsk {
                question: "旧数据要不要迁？".into(),
                default: None,
            }],
            Continuation::Split(vec![NewItem::titled("新表结构（不依赖答案）")]),
        )
        .unwrap()
        .value;
    let ask = blocked.asks[0];
    let part = blocked.split[0];
    assert_eq!(
        b.load_of(&sid("a")),
        3,
        "blocked 的后端不算负载：占位项、前端、拆出的部分"
    );

    // a 停下：拆出的部分还没排期 → 立刻再叫一次。
    assert_eq!(
        b.on_idle(&sid("a"), t),
        StopDecision::Rewake(vec![StopViolation {
            item: part,
            kind: StopKind::ReadyUnscheduled
        }])
    );
    // 同一小时内 a 推进了别的工作。
    let t = t.plus(5 * MINUTE);
    b.report(&a, t, part, go(t.plus(HOUR), "建新表")).unwrap();
    assert_eq!(b.on_idle(&sid("a"), t), StopDecision::Clean);

    // 人回复 → 直接叫醒负责人，后端回到 active、立刻到期。
    let t = t.plus(20 * MINUTE);
    let answered = b.answer_ask(&Actor::Human, t, ask, "要迁").unwrap();
    assert_eq!(woken_owners(&answered.events), [sid("a")]);
    assert_eq!(b.item(backend).unwrap().status, Status::Active);
    assert!(
        b.due_checkpoints(t)
            .iter()
            .any(|d| d.items.contains(&backend))
    );

    // 带证据收尾：子项先完，父项才完；入口跟着完成。
    let t = t.plus(30 * MINUTE);
    b.mark_done(&a, t, part, Evidence::Commit("1111111".into()))
        .unwrap();
    b.mark_done(&a, t, backend, Evidence::Commit("2222222".into()))
        .unwrap();
    b.report(&a, t, frontend, go(t.plus(HOUR), "后端好了，开前端"))
        .unwrap();
    b.mark_done(
        &a,
        t,
        frontend,
        Evidence::Url("https://example.invalid/pr/1".into()),
    )
    .unwrap();
    assert_eq!(
        b.entry_view(EntryNo(1)).unwrap().status,
        EntryStatus::InProgress
    );
    b.mark_done(&a, t, placeholder, Evidence::Text("前后端均已合入".into()))
        .unwrap();
    let view = b.entry_view(EntryNo(1)).unwrap();
    assert_eq!(
        (view.status, view.done, view.items.len()),
        (EntryStatus::Done, 1, 1)
    );

    // 事件序号连续、无重复。
    let seqs: Vec<u64> = b.log().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=b.state().last_seq).collect::<Vec<u64>>());
}

#[test]
fn silent_owner_is_replaced_and_the_new_owner_is_due_at_once() {
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    let w = b
        .claim_entry(
            &Actor::worker("a"),
            T0,
            EntryNo(1),
            vec![NewItem::titled("x")],
        )
        .unwrap()
        .value[0];
    b.report(&Actor::worker("a"), T0, w, go(T0.plus(HOUR), "开工"))
        .unwrap();
    let none = BTreeSet::new();
    let mut t = T0.plus(HOUR);
    for _ in 0..2 {
        let due = b.due_checkpoints(t);
        assert_eq!(due.len(), 1);
        b.record_checkpoint_wake(&Actor::System, t, &due[0].items)
            .unwrap();
        t = t.plus(15 * MINUTE);
    }
    let lost = b.owner_lost(t, &none);
    assert_eq!(
        lost,
        [LostOwner {
            item: w,
            owner: sid("a"),
            cause: LostCause::Silent { missed: 2 }
        }]
    );
    b.reassign_lost(&Actor::System, t, w, sid("b"), "a 两次检查没回报")
        .unwrap();
    assert_eq!(
        b.due_checkpoints(t),
        [CheckpointDue {
            owner: sid("b"),
            items: vec![w]
        }]
    );
    assert_eq!(b.owner_lost(t, &none), []);
}

#[test]
fn restart_loses_no_due_check_or_open_question() {
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    let ids = b
        .claim_entry(
            &Actor::worker("a"),
            T0,
            EntryNo(1),
            vec![NewItem::titled("x"), NewItem::titled("y")],
        )
        .unwrap()
        .value;
    b.report(&Actor::worker("a"), T0, ids[0], go(T0.plus(HOUR), "开工"))
        .unwrap();
    b.block(
        &Actor::worker("a"),
        T0,
        ids[1],
        vec![BlockOn::NewAsk {
            question: "q".into(),
            default: Some(DefaultOption {
                text: "按 A".into(),
                after_ms: 2 * HOUR,
            }),
        }],
        Continuation::Nothing,
    )
    .unwrap();

    // 「落库」= 序列化整份状态；「重启」= 读回。
    let saved = serde_json::to_string(b.state()).unwrap();
    let later = T0.plus(3 * HOUR);
    let back = WorkBook::restore(
        serde_json::from_str(&saved).unwrap(),
        Config::default(),
        later,
    )
    .unwrap();
    assert_eq!(back.due_checkpoints(later), b.due_checkpoints(later));
    assert_eq!(back.due_checkpoints(later)[0].items, [ids[0]]);
    assert_eq!(back.defaults_due(later), [AskId(1)]);
    assert_eq!(back.human_waits(later), b.human_waits(later));
}

use crewd_work::*;
use std::collections::BTreeMap;
const NOW: Ts = Ts(1_700_000_000_000);
fn actor(id: &str, home: &str, roles: &[(&str, Role)]) -> Actor {
    Actor::scoped(
        id,
        AuthorityScope {
            home: CrewId::new(home),
            roles: roles.iter().map(|(c, r)| (CrewId::new(*c), *r)).collect(),
        },
    )
}
fn item(crew: &str, title: &str, owner: &str) -> NewItem {
    let mut n = NewItem::titled(title).owned_by(owner);
    n.crew_id = Some(CrewId::new(crew));
    n
}
fn note() -> Report {
    Report {
        status: None,
        next_check_at: None,
        note: "progress".into(),
    }
}

#[test]
fn captain_authority_is_per_object_even_after_a_global_merge() {
    let mut b = WorkBook::organization(Config::default());
    let a = b
        .create_item(&Actor::Human, NOW, item("a", "a work", "wa"))
        .unwrap()
        .value;
    let x = b
        .create_item(&Actor::Human, NOW, item("b", "b work", "wb"))
        .unwrap()
        .value;
    let captain = actor("ca", "a", &[("a", Role::Captain)]);
    b.report(&captain, NOW, a, note()).unwrap();
    let before = b.state().clone();
    assert_eq!(
        b.report(&captain, NOW, x, note()),
        Err(Rejected::Forbidden(Action::Report))
    );
    assert_eq!(
        b.report(&Actor::captain("legacy-unscoped"), NOW, x, note()),
        Err(Rejected::Forbidden(Action::Report))
    );
    assert_eq!(b.state(), &before);
    b.report(
        &actor("chief", "a", &[("a", Role::Captain), ("b", Role::Captain)]),
        NOW,
        x,
        note(),
    )
    .unwrap();
    b.report(&actor("wb", "a", &[]), NOW, x, note()).unwrap(); // ownership survives home changes
    assert_eq!(b.state().item_crew(x), Some(&CrewId::new("b")));
}

#[test]
fn parent_dependency_directions_and_depth_remain_distinct_across_crews() {
    let mut b = WorkBook::organization(Config::default());
    let root = b
        .create_item(&Actor::Human, NOW, item("a", "root", "a"))
        .unwrap()
        .value;
    let child = b
        .create_item(
            &Actor::Human,
            NOW,
            item("b", "child", "b").under(ItemRef::Id(root)),
        )
        .unwrap()
        .value;
    // A parent waiting for its child is intentional, not a directed cycle.
    b.relink(&Actor::Human, NOW, root, None, vec![child])
        .unwrap();
    let before = b.state().clone();
    assert!(matches!(
        b.relink(&Actor::Human, NOW, child, Some(root), vec![root]),
        Err(Rejected::Invariant(Violation::Cycle(_)))
    ));
    assert_eq!(b.state(), &before);
    let third = b
        .create_item(
            &Actor::Human,
            NOW,
            item("a", "third", "a").under(ItemRef::Id(child)),
        )
        .unwrap()
        .value;
    let before = b.state().clone();
    assert!(matches!(
        b.create_item(
            &Actor::Human,
            NOW,
            item("b", "fourth", "b").under(ItemRef::Id(third))
        ),
        Err(Rejected::Invariant(Violation::ParentDepth {
            depth: 4,
            limit: 3,
            ..
        }))
    ));
    assert_eq!(b.state(), &before);
    let mut previous = third;
    for i in 0..8 {
        previous = b
            .create_item(
                &Actor::Human,
                NOW,
                item("b", &format!("dep {i}"), "b").after(ItemRef::Id(previous)),
            )
            .unwrap()
            .value;
    }
}

#[test]
fn cross_crew_partial_human_and_item_release_preserves_other_blockers() {
    let mut b = WorkBook::organization(Config::default());
    let a = actor("wa", "a", &[]);
    let other = actor("wb", "b", &[]);
    let origin = b
        .create_item(&Actor::Human, NOW, item("a", "origin", "wa"))
        .unwrap()
        .value;
    let dependency = b
        .create_item(&Actor::Human, NOW, item("b", "dependency", "wb"))
        .unwrap()
        .value;
    let blocked = b
        .block(
            &a,
            NOW,
            origin,
            vec![
                BlockOn::Item(dependency),
                BlockOn::NewAsk {
                    question: "decision".into(),
                    default: None,
                },
            ],
            Continuation::Split(vec![NewItem::titled("independent part")]),
        )
        .unwrap()
        .value;
    assert_eq!(
        b.state().item_crew(blocked.split[0]),
        Some(&CrewId::new("a"))
    );
    assert_eq!(b.state().ask_crew(blocked.asks[0]), Some(&CrewId::new("a")));
    let released = b.mark_done(&other, NOW, dependency, None).unwrap();
    assert_eq!(released.value, [origin]);
    assert_eq!(b.item(origin).unwrap().status, Status::Blocked);
    assert_eq!(
        b.item(origin).unwrap().blocked_on,
        [Blocker::Human(blocked.asks[0])]
    );
    assert!(released.events.iter().any(|e| matches!(&e.kind, EventKind::BlockerCleared { id, owner:Some(o), status:Status::Blocked, .. } if *id==origin && *o==SessionId::new("wa"))));
    b.answer_ask(&Actor::Human, NOW, blocked.asks[0], "yes")
        .unwrap();
    assert_eq!(b.item(origin).unwrap().status, Status::Active);
    assert_eq!(b.item(origin).unwrap().next_check_at, Some(NOW));
    b.mark_done(&a, NOW, blocked.split[0], None).unwrap();
    b.mark_done(&a, NOW, origin, None).unwrap();
}

#[test]
fn source_status_and_claim_scope_follow_entry_crew_not_actor_home() {
    let mut b = WorkBook::organization(Config::default());
    b.register_entry_in(NOW, EntryNo(1), CrewId::new("a"))
        .unwrap();
    let worker = actor("wb", "b", &[]);
    let w = b
        .claim_entry(
            &worker,
            NOW,
            EntryNo(1),
            vec![NewItem::titled("cross crew claim")],
        )
        .unwrap()
        .value[0];
    assert_eq!(b.state().item_crew(w), Some(&CrewId::new("a")));
    let cap = actor("cb", "b", &[("b", Role::Captain)]);
    assert_eq!(
        b.close_entry(&cap, NOW, EntryNo(1), "close", None),
        Err(Rejected::Forbidden(Action::CloseEntryDecision))
    );
    b.mark_done(&worker, NOW, w, None).unwrap();
    assert_eq!(b.entry_view(EntryNo(1)).unwrap().status, EntryStatus::Done);
}

fn legacy(crew: &str) -> BookState {
    let mut b = WorkBook::new(CrewId::new(crew), Config::default());
    b.register_entry(NOW, EntryNo(1)).unwrap();
    let w = b
        .claim_entry(
            &Actor::worker("same-owner"),
            NOW,
            EntryNo(1),
            vec![NewItem::titled("original")],
        )
        .unwrap()
        .value[0];
    let h = b
        .block(
            &Actor::worker("same-owner"),
            NOW,
            w,
            vec![BlockOn::NewAsk {
                question: "original ask".into(),
                default: None,
            }],
            Continuation::Nothing,
        )
        .unwrap()
        .value
        .asks[0];
    assert_eq!(h, AskId(1));
    b.state().clone()
}

#[test]
fn legacy_collisions_remap_every_reference_without_fabricating_events() {
    let mut a = legacy("a");
    let mut b = legacy("b");
    a.last_item = 4;
    a.last_ask = 3; // preserve historically issued holes
    a.stop_streaks.insert(
        SessionId::new("same-owner"),
        StopStreak {
            last: vec![StopViolation {
                item: WorkId(1),
                kind: StopKind::Unanswered,
            }],
            unimproved: 2,
            rewakes: 4,
            escalated: false,
        },
    );
    b.stop_streaks.insert(
        SessionId::new("same-owner"),
        StopStreak {
            last: vec![StopViolation {
                item: WorkId(1),
                kind: StopKind::Overdue,
            }],
            unimproved: 1,
            rewakes: 3,
            escalated: true,
        },
    );
    b.clock = NOW.plus(3000);
    let merged = merge_legacy_books(vec![b.clone(), a.clone()]).unwrap();
    assert_eq!(
        merged,
        merge_legacy_books(vec![a.clone(), b.clone()]).unwrap()
    );
    let bm = &merged.identities[&CrewId::new("b")];
    assert_eq!(bm.items[&WorkId(1)], WorkId(5));
    assert_eq!(bm.asks[&AskId(1)], AskId(4));
    assert_eq!(bm.entries[&EntryNo(1)], EntryNo(2));
    let it = &merged.state.items[&WorkId(5)];
    assert_eq!(it.sources, [EntryNo(2)]);
    assert_eq!(it.blocked_on, [Blocker::Human(AskId(4))]);
    assert_eq!(merged.state.asks[&AskId(4)].item, Some(WorkId(5)));
    assert_eq!(it.owner_history, b.items[&WorkId(1)].owner_history);
    assert_eq!(it.revision, b.items[&WorkId(1)].revision);
    assert_eq!(merged.state.last_seq, 0);
    assert_eq!(merged.legacy_sequences[&CrewId::new("b")], b.last_seq);
    let streak = &merged.state.stop_streaks[&SessionId::new("same-owner")];
    assert_eq!(
        (streak.unimproved, streak.rewakes, streak.escalated),
        (2, 4, true)
    );
    assert_eq!(streak.last.len(), 2);
    let decoded: BookState =
        serde_json::from_str(&serde_json::to_string(&merged.state).unwrap()).unwrap();
    let mut restored = WorkBook::restore(decoded, Config::default()).unwrap();
    assert!(restored.log().is_empty());
    assert_eq!(
        restored
            .create_item(&Actor::Human, NOW, item("b", "new", "same-owner"))
            .unwrap()
            .value,
        WorkId(6)
    );
}

#[test]
fn migration_rejects_invalid_depth_missing_references_and_duplicate_crews() {
    let a = legacy("a");
    assert!(matches!(
        merge_legacy_books(vec![a.clone(), a]),
        Err(MigrationError::DuplicateCrew(_))
    ));
    let mut deep = WorkBook::new(CrewId::new("deep"), Config::default());
    let mut p = None;
    for _ in 0..4 {
        let mut n = NewItem::titled("level");
        n.parent = p.map(ItemRef::Id);
        p = Some(deep.create_item(&Actor::Human, NOW, n).unwrap().value);
    }
    assert!(
        matches!(merge_legacy_books(vec![deep.state().clone()]),Err(MigrationError::InvalidBook { crew, violation:Violation::ParentDepth{depth:4,..} }) if crew==CrewId::new("deep"))
    );
    let mut bad = legacy("bad");
    bad.stop_streaks.insert(
        SessionId::new("s"),
        StopStreak {
            last: vec![StopViolation {
                item: WorkId(999),
                kind: StopKind::Overdue,
            }],
            ..StopStreak::default()
        },
    );
    assert!(matches!(
        merge_legacy_books(vec![bad]),
        Err(MigrationError::InvalidBook { .. })
    ));
    let mut bad = legacy("bad");
    bad.items
        .get_mut(&WorkId(1))
        .unwrap()
        .sources
        .push(EntryNo(999));
    assert!(matches!(
        merge_legacy_books(vec![bad]),
        Err(MigrationError::InvalidBook { .. })
    ));
}

#[test]
fn scoped_admin_can_only_close_foreign_owned_work_with_facts_and_in_scope() {
    let mut b = WorkBook::organization(Config::default());
    let w = b
        .create_item(&Actor::Human, NOW, item("a", "work", "owner"))
        .unwrap()
        .value;
    let local = actor("admin", "a", &[("a", Role::Admin)]);
    assert_eq!(
        b.mark_done(&local, NOW, w, None),
        Err(Rejected::Blank("evidence"))
    );
    assert_eq!(
        b.mark_done(
            &actor("admin-b", "b", &[("b", Role::Admin)]),
            NOW,
            w,
            Some(Evidence::Commit("proof".into()))
        ),
        Err(Rejected::Forbidden(Action::Done))
    );
    b.mark_done(&local, NOW, w, Some(Evidence::Commit("proof".into())))
        .unwrap();
}

#[test]
fn organization_restore_requires_exact_nonblank_attribution_and_fixed_depth() {
    let mut b = WorkBook::organization(Config::default());
    let w = b
        .create_item(&Actor::Human, NOW, item("a", "work", "owner"))
        .unwrap()
        .value;
    let mut state = b.state().clone();
    state.organization.as_mut().unwrap().item_crews = BTreeMap::new();
    assert_eq!(
        WorkBook::restore(state, Config::default()).unwrap_err(),
        Violation::OrganizationOwnership
    );
    let mut state = b.state().clone();
    state
        .organization
        .as_mut()
        .unwrap()
        .item_crews
        .insert(w, CrewId::new(" "));
    assert_eq!(
        WorkBook::restore(state, Config::default()).unwrap_err(),
        Violation::OrganizationOwnership
    );
}

#[test]
fn split_and_ask_follow_source_object_when_owner_home_differs() {
    let mut b = WorkBook::organization(Config::default());
    let w = b
        .create_item(&Actor::Human, NOW, item("b", "foreign owned work", "wa"))
        .unwrap()
        .value;
    let r = b
        .block(
            &actor("wa", "a", &[]),
            NOW,
            w,
            vec![BlockOn::NewAsk {
                question: "question".into(),
                default: None,
            }],
            Continuation::Split(vec![NewItem::titled("part")]),
        )
        .unwrap()
        .value;
    assert_eq!(b.state().item_crew(r.split[0]), Some(&CrewId::new("b")));
    assert_eq!(b.state().ask_crew(r.asks[0]), Some(&CrewId::new("b")));
    // A's captain cannot withdraw B's question even once B stops waiting.
    b.report(
        &actor("wa", "a", &[]),
        NOW,
        w,
        Report {
            status: Some(Progress::Open),
            next_check_at: None,
            note: "workaround".into(),
        },
    )
    .unwrap();
    assert_eq!(
        b.withdraw_ask(
            &actor("ca", "a", &[("a", Role::Captain)]),
            NOW,
            r.asks[0],
            "cancel"
        ),
        Err(Rejected::Forbidden(Action::WithdrawAsk))
    );
    b.withdraw_ask(
        &actor("cb", "b", &[("b", Role::Captain)]),
        NOW,
        r.asks[0],
        "cancel",
    )
    .unwrap();
}

#[test]
fn migration_retains_parent_dependency_pending_transfer_and_raw_history() {
    let mut b = WorkBook::new(CrewId::new("b"), Config::default());
    let parent = b
        .create_item(
            &Actor::Human,
            NOW,
            NewItem::titled("parent").owned_by("owner"),
        )
        .unwrap()
        .value;
    let child = b
        .create_item(
            &Actor::Human,
            NOW,
            NewItem::titled("child")
                .owned_by("owner")
                .under(ItemRef::Id(parent)),
        )
        .unwrap()
        .value;
    b.relink(&Actor::Human, NOW, parent, None, vec![child])
        .unwrap();
    b.request_transfer(
        &Actor::worker("owner"),
        NOW,
        parent,
        SessionId::new("next"),
        "explicit transfer reason",
    )
    .unwrap();
    let original = b.state().clone();
    let merged = merge_legacy_books(vec![legacy("a"), original.clone()]).unwrap();
    let map = &merged.identities[&CrewId::new("b")];
    let translated_parent = &merged.state.items[&map.items[&parent]];
    let translated_child = &merged.state.items[&map.items[&child]];
    assert_eq!(translated_parent.depends_on, [map.items[&child]]);
    assert_eq!(translated_child.parent, Some(map.items[&parent]));
    assert_eq!(
        translated_parent.pending_transfer,
        original.items[&parent].pending_transfer
    );
    assert_eq!(
        translated_parent.owner_history,
        original.items[&parent].owner_history
    );
    assert_eq!(translated_parent.body, original.items[&parent].body);
}

#[test]
fn exhausted_identity_ranges_fail_without_partial_changes_or_number_reuse() {
    let mut a = legacy("a");
    a.last_item = u32::MAX;
    assert!(matches!(
        merge_legacy_books(vec![a.clone(), legacy("b")]),
        Err(MigrationError::Exhausted { kind: "W", .. })
    ));
    let mut b = WorkBook::restore(a, Config::default()).unwrap();
    let before = b.state().clone();
    assert_eq!(
        b.create_item(&Actor::Human, NOW, NewItem::titled("overflow")),
        Err(Rejected::Invalid("work identity range exhausted"))
    );
    assert_eq!(b.state(), &before);
    let mut s = legacy("a");
    s.last_ask = u32::MAX;
    let mut b = WorkBook::restore(s, Config::default()).unwrap();
    let before = b.state().clone();
    assert_eq!(
        b.open_ask(&Actor::worker("owner"), NOW, "overflow", None, None),
        Err(Rejected::Invalid("ask identity range exhausted"))
    );
    assert_eq!(b.state(), &before);
}

#[test]
fn legacy_books_never_silently_accept_foreign_attribution() {
    let mut b = WorkBook::new(CrewId::new("a"), Config::default());
    assert_eq!(
        b.create_item(&Actor::Human, NOW, item("b", "foreign", "owner")),
        Err(Rejected::Invalid(
            "cross-crew work requires organization authority"
        ))
    );
    assert!(b.state().items.is_empty());
    assert_eq!(
        b.register_entry(NOW, EntryNo(0)),
        Err(Rejected::Invalid("entry number must be positive"))
    );
}

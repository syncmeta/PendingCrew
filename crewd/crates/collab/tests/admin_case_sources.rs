//! Durable administrative sources through the real stop-check transaction seam.
//! Wake ACKs here exercise the collab DAO only; these are not runner delivery tests.
use crewd_collab::{self as collab, Sender, Tx, stop_checks, wakes, work};
use crewd_proto::{Crew, Desired, Role, Session, WorkCreateParams, WorkPlan};
use crewd_store::Store;
const NOW: i64 = 1_790_000_000_000;
fn fixture() -> Store {
    populate(Store::open_in_memory().unwrap())
}
fn populate(s: Store) -> Store {
    for (id, number, management) in [
        ("crew-a", Some(1), false),
        ("crew-b", Some(2), false),
        ("chief", None, true),
    ] {
        crewd_store::crews::upsert(
            s.conn(),
            &Crew {
                id: id.into(),
                title: id.into(),
                number,
                management,
                working_directory: Some("/tmp/synthetic-admin-source".into()),
                created_at: "2026-09-01T00:00:00.000Z".into(),
                updated_at: "2026-09-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    for (id, crew, role, ext) in [
        ("cap-a", "crew-a", Role::Captain, None),
        ("worker-a", "crew-a", Role::Worker, Some(2)),
        ("cap-b", "crew-b", Role::Captain, None),
    ] {
        crewd_store::sessions::upsert(
            s.conn(),
            &Session {
                id: id.into(),
                crew_id: crew.into(),
                role,
                executor: "claude".into(),
                desired: Desired::Running,
                ext,
                display_name: Some(id.into()),
                updated_at: "2026-09-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    crewd_store::schema::backfill_v4(s.conn()).unwrap();
    s
}
fn tx<T>(s: &mut Store, f: impl FnOnce(&mut Tx<'_>) -> collab::Result<T>) -> T {
    collab::run(s, NOW, f).unwrap().0
}
fn seed(s: &mut Store, crew: &str, title: &str) -> WorkPlan {
    tx(s, |t| {
        work::create(
            t,
            &Sender::Human,
            WorkCreateParams {
                crew_id: crew.into(),
                owner: Some("worker-a".into()),
                title: title.into(),
                ..Default::default()
            },
        )
    })
}
fn escalation_candidate(s: &mut Store) -> stop_checks::Candidate {
    escalation_candidate_for(s, "worker-a")
}
fn escalation_candidate_for(s: &mut Store, owner: &str) -> stop_checks::Candidate {
    for _ in 0..3 {
        let c = stop_checks::candidate(s.conn(), owner, NOW)
            .unwrap()
            .unwrap();
        let stop_checks::Accepted::Rewake(id) = tx(s, |t| stop_checks::accept(t, &c)) else {
            panic!("expected original stop reminder")
        };
        tx(s, |t| {
            wakes::claim(t, &[id])?;
            wakes::ack_delivered(t, owner, &[id], wakes::Riders::Release, &[], &[])
        })
    }
    stop_checks::candidate(s.conn(), owner, NOW)
        .unwrap()
        .unwrap()
}
fn snapshot(s: &Store) -> Vec<String> {
    let names = s
        .conn()
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut result = vec![];
    for name in names {
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let mut q = s
            .conn()
            .prepare(&format!("SELECT * FROM \"{name}\""))
            .unwrap();
        let n = q.column_count();
        let mut rows = q
            .query_map([], |r| {
                let mut row = vec![];
                for i in 0..n {
                    row.push(format!("{:?}", r.get_ref(i)?));
                }
                Ok(row.join("|"))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows.sort();
        result.push(name);
        result.extend(rows);
    }
    result
}

#[test]
fn escalation_transaction_failure_rolls_back_budget_announcement_and_source() {
    let mut s = fixture();
    seed(&mut s, "crew-a", "rollback source");
    let c = escalation_candidate(&mut s);
    let before = snapshot(&s);
    let failed: collab::Result<((), Vec<collab::Change>)> = collab::run(&mut s, NOW, |t| {
        assert_eq!(
            stop_checks::accept(t, &c)?,
            stop_checks::Accepted::Escalated
        );
        Err(collab::resource_invalid("synthetic downstream failure"))
    });
    assert!(failed.is_err());
    assert_eq!(snapshot(&s), before);
    assert!(cases(&s, "crew-a")["items"].as_array().unwrap().is_empty());
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &c)),
        stop_checks::Accepted::Escalated
    );
    assert_eq!(cases(&s, "crew-a")["items"].as_array().unwrap().len(), 1);
}
#[test]
fn unchanged_summary_with_new_original_work_revision_invalidates_captured_source() {
    let mut s = fixture();
    let w = seed(&mut s, "crew-a", "revision witness");
    let c = escalation_candidate(&mut s);
    let prior = serde_json::to_value(&c).unwrap();
    let current = tx(&mut s, |t| {
        work::update(
            t,
            &Sender::Human,
            crewd_proto::WorkUpdateParams {
                id: w.id,
                expected_revision: w.revision,
                note: Some("new report without fixing missing next check".into()),
                ..Default::default()
            },
        )
    });
    let next = stop_checks::candidate(s.conn(), "worker-a", NOW)
        .unwrap()
        .unwrap();
    let observed = serde_json::to_value(&next).unwrap();
    assert_eq!(prior["targets"][0]["id"], observed["targets"][0]["id"]);
    assert_eq!(prior["targets"][0]["kind"], observed["targets"][0]["kind"]);
    assert_ne!(
        prior["targets"][0]["revision"],
        observed["targets"][0]["revision"]
    );
    assert_eq!(observed["targets"][0]["revision"], current.revision);
    assert_eq!(
        prior["expected_epoch"], observed["expected_epoch"],
        "only source revision changes, not the stop budget witness"
    );
    let before = snapshot(&s);
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &c)),
        stop_checks::Accepted::Stale
    );
    assert_eq!(snapshot(&s), before);
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &next)),
        stop_checks::Accepted::Escalated
    );
    assert_eq!(
        cases(&s, "crew-a")["items"][0]["targets"][0]["revision"],
        current.revision
    );
}

fn cases(s: &Store, crew: &str) -> serde_json::Value {
    serde_json::to_value(
        collab::admin_cases::list(
            s.conn(),
            &crewd_proto::admin_cases::AdminCaseListParams {
                crew_id: crew.into(),
                after: None,
                limit: Some(100),
            },
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn full_targets_exceed_summary_cap_and_repeated_escalation_creates_one_source() {
    let mut s = fixture();
    let mut expected = std::collections::BTreeMap::new();
    for i in 0..55 {
        let w = seed(&mut s, "crew-a", &format!("complete source target {i}"));
        expected.insert(w.id, w.revision);
    }
    let c = escalation_candidate(&mut s);
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &c)),
        stop_checks::Accepted::Escalated
    );
    let page = cases(&s, "crew-a");
    let items = page["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "one actual escalation must persist one source: {page}"
    );
    let case = &items[0];
    assert_eq!(case["decision"], "observed");
    assert_eq!(case["execution"], "not_started");
    assert_eq!(case["retirement"], "not_started");
    let targets = case["targets"].as_array().unwrap();
    assert_eq!(
        targets.len(),
        55,
        "human announcement summary must not truncate durable targets"
    );
    let actual: std::collections::BTreeMap<_, _> = targets
        .iter()
        .map(|v| {
            (
                v["id"].as_str().unwrap().to_owned(),
                v["revision"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(case["source"]["sessionId"], "worker-a");
    assert_eq!(
        case["source"]["budgetEpoch"],
        crewd_store::work_stop_checks::get(s.conn(), "worker-a")
            .unwrap()
            .unwrap()
            .epoch
    );
    let announcement = case["source"]["announcementId"].as_str().unwrap();
    let message = collab::messages::get(s.conn(), announcement).unwrap();
    assert_eq!(message.category.as_deref(), Some("work_stop_escalation"));
    let summary: serde_json::Value =
        serde_json::from_str(&message.text[message.text.find('{').unwrap()..]).unwrap();
    assert_eq!(summary["stopWork"].as_array().unwrap().len(), 50);
    assert_eq!(summary["total"], 55);
    assert_eq!(summary["partial"], true);
    let before = snapshot(&s);
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &c)),
        stop_checks::Accepted::Stale
    );
    assert_eq!(snapshot(&s), before);
    assert!(
        stop_checks::candidate(s.conn(), "worker-a", NOW)
            .unwrap()
            .is_none()
    );
    assert_eq!(cases(&s, "crew-a"), page);
}
#[test]
fn foreign_work_is_retained_but_explicitly_outside_case_crew() {
    let mut s = fixture();
    let own = seed(&mut s, "crew-a", "same crew");
    let foreign = seed(&mut s, "crew-b", "foreign crew same owner");
    let c = escalation_candidate(&mut s);
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &c)),
        stop_checks::Accepted::Escalated
    );
    let page = cases(&s, "crew-a");
    let rows = page["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let case = &rows[0];
    assert_eq!(case["crewId"], "crew-a");
    let targets = case["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    let a = targets.iter().find(|t| t["id"] == own.id).unwrap();
    let b = targets.iter().find(|t| t["id"] == foreign.id).unwrap();
    assert_eq!(a["crewId"], "crew-a");
    assert_eq!(a["scope"], "in_scope");
    assert_eq!(b["crewId"], "crew-b");
    assert_eq!(b["scope"], "out_of_scope");
    assert_eq!(
        cases(&s, "crew-b")["items"].as_array().unwrap().len(),
        0,
        "cross-crew target must not invent a separate authorized admin case"
    );
    let query = crewd_proto::admin_cases::AdminCaseGetParams {
        crew_id: "crew-b".into(),
        id: case["id"].as_str().unwrap().into(),
    };
    assert!(
        collab::admin_cases::get(s.conn(), &query).is_err(),
        "explicit crew mismatch must not expose a case as local authority"
    );
}

fn source_metadata(s: &Store) -> Vec<(String, String)> {
    s.conn()
        .prepare("SELECT key,value FROM meta WHERE key LIKE 'admin.case.%' ORDER BY key")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
#[test]
fn file_database_reopen_preserves_source_index_and_next_monotonic_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin-sources.sqlite");
    let mut s = populate(Store::open(&path).unwrap());
    seed(&mut s, "crew-a", "durable first escalation");
    let original = escalation_candidate(&mut s);
    assert_eq!(
        tx(&mut s, |t| stop_checks::accept(t, &original)),
        stop_checks::Accepted::Escalated
    );
    let first = cases(&s, "crew-a");
    let metadata = source_metadata(&s);
    assert!(
        metadata
            .iter()
            .any(|(k, _)| k.starts_with("admin.case.source.v1:")),
        "durable source uniqueness index must exist"
    );
    assert!(
        metadata.iter().any(|(k, _)| k == "admin.case.sequence.v1"),
        "monotonic counter must exist"
    );
    let id = first["items"][0]["id"].as_str().unwrap().to_owned();
    drop(s);
    // Actual file database close/reopen only: no Core or provider restart is implied.
    let mut reopened = Store::open(&path).unwrap();
    assert_eq!(cases(&reopened, "crew-a"), first);
    assert_eq!(source_metadata(&reopened), metadata);
    let direct = collab::admin_cases::get(
        reopened.conn(),
        &crewd_proto::admin_cases::AdminCaseGetParams {
            crew_id: "crew-a".into(),
            id: id.clone(),
        },
    )
    .unwrap();
    assert_eq!(serde_json::to_value(direct).unwrap(), first["items"][0]);
    assert_eq!(
        tx(&mut reopened, |t| stop_checks::accept(t, &original)),
        stop_checks::Accepted::Stale
    );
    assert!(
        stop_checks::candidate(reopened.conn(), "worker-a", NOW)
            .unwrap()
            .is_none()
    );
    assert_eq!(source_metadata(&reopened), metadata);
    tx(&mut reopened, |t| {
        work::create(
            t,
            &Sender::Human,
            WorkCreateParams {
                crew_id: "crew-a".into(),
                owner: Some("cap-a".into()),
                title: "new escalation after reopen".into(),
                ..Default::default()
            },
        )
    });
    let next = escalation_candidate_for(&mut reopened, "cap-a");
    assert_eq!(
        tx(&mut reopened, |t| stop_checks::accept(t, &next)),
        stop_checks::Accepted::Escalated
    );
    let second = cases(&reopened, "crew-a");
    let items = second["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], first["items"][0]);
    let next_id = items[1]["id"].as_str().unwrap();
    assert_eq!(
        next_id.strip_prefix("AC").unwrap().parse::<u64>().unwrap(),
        id.strip_prefix("AC").unwrap().parse::<u64>().unwrap() + 1
    );
    let next_metadata = source_metadata(&reopened);
    assert_eq!(
        next_metadata
            .iter()
            .filter(|(k, _)| k.starts_with("admin.case.source.v1:"))
            .count(),
        2
    );
    drop(reopened);
    let final_store = Store::open(&path).unwrap();
    assert_eq!(cases(&final_store, "crew-a"), second);
    assert_eq!(source_metadata(&final_store), next_metadata);
}

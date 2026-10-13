//! Bounded, stable work context from the authoritative ledger; no model calls.
use super::{
    exec::{BoxFut, Context},
    hooks::ContextExtras,
};
use crewd_proto::{WorkListParams, WorkPage};
use crewd_store::Db;
use serde_json::{Value, json};

pub const KEY: &str = "my_work_items";
const MAX_BYTES: usize = 4096;
const PAGE_SIZE: u16 = 20;

pub struct WorkContext(pub Db);
impl ContextExtras for WorkContext {
    fn extras<'a>(&'a self, session: &'a str) -> BoxFut<'a, Context> {
        let owner = session.to_owned();
        Box::pin(async move {
            let result = self
                .0
                .call_result(move |s| {
                    crewd_collab::work::list(
                        s.conn(),
                        WorkListParams {
                            owner: Some(owner),
                            live_only: true,
                            include_removed: Some(false),
                            limit: Some(PAGE_SIZE),
                            ..Default::default()
                        },
                    )
                })
                .await;
            let value = match result {
                Ok(Ok(page)) => render(page),
                // Explicitly distinguish unreadable state from an empty assignment list.
                _ => json!({"version":1,"unavailable":true,"hint":"Use my_work_items to retry; do not infer no assigned work."}).to_string(),
            };
            Context::from([(KEY.into(), value)])
        })
    }
}

fn clip(s: &str, max: usize) -> (String, bool) {
    let mut chars = s.chars();
    let text: String = chars.by_ref().take(max).collect();
    (text, chars.next().is_some())
}
fn envelope(items: &[Value], after: &Option<String>, fields_clipped: bool) -> String {
    json!({"version":1,"items":items,"nextAfter":after,"partial":after.is_some(),"fieldsClipped":fields_clipped}).to_string()
}
fn render(page: WorkPage) -> String {
    let mut items = Vec::new();
    let mut after = page.next_after;
    let mut clipped = false;
    for item in page.items {
        let (title, title_cut) = clip(&item.title, 128);
        let (crew, crew_cut) = clip(&item.crew_id, 128);
        let (note, note_cut) = item
            .note
            .as_deref()
            .map(|s| clip(s, 256))
            .unwrap_or_default();
        let row_clipped = title_cut || crew_cut || note_cut || item.blocked_on.len() > 8;
        let row = json!({"id":item.id,"crewId":crew,"revision":item.revision,
            "title":title,"status":item.status,"nextStep":note,
            "nextCheckAt":item.next_check_at,"blockedOn":item.blocked_on.iter().take(8).collect::<Vec<_>>(),
            "fieldsClipped":row_clipped});
        items.push(row);
        // Reserve the actual cursor envelope, including when the source page was complete.
        let cursor = items
            .last()
            .and_then(|i| i["id"].as_str())
            .map(str::to_owned);
        if envelope(&items, &cursor, clipped || row_clipped).len() > MAX_BYTES {
            items.pop();
            after = items
                .last()
                .and_then(|i| i["id"].as_str())
                .map(str::to_owned);
            break;
        }
        clipped |= row_clipped;
    }
    envelope(&items, &after, clipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::WorkSummary;
    fn item(n: usize) -> WorkSummary {
        WorkSummary {
            id: format!("W{n}"),
            crew_id: "crew".into(),
            frozen: false,
            title: "title".into(),
            owner: Some("worker".into()),
            status: "active".into(),
            parent: None,
            sources: vec![],
            blocked_on: vec![],
            next_check_at: Some("2026-10-13T00:00:00Z".into()),
            revision: "9007199254740993".into(),
            note: Some("next step".into()),
        }
    }
    #[test]
    fn bounded_utf8_context_keeps_revision_and_real_continuation() {
        let items = (1..=20)
            .map(|n| {
                let mut i = item(n);
                i.title = "界".repeat(1000);
                i.note = Some("界".repeat(1000));
                i
            })
            .collect();
        let page = WorkPage {
            items,
            next_after: None,
        };
        let a = render(page.clone());
        assert_eq!(a, render(page));
        assert!(a.len() <= MAX_BYTES);
        let v: Value = serde_json::from_str(&a).unwrap();
        let rows = v["items"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert!(rows.len() < 20);
        assert_eq!(v["nextAfter"], rows.last().unwrap()["id"]);
        assert_eq!(v["partial"], true);
        assert_eq!(v["fieldsClipped"], true);
        assert_eq!(rows[0]["revision"], "9007199254740993");
    }
    #[test]
    fn explicit_empty_replaces_old_assignments_without_clock_churn() {
        let v: Value = serde_json::from_str(&render(WorkPage {
            items: vec![],
            next_after: None,
        }))
        .unwrap();
        assert_eq!(v["items"], json!([]));
        assert_eq!(v["partial"], false);
        assert!(v.get("unavailable").is_none());
    }
    #[tokio::test]
    async fn actual_database_empty_and_closed_are_distinct() {
        let db = Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap();
        let hook = WorkContext(db.clone());
        let ready = hook.extras("worker").await;
        assert_eq!(
            serde_json::from_str::<Value>(&ready[KEY]).unwrap()["items"],
            json!([])
        );
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
        let failed = hook.extras("worker").await;
        let v: Value = serde_json::from_str(&failed[KEY]).unwrap();
        assert_eq!(v["unavailable"], true);
        assert!(v.get("items").is_none());
    }
    #[tokio::test]
    async fn authoritative_assignments_filter_owner_done_and_frozen_crews() {
        use crewd_proto::{Crew, Session, WorkCreateParams, WorkUpdateParams, WorkUpdateStatus};
        let mut store = crewd_store::Store::open_in_memory().unwrap();
        for crew in ["a", "b"] {
            crewd_store::crews::upsert(
                store.conn(),
                &Crew {
                    id: crew.into(),
                    title: crew.into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        for worker in ["worker", "other"] {
            crewd_store::sessions::upsert(
                store.conn(),
                &Session {
                    id: worker.into(),
                    crew_id: "a".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        }
        let mut ids = Vec::new();
        for (crew, owner, title) in [
            ("a", "worker", "LIVE"),
            ("a", "other", "OTHER"),
            ("a", "worker", "DONE"),
            ("b", "worker", "FROZEN"),
        ] {
            let (p, _) = crewd_collab::run(&mut store, 1790000000000, |tx| {
                crewd_collab::work::create(
                    tx,
                    &crewd_collab::Sender::Human,
                    WorkCreateParams {
                        crew_id: crew.into(),
                        owner: Some(owner.into()),
                        title: title.into(),
                        goal: "long omitted goal".repeat(1000),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
            ids.push((p.id, p.revision));
        }
        crewd_collab::run(&mut store, 1790000000001, |tx| {
            crewd_collab::work::update(
                tx,
                &crewd_collab::Sender::Human,
                WorkUpdateParams {
                    id: ids[2].0.clone(),
                    expected_revision: ids[2].1.clone(),
                    status: Some(WorkUpdateStatus::Done),
                    ..Default::default()
                },
            )
        })
        .unwrap();
        store
            .conn()
            .execute(
                "UPDATE crews SET archived_at='2026-10-13T00:00:00Z' WHERE id='b'",
                [],
            )
            .unwrap();
        let db = Db::spawn(store).unwrap();
        let hook = WorkContext(db.clone());
        let a = hook.extras("worker").await;
        assert_eq!(a, hook.extras("worker").await);
        let v: Value = serde_json::from_str(&a[KEY]).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["id"], ids[0].0);
        assert_eq!(v["items"][0]["title"], "LIVE");
        assert!(!a[KEY].contains("long omitted goal"));
        db.call(|s| {
            s.conn()
                .execute("DELETE FROM organization_work_book", [])
                .unwrap();
        })
        .await
        .unwrap();
        let broken = hook.extras("worker").await;
        assert_eq!(
            serde_json::from_str::<Value>(&broken[KEY]).unwrap()["unavailable"],
            true
        );
        tokio::task::spawn_blocking(move || db.shutdown())
            .await
            .unwrap();
    }
}

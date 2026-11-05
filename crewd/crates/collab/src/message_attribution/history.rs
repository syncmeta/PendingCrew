//! Read-only verification and scope projection for the original v1 correction journal.
use super::{AUDIT_PREFIX, Saved, invalid, key};
use crate::{
    CollabError, Result, Sender, asks, judgments,
    refusal::{Code, Refusal},
};
use crewd_proto::{
    ChannelWorkKind, ChannelWorkObject,
    judgments::{JudgmentLink, JudgmentObjectKind},
    message_attribution::{
        AttributionAction, AttributionTarget, MessageAttributionHistoryAction as ValueAction,
        MessageAttributionHistoryItem as Item, MessageAttributionHistoryPage as Page,
        MessageAttributionHistoryParams as Params, MessageAttributionHistoryValue as Value,
        MessageAttributionOverride,
    },
};
use crewd_store::{
    StoreError,
    message_evidence::Authority,
    object_refs::{self, ObjectKind},
    work_organization,
};
use crewd_work::WorkBook;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;

const PAGE: usize = 20;
const CAP: usize = 256;
const MAX_RECORD_BYTES: i64 = 8192;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Audit {
    version: u32,
    message_id: String,
    action: AttributionAction,
    actor: String,
    at_ms: i64,
    before: Option<Saved>,
    after: Saved,
}

fn bad() -> CollabError {
    invalid("message attribution history")
}
fn bad_cursor() -> CollabError {
    CollabError::Conflict(
        Refusal::new(Code::ResourceRequestConflict)
            .arg("request", "message attribution history cursor"),
    )
}
fn in_bounds(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn decimal(value: &str) -> Option<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == value)
}
fn object_number(kind: ObjectKind, id: &str) -> Option<i64> {
    decimal(id.strip_prefix(kind.prefix())?).filter(|n| *n > 0)
}

fn read_record<T: DeserializeOwned + Serialize>(c: &Connection, key: &str) -> Result<Option<T>> {
    let raw = c
        .query_row(
            "SELECT substr(CAST(value AS BLOB),1,?2) FROM meta WHERE key=?1",
            params![key, MAX_RECORD_BYTES + 1],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    raw.map(|bytes| {
        if bytes.len() > MAX_RECORD_BYTES as usize {
            return Err(bad());
        }
        let parsed: T = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        // Original v1 writes every field, including explicit nulls. This also
        // rejects extra fields in nested public structs that accept them alone.
        if serde_json::to_value(&parsed).map_err(StoreError::from)? != json {
            return Err(bad());
        }
        Ok(parsed)
    })
    .transpose()
}
fn valid_link_shape(link: &JudgmentLink, source_seq: i64) -> bool {
    match link.kind {
        JudgmentObjectKind::Message => {
            in_bounds(&link.id, 256)
                && decimal(&link.revision).is_some_and(|n| n > 0 && n < source_seq)
        }
        JudgmentObjectKind::Work | JudgmentObjectKind::Requirement => {
            let kind = if link.kind == JudgmentObjectKind::Work {
                ObjectKind::Plan
            } else {
                ObjectKind::Requirement
            };
            let mut fields = link.revision.split(':');
            let (Some(channel), Some(status), Some(object), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return false;
            };
            let status_ok = match kind {
                ObjectKind::Plan => matches!(status, "open" | "active" | "blocked"),
                ObjectKind::Requirement => matches!(
                    status,
                    "open" | "unclaimed" | "in_progress" | "needs_ruling"
                ),
                _ => false,
            };
            object_number(kind, &link.id).is_some()
                && status_ok
                && decimal(channel).is_some()
                && decimal(object).is_some_and(|n| kind != ObjectKind::Plan || n > 0)
        }
    }
}

fn valid_snapshot(s: &Saved, seq: i64) -> bool {
    s.version == 1
        && uuid(&s.revision)
        && in_bounds(&s.epoch, 512)
        && (s.db.is_empty() || in_bounds(&s.db, 256))
        && s.value.as_ref().is_none_or(|v| {
            v.actor == "human"
                && v.at_ms >= 0
                && match (&v.action, &v.link) {
                    (AttributionAction::Set, Some(link)) => valid_link_shape(link, seq),
                    (AttributionAction::Clear, None) => true,
                    _ => false,
                }
        })
}

fn check_row(row: &Audit, message: &str, head: &Saved, seq: i64) -> Result<()> {
    if row.version != 1
        || row.message_id != message
        || row.actor != "human"
        || row.at_ms < 0
        || &row.after != head
        || !valid_snapshot(&row.after, seq)
        || row.before.as_ref().is_some_and(|b| !valid_snapshot(b, seq))
        || match (&row.action, &row.after.value) {
            (AttributionAction::Reset, None) => false,
            (AttributionAction::Set | AttributionAction::Clear, Some(v)) => {
                row.action != v.action || row.at_ms != v.at_ms
            }
            _ => true,
        }
    {
        return Err(bad());
    }
    Ok(())
}

struct Projection<'a> {
    c: &'a Connection,
    scope: &'a Params,
    book: Option<WorkBook>,
}
impl Projection<'_> {
    fn visible(&mut self, link: &JudgmentLink) -> Result<bool> {
        let c = self.c;
        if link.kind == JudgmentObjectKind::Message {
            let seq = decimal(&link.revision).ok_or_else(bad)?;
            return Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND channel_id=?2 AND seq=?3)",
                params![link.id, self.scope.channel_id, seq],
                |row| row.get(0),
            )?);
        }
        let kind = if link.kind == JudgmentObjectKind::Work {
            ObjectKind::Plan
        } else {
            ObjectKind::Requirement
        };
        let n = object_number(kind, &link.id).ok_or_else(bad)?;
        let object = ChannelWorkObject {
            kind: if kind == ObjectKind::Plan {
                ChannelWorkKind::Work
            } else {
                ChannelWorkKind::Requirement
            },
            id: link.id.clone(),
        };
        if object_refs::get(c, kind, n)?.is_none_or(|r| r.crew_id != self.scope.crew_id)
            || !crewd_store::channel_work::linked(c, &self.scope.channel_id, &object)?
        {
            return Ok(false);
        }
        let Some(core) = work_organization::core_for_reference(c, kind, n)? else {
            return Ok(false);
        };
        if self.book.is_none() {
            self.book = Some(asks::load_org(c)?);
        }
        let book = self.book.as_ref().expect("loaded");
        let crew = match kind {
            ObjectKind::Plan => core
                .parse()
                .ok()
                .filter(|id| book.item(*id).is_some())
                .and_then(|id| book.state().item_crew(id)),
            ObjectKind::Requirement => {
                let raw_exists: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM object_refs r JOIN todos t ON r.raw_todo_id=t.id
                     WHERE r.number=?1 AND r.kind='requirement' AND t.crew_id=?2 AND t.ledger='agent'
                     AND t.deleted_at IS NULL)",
                    params![n, self.scope.crew_id], |row| row.get(0),
                )?;
                if !raw_exists {
                    return Ok(false);
                }
                core.parse()
                    .ok()
                    .filter(|id| book.entry(*id).is_some())
                    .and_then(|id| book.state().entry_crew(id))
            }
            _ => None,
        };
        Ok(crew.is_some_and(|id| id.as_str() == self.scope.crew_id))
    }

    fn value(&mut self, saved: Option<&Saved>) -> Result<Option<Value>> {
        let Some(MessageAttributionOverride { action, link, .. }) =
            saved.and_then(|s| s.value.as_ref())
        else {
            return Ok(None);
        };
        let value = match (action, link) {
            (AttributionAction::Set, Some(link)) => {
                let visible = self.visible(link)?;
                Value {
                    action: ValueAction::Set,
                    target: visible.then(|| AttributionTarget {
                        kind: link.kind,
                        id: link.id.clone(),
                    }),
                    target_unavailable: !visible,
                }
            }
            (AttributionAction::Clear, None) => Value {
                action: ValueAction::Clear,
                target: None,
                target_unavailable: false,
            },
            _ => return Err(bad()),
        };
        Ok(Some(value))
    }
}

fn checked_count(c: &Connection, prefix: &str, count: usize) -> Result<()> {
    // All v1 keys for this message end in one canonical UUID. A missing head
    // or forged end must not be presented as a complete empty/short history.
    let lower = format!("{prefix}0");
    let upper = format!("{prefix}g");
    let mut query =
        c.prepare("SELECT key FROM meta WHERE key>=?1 AND key<?2 ORDER BY key LIMIT ?3")?;
    let keys = query
        .query_map(params![lower, upper, (CAP + 1) as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if keys.len() > CAP
        || keys
            .iter()
            .filter(|key| key.strip_prefix(prefix).is_some_and(uuid))
            .count()
            != count
    {
        return Err(bad());
    }
    Ok(())
}

pub fn read(c: &Connection, by: &Sender, p: Params) -> Result<Page> {
    if !matches!(by, Sender::Human) {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ResourceForbidden,
        )));
    }
    if [&p.crew_id, &p.channel_id, &p.message_id]
        .iter()
        .any(|id| !in_bounds(id, 256))
    {
        return Err(bad());
    }
    // The source is checked before touching its metadata, without loading or
    // exporting the message body, category, or model diagnosis.
    let seq = c
        .query_row(
            "SELECT m.seq FROM messages m JOIN channels ch ON ch.id=m.channel_id
         WHERE m.id=?1 AND ch.id=?2 AND ch.crew_id=?3",
            params![p.message_id, p.channel_id, p.crew_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .filter(|seq| *seq > 0)
        .ok_or_else(bad)?;

    let root: Option<Saved> = read_record(c, &key(&p.message_id))?;
    let authority = Authority::load(c)?;
    let cursor = |index: usize| -> Result<String> {
        let proof = judgments::fingerprint(&(
            "message-attribution-history-restore-v1",
            &p.crew_id,
            &p.channel_id,
            &p.message_id,
            seq,
            authority.fingerprint(),
            &root,
            index,
        ))?;
        Ok(format!("{index}.{proof}"))
    };
    let start = if let Some(value) = &p.cursor {
        if value.len() > 70 || root.is_none() {
            return Err(bad_cursor());
        }
        let index = value
            .split_once('.')
            .and_then(|(index, _)| index.parse::<usize>().ok())
            .filter(|n| *n > 0 && *n < CAP && n % PAGE == 0)
            .ok_or_else(bad_cursor)?;
        if cursor(index)? != *value {
            return Err(bad_cursor());
        }
        index
    } else {
        0
    };

    let end = (start + PAGE).min(CAP);
    let prefix = format!("{AUDIT_PREFIX}{}:", p.message_id);
    let mut next = root.clone();
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    let mut projection = Projection {
        c,
        scope: &p,
        book: None,
    };
    // Walk the verified ancestry plus one exact lookahead. A supplied cursor
    // never chooses a persisted UUID or permits an orphan/foreign branch jump.
    for index in 0..=end {
        let Some(expected) = next.take() else {
            checked_count(c, &prefix, index)?;
            break;
        };
        if !valid_snapshot(&expected, seq) || !seen.insert(expected.revision.clone()) {
            return Err(bad());
        }
        let row: Audit =
            read_record(c, &format!("{prefix}{}", expected.revision))?.ok_or_else(bad)?;
        check_row(&row, &p.message_id, &expected, seq)?;
        if row
            .before
            .as_ref()
            .is_some_and(|b| seen.contains(&b.revision))
        {
            return Err(bad());
        }
        if index == end {
            next = Some(expected);
            break;
        }
        if index >= start {
            items.push(Item {
                historical: !authority.matches(&row.after.db, &row.after.epoch),
                action: row.action,
                at_ms: row.at_ms,
                before: projection.value(row.before.as_ref())?,
                after: projection.value(Some(&row.after))?,
            });
        }
        next = row.before;
    }
    if start != 0 && items.is_empty() {
        return Err(bad_cursor());
    }
    let has_older = next.is_some();
    let next_cursor = (has_older && end < CAP).then(|| cursor(end)).transpose()?;
    let older_unavailable = has_older && end == CAP;
    Ok(Page {
        crew_id: p.crew_id,
        channel_id: p.channel_id,
        message_id: p.message_id,
        items,
        next_cursor,
        older_unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message_attribution::{get, set},
        messages,
        testkit::{NOW, ok, ok_at, store},
    };
    use crewd_proto::{
        Message,
        message_attribution::{MessageAttributionGet, MessageAttributionSet},
    };
    use crewd_store::{Store, meta_get, meta_set};

    fn post(s: &mut Store, text: &str) -> Message {
        ok(s, |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: text.into(),
                    ..Default::default()
                },
            )
        })
        .message
    }
    fn scope(m: &Message) -> Params {
        Params {
            crew_id: "crew-a".into(),
            channel_id: m.channel_id.clone(),
            message_id: m.id.clone(),
            cursor: None,
        }
    }
    fn change(
        s: &mut Store,
        m: &Message,
        action: AttributionAction,
        link: Option<JudgmentLink>,
        at: i64,
    ) {
        let current = get(
            s.conn(),
            &Sender::Human,
            MessageAttributionGet {
                crew_id: "crew-a".into(),
                message_id: m.id.clone(),
                target: None,
            },
        )
        .unwrap();
        ok_at(s, at, |tx| {
            set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: "crew-a".into(),
                    message_id: m.id.clone(),
                    expected_revision: current.revision,
                    action,
                    link,
                },
            )
        });
    }

    #[test]
    fn corrections_keep_chain_order_current_scope_and_historical_targets() {
        let mut s = store();
        let earlier = post(&mut s, "Private target text");
        let m = post(&mut s, "Private source text");
        let p = scope(&m);
        assert!(
            read(s.conn(), &Sender::Human, p.clone())
                .unwrap()
                .items
                .is_empty()
        );
        let link = JudgmentLink {
            kind: JudgmentObjectKind::Message,
            id: earlier.id.clone(),
            revision: earlier.seq.to_string(),
        };
        change(&mut s, &m, AttributionAction::Set, Some(link), NOW + 10);
        change(&mut s, &m, AttributionAction::Clear, None, NOW + 10);
        super::super::rotate_restore_stamp(s.conn());
        change(&mut s, &m, AttributionAction::Reset, None, NOW);
        let before = s.conn().total_changes();
        let history = read(s.conn(), &Sender::Human, p.clone()).unwrap();
        assert_eq!(
            history
                .items
                .iter()
                .map(|i| i.action.clone())
                .collect::<Vec<_>>(),
            vec![
                AttributionAction::Reset,
                AttributionAction::Clear,
                AttributionAction::Set,
            ]
        );
        assert!(!history.items[0].historical);
        assert!(history.items[1..].iter().all(|i| i.historical));
        assert_eq!(history.items[0].at_ms, NOW);
        assert_eq!(history.items[2].at_ms, NOW + 10);
        assert!(history.items[0].after.is_none());
        assert!(history.items[2].before.is_none());
        assert_eq!(history.items[0].before, history.items[1].after);
        assert_eq!(history.items[1].before, history.items[2].after);
        let value = history.items[2].after.as_ref().unwrap();
        assert_eq!(value.target.as_ref().unwrap().id, earlier.id);
        assert!(!value.target_unavailable);
        let public = serde_json::to_string(&history).unwrap();
        assert!(
            !public.contains("Private")
                && !public.contains("revision")
                && !public.contains("epoch")
        );
        assert!(read(s.conn(), &Sender::Session("w-a1".into()), p.clone()).is_err());
        assert!(
            read(
                s.conn(),
                &Sender::Human,
                Params {
                    crew_id: "crew-b".into(),
                    ..p.clone()
                }
            )
            .is_err()
        );
        assert!(
            read(
                s.conn(),
                &Sender::Human,
                Params {
                    channel_id: "crew-b/lobby".into(),
                    ..p.clone()
                }
            )
            .is_err()
        );
        assert_eq!(s.conn().total_changes(), before);
        s.conn()
            .execute("DELETE FROM messages WHERE id=?1", [&earlier.id])
            .unwrap();
        let redacted = read(s.conn(), &Sender::Human, p).unwrap();
        let old = redacted.items[2].after.as_ref().unwrap();
        assert!(old.target_unavailable && old.target.is_none());
    }

    #[test]
    fn qualified_legacy_and_new_corrections_survive_normal_settings_and_keep_a_cursor() {
        use crewd_store::{judgment_state, message_evidence, remote};
        let mut s = store();
        let m = post(&mut s, "source");
        let legacy = judgment_state::epoch(s.conn()).unwrap();
        let query = MessageAttributionGet {
            crew_id: "crew-a".into(),
            message_id: m.id.clone(),
            target: None,
        };
        change(&mut s, &m, AttributionAction::Clear, None, NOW);
        let mut legacy_head = read_record::<Saved>(s.conn(), &key(&m.id))
            .unwrap()
            .unwrap();
        let audit_key = format!("{AUDIT_PREFIX}{}:{}", m.id, legacy_head.revision);
        legacy_head.epoch = legacy.clone();
        meta_set(
            s.conn(),
            &key(&m.id),
            &serde_json::to_string(&legacy_head).unwrap(),
        )
        .unwrap();
        let raw = meta_get(s.conn(), &audit_key).unwrap().unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        json["after"]["epoch"] = legacy.into();
        meta_set(s.conn(), &audit_key, &serde_json::to_string(&json).unwrap()).unwrap();
        assert_eq!(
            get(s.conn(), &Sender::Human, query.clone()).unwrap().source,
            crewd_proto::message_attribution::AttributionSource::Stale
        );
        s.with_tx(|tx| message_evidence::capture_legacy(tx))
            .unwrap();
        let legacy_current = get(s.conn(), &Sender::Human, query.clone()).unwrap();
        assert_eq!(
            legacy_current.source,
            crewd_proto::message_attribution::AttributionSource::Human
        );
        assert!(
            s.with_tx(|c| remote::configure(c, 1, false, "127.0.0.1", 7423))
                .unwrap()
        );
        assert_eq!(
            get(s.conn(), &Sender::Human, query.clone()).unwrap(),
            legacy_current
        );
        for i in 0..20 {
            change(
                &mut s,
                &m,
                if i % 2 == 0 {
                    AttributionAction::Reset
                } else {
                    AttributionAction::Clear
                },
                None,
                NOW + i,
            );
        }
        let current = get(s.conn(), &Sender::Human, query.clone()).unwrap();
        let first = read(s.conn(), &Sender::Human, scope(&m)).unwrap();
        let older = Params {
            cursor: first.next_cursor.clone(),
            ..scope(&m)
        };
        let last = read(s.conn(), &Sender::Human, older.clone()).unwrap();
        assert_eq!(last.items.len(), 1);
        assert!(!last.items[0].historical);
        s.with_tx(|c| judgment_state::prepare_configuration(c))
            .unwrap();
        assert!(
            s.with_tx(|c| remote::configure(c, 2, false, "127.0.0.1", 7424))
                .unwrap()
        );
        assert_eq!(
            get(s.conn(), &Sender::Human, query.clone()).unwrap(),
            current
        );
        assert_eq!(read(s.conn(), &Sender::Human, scope(&m)).unwrap(), first);
        assert_eq!(read(s.conn(), &Sender::Human, older.clone()).unwrap(), last);

        super::super::rotate_restore_stamp(s.conn());
        assert!(matches!(
            read(s.conn(), &Sender::Human, older),
            Err(CollabError::Conflict(_))
        ));
        assert!(
            read(s.conn(), &Sender::Human, scope(&m))
                .unwrap()
                .items
                .iter()
                .all(|row| row.historical)
        );
        assert_eq!(
            get(s.conn(), &Sender::Human, query).unwrap().source,
            crewd_proto::message_attribution::AttributionSource::Stale
        );
    }

    #[test]
    fn historical_work_revision_may_advance_but_unlink_redacts_its_target() {
        let mut s = store();
        let m = post(&mut s, "source");
        let p = scope(&m);
        let w = ok(&mut s, |tx| {
            crate::work::create(
                tx,
                &Sender::Human,
                crewd_proto::WorkCreateParams {
                    crew_id: p.crew_id.clone(),
                    channel_id: Some(p.channel_id.clone()),
                    title: "Historical Work target".into(),
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    ..Default::default()
                },
            )
        });
        let link = get(
            s.conn(),
            &Sender::Human,
            MessageAttributionGet {
                crew_id: p.crew_id.clone(),
                message_id: p.message_id.clone(),
                target: Some(AttributionTarget {
                    kind: JudgmentObjectKind::Work,
                    id: w.id.clone(),
                }),
            },
        )
        .unwrap()
        .resolved_candidate
        .unwrap()
        .link;
        change(&mut s, &m, AttributionAction::Set, Some(link), NOW);
        ok(&mut s, |tx| {
            crate::work::close_factual(
                tx,
                &Sender::Human,
                crewd_proto::WorkCloseParams {
                    id: w.id.clone(),
                    expected_revision: w.revision.clone(),
                    kind: crewd_proto::WorkClosureKind::Done,
                    note: "checked".into(),
                    evidence: crewd_proto::WorkEvidenceParam {
                        kind: crewd_proto::WorkEvidenceKind::Text,
                        value: "proof".into(),
                    },
                },
            )
        });
        let visible = read(s.conn(), &Sender::Human, p.clone()).unwrap();
        assert_eq!(
            visible.items[0]
                .after
                .as_ref()
                .unwrap()
                .target
                .as_ref()
                .unwrap()
                .id,
            w.id
        );
        ok(&mut s, |tx| {
            let revision =
                crewd_store::channel_work::revision(tx.conn(), &p.channel_id)?.to_string();
            crate::channel_work::link(
                tx,
                &Sender::Human,
                crewd_proto::ChannelWorkLinkParams {
                    channel_id: p.channel_id.clone(),
                    expected_link_revision: revision,
                    object: ChannelWorkObject {
                        kind: ChannelWorkKind::Work,
                        id: w.id.clone(),
                    },
                },
                false,
            )
        });
        let redacted = read(s.conn(), &Sender::Human, p).unwrap();
        let value = redacted.items[0].after.as_ref().unwrap();
        assert!(value.target_unavailable && value.target.is_none());
    }

    #[test]
    fn pagination_reopens_but_rejects_changed_head_foreign_cursor_and_restore() {
        let mut s = store();
        let m = post(&mut s, "first source");
        let other = post(&mut s, "another source");
        for i in 0..21 {
            change(&mut s, &m, AttributionAction::Clear, None, NOW + i);
            change(&mut s, &other, AttributionAction::Clear, None, NOW + i);
        }
        let p = scope(&m);
        let first = read(s.conn(), &Sender::Human, p.clone()).unwrap();
        let more = Params {
            cursor: first.next_cursor.clone(),
            ..p.clone()
        };
        assert_eq!(first.items.len(), PAGE);
        assert!(!first.older_unavailable);
        let last = read(s.conn(), &Sender::Human, more.clone()).unwrap();
        assert_eq!(last.items.len(), 1);
        assert!(last.next_cursor.is_none() && !last.older_unavailable);
        assert!(last.items[0].before.is_none());
        assert!(matches!(
            read(
                s.conn(),
                &Sender::Human,
                Params {
                    cursor: first.next_cursor.clone(),
                    ..scope(&other)
                }
            ),
            Err(CollabError::Conflict(_))
        ));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(
            read(reopened.conn(), &Sender::Human, more.clone()).unwrap(),
            last
        );
        change(&mut reopened, &m, AttributionAction::Reset, None, NOW);
        assert!(matches!(
            read(reopened.conn(), &Sender::Human, more.clone()),
            Err(CollabError::Conflict(_))
        ));
        super::super::rotate_restore_stamp(s.conn());
        assert!(matches!(
            read(s.conn(), &Sender::Human, more),
            Err(CollabError::Conflict(_))
        ));
    }

    #[test]
    fn forged_tombstone_missing_head_and_hidden_orphan_refuse() {
        let mut s = store();
        let m = post(&mut s, "source");
        change(&mut s, &m, AttributionAction::Clear, None, NOW);
        change(&mut s, &m, AttributionAction::Reset, None, NOW);
        let p = scope(&m);
        let original_head = meta_get(s.conn(), &key(&m.id)).unwrap().unwrap();
        let head: Saved = serde_json::from_str(&original_head).unwrap();
        let record_key = format!("{AUDIT_PREFIX}{}:{}", m.id, head.revision);
        let original = meta_get(s.conn(), &record_key).unwrap().unwrap();
        let base: serde_json::Value = serde_json::from_str(&original).unwrap();
        for (field, value) in [
            ("message_id", serde_json::json!("different-source")),
            ("action", serde_json::json!("set")),
            ("before", serde_json::Value::Null),
            ("before", serde_json::to_value(&head).unwrap()),
            ("after", base["before"].clone()),
        ] {
            let mut changed = base.clone();
            changed[field] = value;
            meta_set(s.conn(), &record_key, &changed.to_string()).unwrap();
            let before = s.conn().total_changes();
            assert!(
                read(s.conn(), &Sender::Human, p.clone()).is_err(),
                "{field}"
            );
            assert_eq!(s.conn().total_changes(), before);
        }
        meta_set(s.conn(), &record_key, &original).unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [key(&m.id)])
            .unwrap();
        assert!(read(s.conn(), &Sender::Human, p.clone()).is_err());
        meta_set(s.conn(), &key(&m.id), &original_head).unwrap();
        s.conn()
            .execute("DELETE FROM meta WHERE key=?1", [record_key])
            .unwrap();
        assert!(read(s.conn(), &Sender::Human, p).is_err());
    }

    #[test]
    fn explicit_cap_leaves_older_history_unavailable_without_writes() {
        let mut s = store();
        let m = post(&mut s, "source");
        for i in 0..=CAP {
            change(&mut s, &m, AttributionAction::Clear, None, NOW + i as i64);
        }
        let before = s.conn().total_changes();
        let mut p = scope(&m);
        let mut count = 0;
        loop {
            let page = read(s.conn(), &Sender::Human, p.clone()).unwrap();
            count += page.items.len();
            if page.next_cursor.is_none() {
                assert!(page.older_unavailable);
                break;
            }
            assert!(!page.older_unavailable);
            p.cursor = page.next_cursor;
        }
        assert_eq!(count, CAP);
        assert_eq!(s.conn().total_changes(), before);
    }
}

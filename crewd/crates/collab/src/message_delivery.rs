//! Optional routing evidence is owned by the actual enqueue transaction.
use crate::{
    CollabError, Result, Sender, Tx, judgments,
    refusal::{Code, Refusal},
    wakes,
};
use crewd_proto::{
    Mention, MentionKind, Message,
    message_delivery::{
        MessageDeliveryItem, MessageDeliveryParams, MessageDeliveryRouting as Routing,
        MessageDeliveryState as State, MessageDeliveryView, MessageWakePath as Path,
        MessageWakeReason as Reason,
    },
    orchestra::WakePriorityView,
};
use crewd_store::{StoreError, message_evidence::Authority};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const PREFIX: &str = "message.delivery.v1:";
const MAX_RECORD_BYTES: i64 = 128 * 1024;
const LIMIT: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Direct {
    explicit: Vec<String>,
    replied: Vec<String>,
    ask_answer: bool,
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub message_id: String,
    pub channel_id: String,
    pub seq: i64,
    pub path: Path,
    pub reasons: Vec<Reason>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observed {
    wake: i64,
    session: String,
    priority: wakes::WakePriority,
    path: Path,
    reasons: Vec<Reason>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Trace {
    message_id: String,
    channel_id: String,
    crew_id: String,
    seq: i64,
    database: String,
    epoch: String,
    direct: Option<Direct>,
    items: Vec<Observed>,
    no_target: bool,
    truncated: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    trace: Trace,
    seal: String,
}

fn invalid() -> CollabError {
    crate::resource_acl::invalid("message delivery evidence")
}
fn bounded_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
fn distinct(reasons: &[Reason]) -> bool {
    !reasons.is_empty()
        && reasons.len() <= 8
        && reasons
            .iter()
            .enumerate()
            .all(|(i, r)| !reasons[..i].contains(r))
}

pub(crate) fn direct_targets(mentions: &[Mention]) -> Vec<String> {
    mentions
        .iter()
        .filter(|m| matches!(m.kind, MentionKind::Session | MentionKind::Captain))
        .filter_map(|m| m.target_id.as_ref())
        .filter(|id| bounded_id(id))
        .take(256)
        .cloned()
        .collect()
}
fn key(id: &str) -> String {
    format!("{PREFIX}{id}")
}

fn load(
    c: &Connection,
    message: &str,
    channel: &str,
    crew: &str,
    seq: i64,
) -> Result<Option<Trace>> {
    let bytes = c
        .query_row(
            "SELECT substr(CAST(value AS BLOB),1,?2) FROM meta WHERE key=?1",
            params![key(message), MAX_RECORD_BYTES + 1],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    if bytes.len() > MAX_RECORD_BYTES as usize {
        return Err(invalid());
    }
    let Saved { trace, seal } = serde_json::from_slice::<Saved>(&bytes).map_err(|_| invalid())?;
    let mut ids = BTreeSet::new();
    if trace.message_id != message
        || trace.channel_id != channel
        || trace.crew_id != crew
        || trace.seq != seq
        || trace.items.len() > LIMIT
        || (trace.truncated && trace.items.len() != LIMIT)
        || (trace.no_target && !trace.items.is_empty())
        || trace.items.iter().any(|i| {
            i.wake <= 0 || !ids.insert(i.wake) || !bounded_id(&i.session) || !distinct(&i.reasons)
        })
        || judgments::fingerprint(&trace)? != seal
    {
        return Err(invalid());
    }
    Ok(Some(trace))
}

fn save(c: &Connection, trace: Trace) -> Result<()> {
    let key = key(&trace.message_id);
    let seal = judgments::fingerprint(&trace)?;
    let raw = serde_json::to_string(&Saved { trace, seal }).map_err(StoreError::from)?;
    if raw.len() > MAX_RECORD_BYTES as usize {
        return Err(invalid());
    }
    crewd_store::meta_set(c, &key, &raw)?;
    Ok(())
}

fn new_trace(
    message: &str,
    channel: &str,
    crew: String,
    seq: i64,
    database: String,
    epoch: String,
) -> Trace {
    Trace {
        message_id: message.into(),
        channel_id: channel.into(),
        crew_id: crew,
        seq,
        database,
        epoch,
        direct: None,
        items: Vec::new(),
        no_target: false,
        truncated: false,
    }
}

pub(crate) fn record_post(
    tx: &Tx<'_>,
    message: &Message,
    crew: &str,
    explicit: Vec<String>,
    replied: Vec<String>,
    ask_answer: bool,
) -> Result<()> {
    let c = tx.conn();
    optional_write(c, || {
        let authority = Authority::load(c)?;
        let (database, epoch) = authority.current().ok_or_else(invalid)?;
        let mut trace = new_trace(
            &message.id,
            &message.channel_id,
            crew.into(),
            message.seq,
            database.into(),
            epoch.into(),
        );
        trace.direct = Some(Direct {
            explicit,
            replied,
            ask_answer,
        });
        // This witness comes only from the actual post transaction, never
        // from imported message metadata or materialized public mentions.
        let key = key(&message.id);
        let seal = judgments::fingerprint(&trace)?;
        let data = serde_json::to_string(&Saved { trace, seal }).map_err(StoreError::from)?;
        c.execute(
            "INSERT INTO meta(key,value) VALUES (?1,?2)",
            params![key, data],
        )?;
        Ok(())
    })
}

fn with_trace(
    c: &Connection,
    message: &str,
    channel: &str,
    seq: i64,
    update: impl FnOnce(&mut Trace) -> bool,
) -> Result<()> {
    let crew = c
        .query_row(
            "SELECT ch.crew_id FROM messages m JOIN channels ch ON ch.id=m.channel_id
         WHERE m.id=?1 AND ch.id=?2 AND m.seq=?3",
            params![message, channel, seq],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(crew) = crew else {
        return Ok(());
    };
    let authority = Authority::load(c)?;
    let (database, epoch) = authority.current().ok_or_else(invalid)?;
    let mut trace = load(c, message, channel, &crew, seq)?
        .filter(|trace| authority.matches(&trace.database, &trace.epoch))
        .unwrap_or_else(|| new_trace(message, channel, crew, seq, database.into(), epoch.into()));
    if update(&mut trace) {
        // Only a real new routing observation can reseal qualified legacy evidence.
        trace.epoch = epoch.into();
        save(c, trace)?;
    }
    Ok(())
}

fn optional_write(c: &Connection, write: impl FnOnce() -> Result<()>) -> Result<()> {
    c.execute_batch("SAVEPOINT message_delivery_evidence")?;
    if write().is_err() {
        // This metadata is evidence only. An optional record failure may not
        // cancel a real wake or another side effect in the owning transaction.
        c.execute_batch(
            "ROLLBACK TO message_delivery_evidence; RELEASE message_delivery_evidence",
        )?;
    } else {
        c.execute_batch("RELEASE message_delivery_evidence")?;
    }
    Ok(())
}

pub fn enqueue(
    tx: &mut Tx<'_>,
    request: wakes::WakeRequest,
    capture: Capture,
) -> Result<wakes::Enqueued> {
    let exact = request.kind == wakes::WakeKind::Message
        && request.source == wakes::source::message(&capture.message_id)
        && capture.seq > 0
        && bounded_id(&request.session)
        && distinct(&capture.reasons);
    let recipient = request.session.clone();
    let priority = request.priority;
    let result = wakes::enqueue(tx, request)?;
    if exact && let wakes::Enqueued::New(wake) = result {
        let c = tx.conn();
        optional_write(c, || {
            with_trace(
                c,
                &capture.message_id,
                &capture.channel_id,
                capture.seq,
                |trace| {
                    if trace.items.len() == LIMIT {
                        let changed = !trace.truncated;
                        trace.truncated = true;
                        return changed;
                    }
                    let mut reasons = Vec::new();
                    for reason in capture.reasons {
                        if reason == Reason::DirectedUnknown
                            && let Some(direct) = &trace.direct
                        {
                            let explicit = direct.explicit.contains(&recipient);
                            let reply = direct.replied.contains(&recipient);
                            if explicit {
                                reasons.push(Reason::ExplicitMention);
                            }
                            if reply {
                                reasons.push(Reason::ReplyToSender);
                            }
                            if explicit || reply {
                                continue;
                            }
                        }
                        reasons.push(reason);
                    }
                    trace.no_target = false;
                    trace.items.push(Observed {
                        wake,
                        session: recipient,
                        priority,
                        path: capture.path,
                        reasons,
                    });
                    true
                },
            )
        })?;
    }
    Ok(result)
}

/// A complete empty evaluation is evidence only for that evaluation cut.
/// Positive enqueues are captured independently and supersede an empty view.
pub fn no_targets(tx: &Tx<'_>, message: &str, channel: &str, seq: i64) -> Result<()> {
    let c = tx.conn();
    optional_write(c, || {
        with_trace(c, message, channel, seq, |trace| {
            let ordinary = trace.direct.as_ref().is_some_and(|post| !post.ask_answer);
            if ordinary && trace.items.is_empty() && !trace.no_target {
                trace.no_target = true;
                true
            } else {
                false
            }
        })
    })
}

/// Run in the authorized crew purge before its channels/messages cascade.
pub(crate) fn purge_source_crew(tx: &Tx<'_>, crew: &str) -> Result<()> {
    tx.conn().execute(
        "DELETE FROM meta WHERE key IN (
           SELECT ?1 || m.id FROM channels ch JOIN messages m ON m.channel_id=ch.id
           WHERE ch.crew_id=?2
        )",
        params![PREFIX, crew],
    )?;
    Ok(())
}

fn priority(priority: wakes::WakePriority) -> WakePriorityView {
    match priority {
        wakes::WakePriority::Auto => WakePriorityView::Auto,
        wakes::WakePriority::System => WakePriorityView::System,
        wakes::WakePriority::Human => WakePriorityView::Human,
    }
}

pub fn read(c: &Connection, by: &Sender, p: MessageDeliveryParams) -> Result<MessageDeliveryView> {
    if !matches!(by, Sender::Human) {
        return Err(CollabError::Forbidden(Refusal::new(
            Code::ResourceForbidden,
        )));
    }
    if [&p.message_id, &p.channel_id, &p.crew_id]
        .iter()
        .any(|id| !bounded_id(id))
    {
        return Err(invalid());
    }
    let seq = c
        .query_row(
            "SELECT m.seq FROM messages m JOIN channels ch ON ch.id=m.channel_id
         WHERE m.id=?1 AND ch.id=?2 AND ch.crew_id=?3",
            params![p.message_id, p.channel_id, p.crew_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or_else(invalid)?;
    let trace = load(c, &p.message_id, &p.channel_id, &p.crew_id, seq).unwrap_or(None);
    let mut view = MessageDeliveryView {
        crew_id: p.crew_id,
        channel_id: p.channel_id,
        message_id: p.message_id,
        routing: Routing::Unknown,
        items: Vec::new(),
        truncated: false,
    };
    let Some(trace) = trace else {
        return Ok(view);
    };
    if !Authority::load(c)?.matches(&trace.database, &trace.epoch) {
        return Ok(view);
    }
    if trace.items.is_empty() {
        if trace.no_target {
            view.routing = Routing::NoEligibleTarget;
        }
        return Ok(view);
    }
    view.routing = Routing::TargetsRecorded;
    view.truncated = trace.truncated;
    let ids: Vec<_> = trace.items.iter().map(|item| item.wake).collect();
    let rows = wakes::by_ids(c, &ids)?;
    let source = wakes::source::message(&view.message_id);
    for item in trace.items {
        let session_id = crewd_store::sessions::get(c, &item.session)?.map(|s| s.id);
        let state = if let Some(row) = rows.iter().find(|w| {
            w.id == item.wake
                && w.session == item.session
                && w.kind == wakes::WakeKind::Message
                && w.source == source
                && w.priority == item.priority
                && session_id.is_some()
        }) {
            match row.state {
                wakes::WakeState::Pending => State::Queued,
                wakes::WakeState::Inflight => {
                    match crate::cursor_reservations::is_delivery_rider(c, &item.session, item.wake)
                        .unwrap_or(None)
                    {
                        Some(true) => State::Queued,
                        Some(false) => State::Delivering,
                        None => State::Unknown,
                    }
                }
                wakes::WakeState::Delivered => State::DeliveredOrAlreadyRead,
                wakes::WakeState::Dropped => State::Dropped,
            }
        } else {
            State::Unknown
        };
        view.items.push(MessageDeliveryItem {
            session_id,
            priority: priority(item.priority),
            path: item.path,
            reasons: item.reasons,
            state,
        });
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        messages,
        testkit::{ok, store},
    };
    use crewd_proto::{Message, Role, Session};
    use crewd_store::Store;

    fn rotate_restore_stamp(s: &Store) {
        crewd_store::meta_set(
            s.conn(),
            "personalDraft.restoreStamp.v1",
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
    }

    fn post(
        s: &mut Store,
        sender: Sender,
        reply: Option<String>,
        mentions: Vec<Mention>,
    ) -> Message {
        ok(s, |tx| {
            messages::post(
                tx,
                sender,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: "Synthetic delivery evidence".into(),
                    in_reply_to: reply,
                    mentions,
                    ..Default::default()
                },
            )
        })
        .message
    }

    fn scope_of(message: &Message) -> MessageDeliveryParams {
        MessageDeliveryParams {
            crew_id: "crew-a".into(),
            channel_id: message.channel_id.clone(),
            message_id: message.id.clone(),
        }
    }

    fn observation(s: &Store, message: &Message) -> MessageDeliveryView {
        read(s.conn(), &Sender::Human, scope_of(message)).unwrap()
    }

    fn recorded(
        s: &mut Store,
        message: &Message,
        session: &str,
        path: Path,
        reasons: Vec<Reason>,
    ) -> wakes::Enqueued {
        ok(s, |tx| {
            enqueue(
                tx,
                wakes::WakeRequest {
                    session: session.into(),
                    kind: wakes::WakeKind::Message,
                    source: wakes::source::message(&message.id),
                    priority: wakes::WakePriority::Human,
                    not_before_ms: None,
                    note: None,
                    origin: None,
                },
                Capture {
                    message_id: message.id.clone(),
                    channel_id: message.channel_id.clone(),
                    seq: message.seq,
                    path,
                    reasons,
                },
            )
        })
    }

    #[test]
    fn exact_direct_reply_overlap_and_duplicate_are_attributed_once() {
        let mut s = store();
        let previous = post(&mut s, Sender::Session("w-a1".into()), None, vec![]);
        let m = post(
            &mut s,
            Sender::Human,
            Some(previous.id),
            vec![Mention {
                kind: MentionKind::Session,
                target_id: Some("w-a1".into()),
            }],
        );
        assert_eq!(observation(&s, &m).routing, Routing::Unknown);
        let wakes::Enqueued::New(wake) = recorded(
            &mut s,
            &m,
            "w-a1",
            Path::Live,
            vec![Reason::DirectedUnknown, Reason::Subscription],
        ) else {
            panic!("new wake required");
        };
        assert_eq!(
            recorded(&mut s, &m, "w-a1", Path::Backlog, vec![Reason::DmPeer]),
            wakes::Enqueued::Duplicate(wake)
        );
        let current = observation(&s, &m);
        assert_eq!(current.routing, Routing::TargetsRecorded);
        assert_eq!(current.items.len(), 1);
        let row = &current.items[0];
        assert_eq!(row.path, Path::Live);
        assert_eq!(
            row.reasons,
            [
                Reason::ExplicitMention,
                Reason::ReplyToSender,
                Reason::Subscription
            ]
        );
        assert_eq!(row.state, State::Queued);
        ok(&mut s, |tx| wakes::mark_delivered(tx, &[wake]));
        assert_eq!(
            observation(&s, &m).items[0].state,
            State::DeliveredOrAlreadyRead
        );
        s.conn()
            .execute("DELETE FROM wakes WHERE id=?1", [wake])
            .unwrap();
        let pruned = observation(&s, &m);
        assert_eq!(pruned.items[0].state, State::Unknown);
        assert_eq!(pruned.items[0].reasons, row.reasons);
        rotate_restore_stamp(&s);
        assert_eq!(observation(&s, &m).routing, Routing::Unknown);
    }

    #[test]
    fn zero_is_a_completed_witness_and_old_rows_keep_unknown() {
        let mut s = store();
        let m = post(&mut s, Sender::Human, None, vec![]);
        ok(&mut s, |tx| no_targets(tx, &m.id, &m.channel_id, m.seq + 1));
        assert_eq!(observation(&s, &m).routing, Routing::Unknown);
        ok(&mut s, |tx| no_targets(tx, &m.id, &m.channel_id, m.seq));
        assert_eq!(observation(&s, &m).routing, Routing::NoEligibleTarget);
        let before = s.conn().total_changes();
        assert!(
            read(
                s.conn(),
                &Sender::Human,
                MessageDeliveryParams {
                    channel_id: "crew-b/lobby".into(),
                    ..scope_of(&m)
                }
            )
            .is_err()
        );
        assert!(read(s.conn(), &Sender::Session("w-a1".into()), scope_of(&m)).is_err());
        assert_eq!(s.conn().total_changes(), before);
        assert!(matches!(
            recorded(
                &mut s,
                &m,
                "w-a1",
                Path::Backlog,
                vec![Reason::Subscription]
            ),
            wakes::Enqueued::New(_)
        ));
        assert_eq!(observation(&s, &m).routing, Routing::TargetsRecorded);

        let mut old = m.clone();
        old.id = "legacy-no-evidence".into();
        let fake_import = serde_json::json!({
            "messageDeliveryDirectV1": {"explicit":["w-a1"],"replied":[],"askAnswer":false},
            "messageDeliveryTraceV1": {"items":[],"noTarget":true},
        });
        old.seq = crewd_store::messages::insert(s.conn(), &old, Some(&fake_import))
            .unwrap()
            .unwrap();
        ok(&mut s, |tx| {
            no_targets(tx, &old.id, &old.channel_id, old.seq)
        });
        assert_eq!(observation(&s, &old).routing, Routing::Unknown);
        recorded(
            &mut s,
            &old,
            "w-a1",
            Path::Backlog,
            vec![Reason::DirectedUnknown],
        );
        assert_eq!(
            observation(&s, &old).items[0].reasons,
            [Reason::DirectedUnknown]
        );
    }

    #[test]
    fn routed_and_no_target_evidence_survive_only_their_real_restore_cut() {
        use crewd_store::{judgment_state, message_evidence, remote};
        let mut s = store();
        let initially_stale = post(&mut s, Sender::Human, None, vec![]);
        ok(&mut s, |tx| {
            no_targets(
                tx,
                &initially_stale.id,
                &initially_stale.channel_id,
                initially_stale.seq,
            )
        });
        let old_epoch = judgment_state::epoch(s.conn()).unwrap();
        let as_legacy = |s: &Store, m: &Message, epoch: &str| {
            let mut trace = load(s.conn(), &m.id, &m.channel_id, "crew-a", m.seq)
                .unwrap()
                .unwrap();
            trace.epoch = epoch.into();
            save(s.conn(), trace).unwrap();
        };
        as_legacy(&s, &initially_stale, &old_epoch);

        assert!(
            s.with_tx(|c| remote::configure(c, 1, false, "127.0.0.1", 7423))
                .unwrap()
        );
        let exact = judgment_state::epoch(s.conn()).unwrap();
        assert_ne!(old_epoch, exact);
        let routed = post(&mut s, Sender::Human, None, vec![]);
        let empty = post(&mut s, Sender::Human, None, vec![]);
        recorded(
            &mut s,
            &routed,
            "w-a1",
            Path::Live,
            vec![Reason::ExplicitMention],
        );
        ok(&mut s, |tx| {
            no_targets(tx, &empty.id, &empty.channel_id, empty.seq)
        });
        as_legacy(&s, &routed, &exact);
        as_legacy(&s, &empty, &exact);
        assert_eq!(observation(&s, &routed).routing, Routing::Unknown);
        s.with_tx(|tx| message_evidence::capture_legacy(tx))
            .unwrap();
        assert_eq!(observation(&s, &initially_stale).routing, Routing::Unknown);
        let routed_before = observation(&s, &routed);
        let empty_before = observation(&s, &empty);
        assert_eq!(routed_before.items[0].state, State::Queued);
        assert_eq!(empty_before.routing, Routing::NoEligibleTarget);

        assert!(
            s.with_tx(|c| remote::configure(c, 2, false, "127.0.0.1", 7424))
                .unwrap()
        );
        s.with_tx(|c| judgment_state::prepare_configuration(c))
            .unwrap();
        assert_eq!(observation(&s, &routed), routed_before);
        assert_eq!(observation(&s, &empty), empty_before);
        assert_eq!(observation(&s, &initially_stale).routing, Routing::Unknown);
        recorded(
            &mut s,
            &routed,
            "w-a2",
            Path::Backlog,
            vec![Reason::Subscription],
        );
        let current = observation(&s, &routed);
        assert_eq!(current.items.len(), 2);
        assert_eq!(current.items[0], routed_before.items[0]);
        let stored = load(
            s.conn(),
            &routed.id,
            &routed.channel_id,
            "crew-a",
            routed.seq,
        )
        .unwrap()
        .unwrap();
        assert!(stored.epoch.starts_with("restore-v1:"));

        crewd_store::meta_set(
            s.conn(),
            "personalDraft.restoreStamp.v1",
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        s.with_tx(|tx| message_evidence::capture_legacy(tx))
            .unwrap();
        assert_eq!(observation(&s, &routed).routing, Routing::Unknown);
        assert_eq!(observation(&s, &empty).routing, Routing::Unknown);
    }

    #[test]
    fn corrupt_optional_evidence_keeps_actual_enqueue_and_private_metadata() {
        let mut s = store();
        let m = post(&mut s, Sender::Human, None, vec![]);
        let raw =
            r#"{"judgmentV1":{"unrelated":"protected"},"messageDeliveryDirectV1":{"fake":true}}"#;
        crewd_store::meta_set(s.conn(), &key(&m.id), "invalid private evidence").unwrap();
        s.conn()
            .execute("UPDATE messages SET extra=?1 WHERE id=?2", (raw, &m.id))
            .unwrap();
        let wakes::Enqueued::New(wake) = recorded(
            &mut s,
            &m,
            "w-a1",
            Path::Live,
            vec![Reason::ChannelOwnerFallback],
        ) else {
            panic!("new wake required");
        };
        assert_eq!(wakes::by_ids(s.conn(), &[wake]).unwrap().len(), 1);
        assert_eq!(observation(&s, &m).routing, Routing::Unknown);
        let retained: String = s
            .conn()
            .query_row("SELECT extra FROM messages WHERE id=?1", [&m.id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(retained, raw);
    }

    #[test]
    fn projection_cap_never_caps_actual_new_enqueues() {
        let mut s = store();
        let m = post(&mut s, Sender::Human, None, vec![]);
        for index in 0..=LIMIT {
            let id = format!("delivery-target-{index}");
            crewd_store::sessions::upsert(
                s.conn(),
                &Session {
                    id: id.clone(),
                    crew_id: "crew-a".into(),
                    role: Role::Worker,
                    executor: "codex".into(),
                    updated_at: "2026-10-01T00:00:00.000Z".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
            assert!(matches!(
                recorded(&mut s, &m, &id, Path::Live, vec![Reason::Subscription]),
                wakes::Enqueued::New(_)
            ));
        }
        let view = observation(&s, &m);
        assert_eq!(view.items.len(), LIMIT);
        assert!(view.truncated);
        assert_eq!(
            wakes::for_source(
                s.conn(),
                wakes::WakeKind::Message,
                &wakes::source::message(&m.id)
            )
            .unwrap()
            .len(),
            LIMIT + 1
        );
    }
}

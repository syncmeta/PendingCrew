//! Trusted, per-message Stop relay provenance, stored in the existing private meta namespace.
//! No public post/RPC field or legacy message extra grants this identity. Historical unmarked
//! messages remain Ordinary; a present invalid marker is an error, never inferred progress.
use crate::{CollabError, Result, Tx, messages, sessions};
use crewd_proto::{Message, SenderKind};
use crewd_store::StoreError;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "message.provenance.v1.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageProvenance {
    Ordinary,
    StopRelay { source_session: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredKind {
    StopRelay,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    version: u32,
    kind: StoredKind,
    message_id: String,
    seq: i64,
    source_session: String,
}

fn key(id: &str) -> String {
    format!("{PREFIX}{id}")
}

fn invalid(message_id: &str, reason: &str) -> CollabError {
    StoreError::Invalid(format!(
        "invalid message provenance for {message_id}: {reason}"
    ))
    .into()
}

fn validate(message: &Message, envelope: &Envelope) -> Result<()> {
    if envelope.version != 1 || envelope.kind != StoredKind::StopRelay {
        return Err(invalid(&message.id, "unsupported version or kind"));
    }
    if envelope.message_id != message.id || envelope.seq <= 0 || envelope.seq != message.seq {
        return Err(invalid(&message.id, "message identity mismatch"));
    }
    if envelope.source_session.trim().is_empty() {
        return Err(invalid(&message.id, "empty source session"));
    }
    match message.sender_kind {
        SenderKind::Session | SenderKind::Captain => {
            if message.sender_session_id.as_deref() != Some(envelope.source_session.as_str()) {
                return Err(invalid(
                    &message.id,
                    "source session does not match the sender",
                ));
            }
        }
        SenderKind::System => {
            // The actual System fallback retains its real caller independently of the sender.
            if message.sender_session_id.is_some() {
                return Err(invalid(&message.id, "system sender has a session identity"));
            }
        }
        SenderKind::Human => {
            return Err(invalid(&message.id, "human message cannot be a Stop relay"));
        }
    }
    Ok(())
}

/// Read alongside the actual Message in the same database job/transaction. A source session may
/// have since been purged while this message survives in a foreign channel; do not require it live.
pub fn read(conn: &Connection, message: &Message) -> Result<MessageProvenance> {
    let Some(raw) = crewd_store::meta_get(conn, &key(&message.id))? else {
        return Ok(MessageProvenance::Ordinary);
    };
    let envelope: Envelope =
        serde_json::from_str(&raw).map_err(|_| invalid(&message.id, "malformed envelope"))?;
    validate(message, &envelope)?;
    Ok(MessageProvenance::StopRelay {
        source_session: envelope.source_session,
    })
}

/// Only the successful private Stop relay path may call this, before its existing Tx commits.
pub(crate) fn mark_stop_relay(
    tx: &mut Tx<'_>,
    posted: &messages::Posted,
    source_session: &str,
) -> Result<()> {
    let actual = messages::get(tx.conn(), &posted.message.id)?;
    if actual != posted.message {
        return Err(invalid(
            &posted.message.id,
            "Posted differs from the stored message",
        ));
    }
    if source_session.trim().is_empty() {
        return Err(invalid(&actual.id, "empty source session"));
    }
    // Unlike a historical read, a new trusted relay must have a real, current caller.
    sessions::get(tx.conn(), source_session)?;
    let envelope = Envelope {
        version: 1,
        kind: StoredKind::StopRelay,
        message_id: actual.id.clone(),
        seq: actual.seq,
        source_session: source_session.into(),
    };
    validate(&actual, &envelope)?;
    let value = serde_json::to_string(&envelope).map_err(StoreError::from)?;
    // Never overwrite a historical identity. Actual relays use fresh message ids; replay is
    // suppressed before reaching this path. Any unexpected duplicate is a transaction error.
    tx.conn().execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2)",
        params![key(&actual.id), value],
    )?;
    Ok(())
}

/// After actual message deletion, remove only this module's orphan keys in the same Tx. Prefix
/// equality uses substr rather than LIKE, so punctuation/underscores never widen the match.
pub(crate) fn prune_orphans(tx: &mut Tx<'_>) -> Result<usize> {
    Ok(tx.conn().execute(
        "DELETE FROM meta
         WHERE substr(key,1,length(?1))=?1
           AND NOT EXISTS (
             SELECT 1 FROM messages WHERE messages.id=substr(meta.key,length(?1)+1)
           )",
        [PREFIX],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok, store};

    fn post(s: &mut crewd_store::Store, sender: crate::Sender) -> messages::Posted {
        ok(s, |tx| {
            messages::post(
                tx,
                sender,
                messages::NewPost {
                    channel_id: crewd_store::channels::lobby_id("crew-a"),
                    text: "same ordinary or relayed question?".into(),
                    category: Some("question".into()),
                    ..Default::default()
                },
            )
        })
    }

    fn envelope(message: &Message, source: &str) -> serde_json::Value {
        serde_json::json!({"version":1,"kind":"stop_relay","messageId":message.id,
            "seq":message.seq,"sourceSession":source})
    }

    fn raw_marker(s: &crewd_store::Store, message: &Message, raw: &str) {
        crewd_store::meta_set(s.conn(), &key(&message.id), raw).unwrap();
    }

    fn assert_invalid(result: Result<MessageProvenance>) {
        assert!(
            matches!(&result, Err(CollabError::Store(StoreError::Invalid(_)))),
            "a present invalid marker must not become Ordinary or trusted: {result:?}"
        );
    }

    #[test]
    fn absent_marker_and_unknown_extra_are_ordinary() {
        let mut s = store();
        let p = post(&mut s, crate::Sender::Session("w-a1".into()));
        assert_eq!(
            read(s.conn(), &p.message).unwrap(),
            MessageProvenance::Ordinary
        );
        // Storage control only, not a claim that this is a real legacy-import execution.
        let extra = serde_json::json!({"messageProvenance": envelope(&p.message, "w-a1"),
            "message.provenance.v1": "stop_relay"});
        s.conn()
            .execute(
                "UPDATE messages SET extra=?1 WHERE id=?2",
                params![extra.to_string(), p.message.id],
            )
            .unwrap();
        let actual = messages::get(s.conn(), &p.message.id).unwrap();
        assert_eq!(
            read(s.conn(), &actual).unwrap(),
            MessageProvenance::Ordinary
        );
        assert_eq!(
            s.conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            crewd_store::schema::SCHEMA_VERSION
        );
    }

    #[test]
    fn malformed_and_mismatched_markers_fail_closed() {
        let mut s = store();
        let p = post(&mut s, crate::Sender::Session("w-a1".into()));
        let good = envelope(&p.message, "w-a1");
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("kind", serde_json::json!("ordinary")),
            ("messageId", serde_json::json!("another-message")),
            ("seq", serde_json::json!(p.message.seq + 1)),
            ("seq", serde_json::json!(0)),
            ("sourceSession", serde_json::json!("w-a2")),
            ("sourceSession", serde_json::json!("")),
            ("sourceSession", serde_json::json!(" \t")),
            ("sourceSession", serde_json::Value::Null),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            raw_marker(&s, &p.message, &bad.to_string());
            assert_invalid(read(s.conn(), &p.message));
        }
        for raw in ["not json", "{}", "{\"version\":1,\"version\":1}"] {
            raw_marker(&s, &p.message, raw);
            assert_invalid(read(s.conn(), &p.message));
        }
        let duplicate_version = format!("{{\"version\":1,{}", &good.to_string()[1..]);
        raw_marker(&s, &p.message, &duplicate_version);
        assert_invalid(read(s.conn(), &p.message));
        let mut missing_source = good.clone();
        missing_source
            .as_object_mut()
            .unwrap()
            .remove("sourceSession");
        raw_marker(&s, &p.message, &missing_source.to_string());
        assert_invalid(read(s.conn(), &p.message));
        raw_marker(&s, &p.message, &good.to_string());
        assert_eq!(
            read(s.conn(), &p.message).unwrap(),
            MessageProvenance::StopRelay {
                source_session: "w-a1".into()
            }
        );
    }

    #[test]
    fn human_messages_cannot_gain_relay_identity() {
        let mut s = store();
        let p = post(&mut s, crate::Sender::Human);
        raw_marker(&s, &p.message, &envelope(&p.message, "w-a1").to_string());
        assert_invalid(read(s.conn(), &p.message));
    }

    #[test]
    fn mark_checks_actual_row_source_and_never_overwrites_history() {
        let mut s = store();
        let p = post(&mut s, crate::Sender::Session("w-a1".into()));
        let mut wrong_row = p.clone();
        wrong_row.message.seq += 1;
        let failed = crate::run(&mut s, NOW, |tx| mark_stop_relay(tx, &wrong_row, "w-a1"));
        assert!(matches!(
            failed,
            Err(CollabError::Store(StoreError::Invalid(_)))
        ));
        let failed = crate::run(&mut s, NOW, |tx| mark_stop_relay(tx, &p, "w-a2"));
        assert!(matches!(
            failed,
            Err(CollabError::Store(StoreError::Invalid(_)))
        ));
        assert_eq!(
            read(s.conn(), &p.message).unwrap(),
            MessageProvenance::Ordinary
        );
        ok(&mut s, |tx| mark_stop_relay(tx, &p, "w-a1"));
        let before = crewd_store::meta_get(s.conn(), &key(&p.message.id)).unwrap();
        assert!(crate::run(&mut s, NOW, |tx| mark_stop_relay(tx, &p, "w-a1")).is_err());
        assert_eq!(
            crewd_store::meta_get(s.conn(), &key(&p.message.id)).unwrap(),
            before
        );
        let system = post(&mut s, crate::Sender::System);
        assert!(
            crate::run(&mut s, NOW, |tx| mark_stop_relay(
                tx,
                &system,
                "missing-source"
            ))
            .is_err()
        );
        assert_eq!(
            read(s.conn(), &system.message).unwrap(),
            MessageProvenance::Ordinary
        );
    }

    #[test]
    fn system_fallback_provenance_survives_source_purge_in_foreign_history() {
        let mut s = store();
        // Private marker contract control: the production fallback is reached only by a refused
        // private relay helper today, not naturally by on_turn_end. This post tests retained data.
        let p = post(&mut s, crate::Sender::System);
        ok(&mut s, |tx| mark_stop_relay(tx, &p, "cap-x"));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
        assert!(sessions::get(s.conn(), "cap-x").is_err());
        let actual = messages::get(s.conn(), &p.message.id).unwrap();
        assert_eq!(
            read(s.conn(), &actual).unwrap(),
            MessageProvenance::StopRelay {
                source_session: "cap-x".into()
            }
        );
    }

    #[test]
    fn session_provenance_survives_source_purge_in_foreign_topic_history() {
        let mut s = store();
        let topic = ok(&mut s, |tx| {
            crate::channels::create(
                tx,
                &crate::Sender::Session("cap-x".into()),
                crate::channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: crewd_proto::ChannelKind::Topic,
                    name: "synthetic foreign history".into(),
                    purpose: None,
                    members: vec![crewd_proto::MemberRef::session("w-a1")],
                },
            )
        });
        let p = ok(&mut s, |tx| {
            messages::post(
                tx,
                crate::Sender::Session("cap-x".into()),
                messages::NewPost {
                    channel_id: topic.id.clone(),
                    text: "synthetic retained session question?".into(),
                    category: Some("question".into()),
                    ..Default::default()
                },
            )
        });
        // Storage contract fixture, not a claim that Stop currently selects foreign topics.
        ok(&mut s, |tx| mark_stop_relay(tx, &p, "cap-x"));
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
        assert!(sessions::get(s.conn(), "cap-x").is_err());
        let actual = messages::get(s.conn(), &p.message.id).unwrap();
        assert_eq!(actual.sender_session_id.as_deref(), Some("cap-x"));
        assert_eq!(
            read(s.conn(), &actual).unwrap(),
            MessageProvenance::StopRelay {
                source_session: "cap-x".into()
            }
        );
    }

    #[test]
    fn orphan_pruning_preserves_live_invalid_markers_and_nearby_meta() {
        let mut s = store();
        let p = post(&mut s, crate::Sender::Session("w-a1".into()));
        raw_marker(&s, &p.message, "corrupt live marker");
        crewd_store::meta_set(s.conn(), &key("absent-message"), "corrupt orphan marker").unwrap();
        for k in [
            "messageXprovenanceXv1.orphan",
            "message.provenance.v10.orphan",
            "agent.turn.unrelated",
        ] {
            crewd_store::meta_set(s.conn(), k, "kept").unwrap();
        }
        assert_eq!(ok(&mut s, prune_orphans), 1);
        assert_invalid(read(s.conn(), &p.message));
        assert_eq!(
            crewd_store::meta_get(s.conn(), &key("absent-message")).unwrap(),
            None
        );
        for k in [
            "messageXprovenanceXv1.orphan",
            "message.provenance.v10.orphan",
            "agent.turn.unrelated",
        ] {
            assert_eq!(
                crewd_store::meta_get(s.conn(), k).unwrap().as_deref(),
                Some("kept")
            );
        }
    }
}

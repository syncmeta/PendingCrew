//! Human corrections are independent facts. Raw model diagnostics are never rewritten.
use crate::{Result, Sender, Tx, channels, crews, judgments, messages};
use crewd_proto::{
    ChannelWorkKind, ChannelWorkObject, Message, judgments::*, message_attribution::*,
};
use crewd_store::message_evidence::Authority;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
mod history;
pub use history::read as read_history;

const PREFIX: &str = "message.attribution.v1:";
const AUDIT_PREFIX: &str = "message.attribution.audit.v1:";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    epoch: String,
    db: String,
    revision: String,
    value: Option<MessageAttributionOverride>,
}
#[derive(Serialize)]
struct Audit<'a> {
    version: u32,
    message_id: &'a str,
    action: &'a AttributionAction,
    actor: &'a str,
    at_ms: i64,
    before: Option<&'a Saved>,
    after: &'a Saved,
}
fn invalid(detail: &str) -> crate::CollabError {
    crate::resource_acl::invalid(detail)
}
fn key(id: &str) -> String {
    format!("{PREFIX}{id}")
}
fn saved(c: &Connection, id: &str) -> Result<Option<Saved>> {
    let result: Option<Saved> = crewd_store::meta_get(c, &key(id))?
        .map(|v| serde_json::from_str(&v).map_err(crewd_store::StoreError::from))
        .transpose()?;
    if result.as_ref().is_some_and(|s| {
        s.version != 1
            || s.revision.is_empty()
            || s.value.as_ref().is_some_and(|v| {
                v.actor != "human"
                    || matches!(v.action, AttributionAction::Reset)
                    || matches!(v.action, AttributionAction::Set) != v.link.is_some()
            })
    }) {
        return Err(invalid("corrupt attribution record"));
    }
    Ok(result)
}
fn source(c: &Connection, by: &Sender, p: &MessageAttributionGet) -> Result<Message> {
    let m = messages::get(c, &p.message_id)?;
    let ch = channels::get(c, &m.channel_id)?;
    if ch.crew_id != p.crew_id {
        return Err(invalid("crewId"));
    }
    // Same crew/channel ACL as judgment.inspect, without reading optional model metadata.
    match by {
        Sender::Human => {}
        Sender::Session(id)
            if crate::sessions::get(c, id)?.crew_id == p.crew_id
                && channels::readable_by(c, &m.channel_id, id)? => {}
        _ => {
            return Err(crate::CollabError::Forbidden(crate::refusal::Refusal::new(
                crate::refusal::Code::ResourceForbidden,
            )));
        }
    }
    Ok(m)
}
fn active(c: &Connection, m: &Message) -> Result<bool> {
    let ch = channels::get(c, &m.channel_id)?;
    Ok(ch.archived_at.is_none() && !crews::is_archived(c, &ch.crew_id)?)
}
fn target(c: &Connection, m: &Message, t: &AttributionTarget) -> Result<AttributionCandidate> {
    if !active(c, m)? {
        return Err(invalid("archived source"));
    }
    match t.kind {
        JudgmentObjectKind::Message => {
            let other = messages::get(c, &t.id)?;
            if other.channel_id != m.channel_id || other.seq >= m.seq {
                return Err(invalid("target message"));
            }
            Ok(AttributionCandidate {
                link: JudgmentLink {
                    kind: t.kind,
                    id: t.id.clone(),
                    revision: other.seq.to_string(),
                },
                summary: other.text.chars().take(200).collect(),
            })
        }
        JudgmentObjectKind::Work | JudgmentObjectKind::Requirement => {
            let o = ChannelWorkObject {
                kind: if t.kind == JudgmentObjectKind::Work {
                    ChannelWorkKind::Work
                } else {
                    ChannelWorkKind::Requirement
                },
                id: t.id.clone(),
            };
            let association =
                crate::channel_work::association(c, &Sender::Human, &m.channel_id, &o)?;
            if !association.linked {
                return Err(invalid("target not linked"));
            }
            let b = crate::asks::load_org(c)?;
            let s = crate::channel_work::summary(c, &b, o)?;
            if s.crew_id != channels::get(c, &m.channel_id)?.crew_id
                || matches!(s.status.as_str(), "done" | "dropped" | "closed")
            {
                return Err(invalid("target not active"));
            }
            Ok(AttributionCandidate {
                link: JudgmentLink {
                    kind: t.kind,
                    id: t.id.clone(),
                    revision: format!(
                        "{}:{}:{}",
                        association.link_revision,
                        s.status,
                        s.revision.unwrap_or_default()
                    ),
                },
                summary: s.label,
            })
        }
    }
}
fn valid_link(c: &Connection, m: &Message, l: &JudgmentLink) -> Result<bool> {
    match target(
        c,
        m,
        &AttributionTarget {
            kind: l.kind,
            id: l.id.clone(),
        },
    ) {
        Ok(t) => Ok(t.link == *l),
        Err(
            crate::CollabError::Invalid(_)
            | crate::CollabError::NotFound(_)
            | crate::CollabError::Forbidden(_)
            | crate::CollabError::Conflict(_),
        ) => Ok(false),
        Err(e) => Err(e),
    }
}
fn model(c: &Connection, m: &Message) -> Result<(AttributionSource, Option<JudgmentLink>)> {
    let Some(r) = judgments::record(c, &JudgmentSourceKind::Message, &m.id)? else {
        return Ok((AttributionSource::None, None));
    };
    if !matches!(r.phase, JudgmentPhase::Settled) {
        return Ok((
            if matches!(
                r.phase,
                JudgmentPhase::Suppressed | JudgmentPhase::Historical
            ) {
                AttributionSource::Stale
            } else {
                AttributionSource::None
            },
            None,
        ));
    }
    let settings = judgments::configuration(c)?;
    let mut valid = active(c, m)?
        && settings.revision.as_ref() == Some(&r.config_revision)
        && crewd_store::meta_get(c, judgments::PENDING_KEY)?.is_none();
    for (id, floor) in &r.captured_floors {
        valid &= crate::wakes::human_stop_floor(c, id)? == *floor
            && r.captured_stop_epochs.get(id) == Some(&judgments::stop_epoch(c, id)?);
    }
    if let Some(l) = &r.attribution {
        valid &= valid_link(c, m, l)?;
    }
    // Attribution is an immutable reference, not a wake permission. New unrelated
    // messages/loads do not invalidate it; authority, restore and Stop still do.
    if valid {
        Ok((AttributionSource::Model, r.attribution))
    } else {
        Ok((AttributionSource::Stale, None))
    }
}
fn revision(m: &Message, s: Option<&Saved>, authority: &Authority) -> Result<String> {
    judgments::fingerprint(&(
        "attribution-restore-v1",
        &m.id,
        m.seq,
        authority.fingerprint(),
        s.map(|s| &s.revision),
    ))
}
pub fn get(
    c: &Connection,
    by: &Sender,
    p: MessageAttributionGet,
) -> Result<MessageAttributionView> {
    let m = source(c, by, &p)?;
    let s = saved(c, &m.id)?;
    let authority = Authority::load(c)?;
    let rev = revision(&m, s.as_ref(), &authority)?;
    let mut candidates = Vec::new();
    let mut truncated = false;
    if active(c, &m)? {
        let page = crate::channel_work::list(
            c,
            by,
            crewd_proto::ChannelWorkListParams {
                channel_id: m.channel_id.clone(),
                kinds: Some(vec![ChannelWorkKind::Work, ChannelWorkKind::Requirement]),
                tag: None,
                after: None,
                limit: Some(10),
            },
        )?;
        truncated = page.next.is_some();
        for item in page.items {
            if matches!(item.status.as_str(), "done" | "dropped" | "closed")
                || item.crew_id != p.crew_id
            {
                continue;
            }
            let kind = if item.object.kind == ChannelWorkKind::Work {
                JudgmentObjectKind::Work
            } else {
                JudgmentObjectKind::Requirement
            };
            candidates.push(AttributionCandidate {
                link: JudgmentLink {
                    kind,
                    id: item.object.id,
                    revision: format!(
                        "{}:{}:{}",
                        page.link_revision,
                        item.status,
                        item.revision.unwrap_or_default()
                    ),
                },
                summary: item.label,
            });
        }
        let mut q=c.prepare("SELECT id,text,seq FROM messages WHERE channel_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 11")?;
        let rows = q
            .query_map(rusqlite::params![m.channel_id, m.seq], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        truncated |= rows.len() > 10;
        for (id, text, seq) in rows.into_iter().take(10) {
            candidates.push(AttributionCandidate {
                link: JudgmentLink {
                    kind: JudgmentObjectKind::Message,
                    id,
                    revision: seq.to_string(),
                },
                summary: text.chars().take(200).collect(),
            });
        }
    }
    let resolved_candidate = p.target.as_ref().map(|t| target(c, &m, t)).transpose()?;
    let value = s.as_ref().and_then(|s| s.value.clone());
    let (src, link) = if let Some(v) = &value {
        let current = s
            .as_ref()
            .is_some_and(|s| authority.matches(&s.db, &s.epoch));
        let valid = current
            && active(c, &m)?
            && match &v.link {
                Some(l) => valid_link(c, &m, l)?,
                None => true,
            };
        if valid {
            (AttributionSource::Human, v.link.clone())
        } else {
            (AttributionSource::Stale, None)
        }
    } else {
        model(c, &m)?
    };
    Ok(MessageAttributionView {
        crew_id: p.crew_id,
        message_id: p.message_id,
        revision: rev,
        source: src,
        link,
        r#override: value,
        candidates,
        candidates_truncated: truncated,
        sample_only: true,
        resolved_candidate,
    })
}
pub fn set(
    tx: &mut Tx<'_>,
    by: &Sender,
    p: MessageAttributionSet,
) -> Result<MessageAttributionView> {
    if !matches!(by, Sender::Human) {
        return Err(crate::CollabError::Forbidden(crate::refusal::Refusal::new(
            crate::refusal::Code::ResourceForbidden,
        )));
    }
    let query = MessageAttributionGet {
        crew_id: p.crew_id,
        message_id: p.message_id,
        target: None,
    };
    let m = source(tx.conn(), by, &query)?;
    if !active(tx.conn(), &m)? {
        return Err(invalid("archived source"));
    }
    let before = saved(tx.conn(), &m.id)?;
    let authority = Authority::load(tx.conn())?;
    let (database, epoch) = authority
        .current()
        .ok_or_else(|| invalid("message correction authority unavailable"))?;
    if revision(&m, before.as_ref(), &authority)? != p.expected_revision {
        return Err(crate::CollabError::Conflict(
            crate::refusal::Refusal::new(crate::refusal::Code::ResourceRequestConflict)
                .arg("request", "message attribution revision"),
        ));
    }
    if matches!(p.action, AttributionAction::Set) != p.link.is_some() {
        return Err(invalid("link"));
    }
    if let Some(l) = &p.link
        && !valid_link(tx.conn(), &m, l)?
    {
        return Err(invalid("target revision"));
    }
    let value = if p.action == AttributionAction::Reset {
        None
    } else {
        Some(MessageAttributionOverride {
            action: p.action.clone(),
            link: p.link,
            actor: "human".into(),
            at_ms: tx.now_ms(),
        })
    };
    let after = Saved {
        version: 1,
        epoch: epoch.into(),
        db: database.into(),
        revision: uuid::Uuid::new_v4().to_string(),
        value,
    };
    let audit = Audit {
        version: 1,
        message_id: &m.id,
        action: &p.action,
        actor: "human",
        at_ms: tx.now_ms(),
        before: before.as_ref(),
        after: &after,
    };
    crewd_store::meta_set(
        tx.conn(),
        &format!("{AUDIT_PREFIX}{}:{}", m.id, after.revision),
        &serde_json::to_string(&audit).map_err(crewd_store::StoreError::from)?,
    )?;
    crewd_store::meta_set(
        tx.conn(),
        &key(&m.id),
        &serde_json::to_string(&after).map_err(crewd_store::StoreError::from)?,
    )?;
    tx.note(crate::Change::ChannelChanged {
        channel_id: m.channel_id,
    });
    get(tx.conn(), by, query)
}
#[cfg(test)]
fn rotate_restore_stamp(c: &Connection) {
    crewd_store::meta_set(
        c,
        "personalDraft.restoreStamp.v1",
        &uuid::Uuid::new_v4().to_string(),
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        messages,
        testkit::{ok, store},
    };
    use crewd_proto::judgments::JudgmentObjectKind;
    #[test]
    fn human_correction_clear_reset_and_cas_preserve_message() {
        let mut s = store();
        let old = ok(&mut s, |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: "old".into(),
                    ..Default::default()
                },
            )
        })
        .message;
        let msg = ok(&mut s, |tx| {
            messages::post(
                tx,
                Sender::Human,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: "new".into(),
                    ..Default::default()
                },
            )
        })
        .message;
        let p = MessageAttributionGet {
            crew_id: "crew-a".into(),
            message_id: msg.id.clone(),
            target: Some(AttributionTarget {
                kind: JudgmentObjectKind::Message,
                id: old.id,
            }),
        };
        let first = get(s.conn(), &Sender::Human, p.clone()).unwrap();
        let link = first.resolved_candidate.unwrap().link;
        let corrected = ok(&mut s, |tx| {
            set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: p.crew_id.clone(),
                    message_id: p.message_id.clone(),
                    expected_revision: first.revision.clone(),
                    action: AttributionAction::Set,
                    link: Some(link.clone()),
                },
            )
        });
        assert_eq!(corrected.source, AttributionSource::Human);
        assert_eq!(corrected.link, Some(link));
        let cleared = ok(&mut s, |tx| {
            set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: p.crew_id.clone(),
                    message_id: p.message_id.clone(),
                    expected_revision: corrected.revision,
                    action: AttributionAction::Clear,
                    link: None,
                },
            )
        });
        assert_eq!(cleared.source, AttributionSource::Human);
        assert!(cleared.link.is_none());
        let reset = ok(&mut s, |tx| {
            set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: p.crew_id,
                    message_id: p.message_id,
                    expected_revision: cleared.revision,
                    action: AttributionAction::Reset,
                    link: None,
                },
            )
        });
        assert_eq!(reset.source, AttributionSource::None);
        assert!(reset.r#override.is_none());
        assert_ne!(reset.revision, first.revision);
        assert_eq!(messages::get(s.conn(), &msg.id).unwrap(), msg);
    }
    fn post(s: &mut crewd_store::Store, text: &str, by: Sender) -> Message {
        ok(s, |tx| {
            messages::post(
                tx,
                by,
                messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: text.into(),
                    ..Default::default()
                },
            )
        })
        .message
    }
    fn query(m: &Message) -> MessageAttributionGet {
        MessageAttributionGet {
            crew_id: "crew-a".into(),
            message_id: m.id.clone(),
            target: None,
        }
    }
    fn clear(s: &mut crewd_store::Store, m: &Message) -> MessageAttributionView {
        let v = get(s.conn(), &Sender::Human, query(m)).unwrap();
        ok(s, |tx| {
            set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: "crew-a".into(),
                    message_id: m.id.clone(),
                    expected_revision: v.revision,
                    action: AttributionAction::Clear,
                    link: None,
                },
            )
        })
    }
    #[test]
    fn old_target_outside_sample_any_sender_cas_acl_and_restore() {
        let mut s = store();
        let old = post(&mut s, "old", Sender::Human);
        for _ in 0..12 {
            post(&mut s, "middle", Sender::Human);
        }
        let m = post(&mut s, "worker output", Sender::Session("w-a1".into()));
        let mut q = query(&m);
        let first = get(s.conn(), &Sender::Human, q.clone()).unwrap();
        assert!(first.candidates_truncated);
        assert!(first.candidates.iter().all(|c| c.link.id != old.id));
        q.target = Some(AttributionTarget {
            kind: JudgmentObjectKind::Message,
            id: old.id,
        });
        let target = get(s.conn(), &Sender::Human, q)
            .unwrap()
            .resolved_candidate
            .unwrap();
        let p = MessageAttributionSet {
            crew_id: "crew-a".into(),
            message_id: m.id.clone(),
            expected_revision: first.revision.clone(),
            action: AttributionAction::Set,
            link: Some(target.link),
        };
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| set(
                tx,
                &Sender::Session("cap-a".into()),
                p.clone()
            ))
            .is_err()
        );
        assert!(get(s.conn(), &Sender::Session("w-b1".into()), query(&m)).is_err());
        let result = ok(&mut s, |tx| set(tx, &Sender::Human, p.clone()));
        assert_eq!(result.source, AttributionSource::Human);
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| set(
                tx,
                &Sender::Human,
                p.clone()
            ))
            .is_err()
        );
        rotate_restore_stamp(s.conn());
        let stale = get(s.conn(), &Sender::Human, query(&m)).unwrap();
        assert_eq!(stale.source, AttributionSource::Stale);
        assert!(stale.link.is_none());
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    expected_revision: result.revision,
                    ..p
                }
            ))
            .is_err()
        );
    }
    #[test]
    fn audit_and_override_roll_back_together_and_reset_preserves_audit() {
        let mut s = store();
        let m = post(&mut s, "source", Sender::Human);
        let first = clear(&mut s, &m);
        let before = crewd_store::meta_get(s.conn(), &key(&m.id)).unwrap();
        let p = MessageAttributionSet {
            crew_id: "crew-a".into(),
            message_id: m.id.clone(),
            expected_revision: first.revision,
            action: AttributionAction::Reset,
            link: None,
        };
        let result: Result<((), Vec<crate::Change>)> =
            crate::run(&mut s, crate::testkit::NOW, |tx| {
                set(tx, &Sender::Human, p.clone())?;
                Err(invalid("forced downstream failure"))
            });
        assert!(result.is_err());
        assert_eq!(
            crewd_store::meta_get(s.conn(), &key(&m.id)).unwrap(),
            before
        );
        let n: i64 = s
            .conn()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'message.attribution.audit.v1:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        ok(&mut s, |tx| set(tx, &Sender::Human, p));
        let rows: Vec<String> = s
            .conn()
            .prepare("SELECT value FROM meta WHERE key LIKE 'message.attribution.audit.v1:%'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().any(
                |r| serde_json::from_str::<serde_json::Value>(r).unwrap()["action"] == "reset"
            )
        );
    }
    #[test]
    fn late_model_preserved_but_cannot_replace_human_and_reset_never_requeries() {
        let mut s = store();
        crewd_store::meta_set(
            s.conn(),
            judgments::CONFIG_KEY,
            &serde_json::to_string(&JudgmentSettings {
                revision: Some("fake-config".into()),
                config: Some(JudgmentConfig {
                    enabled: true,
                    provider: "typesafe".into(),
                    base_url: "https://example.invalid".into(),
                    model: "fake".into(),
                    choice_min: 0.5,
                    wake_yes: 0.5,
                    similar_requirements: None,
                    similar_asks: None,
                    credential_source: "file".into(),
                }),
            })
            .unwrap(),
        )
        .unwrap();
        crewd_store::meta_set(s.conn(), judgments::ACTIVE_KEY, "test").unwrap();
        let old = post(&mut s, "previous", Sender::Human);
        let m = post(&mut s, "source", Sender::Human);
        let (r, input, _) = ok(&mut s, |tx| {
            judgments::dispatch(tx, &JudgmentSourceKind::Message, &m.id)
        })
        .unwrap();
        let link = input.links.iter().find(|l| l.id == old.id).unwrap();
        let link = JudgmentLink {
            kind: JudgmentObjectKind::Message,
            id: old.id.clone(),
            revision: link.revision.clone(),
        };
        let human = clear(&mut s, &m);
        assert!(ok(&mut s, |tx| judgments::settle(
            tx,
            &JudgmentSourceKind::Message,
            &m.id,
            &r.decision_id,
            vec![],
            Some(link.clone()),
            serde_json::json!({"status":"answered"})
        )));
        let view = get(s.conn(), &Sender::Human, query(&m)).unwrap();
        assert_eq!(view.source, AttributionSource::Human);
        assert!(view.link.is_none());
        assert_eq!(view.revision, human.revision);
        let raw = judgments::record(s.conn(), &JudgmentSourceKind::Message, &m.id)
            .unwrap()
            .unwrap();
        assert_eq!(raw.attribution, Some(link.clone()));
        let reset = ok(&mut s, |tx| {
            set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: "crew-a".into(),
                    message_id: m.id.clone(),
                    expected_revision: human.revision,
                    action: AttributionAction::Reset,
                    link: None,
                },
            )
        });
        assert_eq!(reset.source, AttributionSource::Model);
        assert_eq!(reset.link, Some(link));
        post(&mut s, "unrelated later message", Sender::Human);
        assert_eq!(
            get(s.conn(), &Sender::Human, query(&m)).unwrap().source,
            AttributionSource::Model
        );
        crewd_store::judgment_state::restored(s.conn()).unwrap();
        assert_eq!(
            get(s.conn(), &Sender::Human, query(&m)).unwrap().source,
            AttributionSource::Stale
        );
        assert_eq!(
            judgments::record(s.conn(), &JudgmentSourceKind::Message, &m.id)
                .unwrap()
                .unwrap()
                .decision_id,
            raw.decision_id
        );
    }

    #[test]
    fn work_target_outside_sample_closure_and_forged_revision_are_checked() {
        let mut s = store();
        let mut works = Vec::new();
        for n in 0..12 {
            works.push(ok(&mut s, |tx| {
                crate::work::create(
                    tx,
                    &Sender::Human,
                    crewd_proto::WorkCreateParams {
                        crew_id: "crew-a".into(),
                        title: format!("work {n}"),
                        request_id: Some(uuid::Uuid::new_v4().to_string()),
                        channel_id: Some("crew-a/lobby".into()),
                        ..Default::default()
                    },
                )
            }));
        }
        let m = post(&mut s, "work source", Sender::Human);
        let first = get(s.conn(), &Sender::Human, query(&m)).unwrap();
        assert!(first.candidates_truncated);
        let w = works
            .iter()
            .find(|w| first.candidates.iter().all(|t| t.link.id != w.id))
            .unwrap();
        let mut q = query(&m);
        q.target = Some(AttributionTarget {
            kind: JudgmentObjectKind::Work,
            id: w.id.clone(),
        });
        let candidate = get(s.conn(), &Sender::Human, q)
            .unwrap()
            .resolved_candidate
            .unwrap();
        let p = MessageAttributionSet {
            crew_id: "crew-a".into(),
            message_id: m.id.clone(),
            expected_revision: first.revision,
            action: AttributionAction::Set,
            link: Some(candidate.link),
        };
        let mut forged = p.clone();
        forged.link.as_mut().unwrap().revision = "forged".into();
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| set(
                tx,
                &Sender::Human,
                forged
            ))
            .is_err()
        );
        ok(&mut s, |tx| set(tx, &Sender::Human, p));
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
        let stale = get(s.conn(), &Sender::Human, query(&m)).unwrap();
        assert_eq!(stale.source, AttributionSource::Stale);
        assert!(stale.link.is_none());
        assert!(stale.r#override.is_some());
    }
    #[test]
    fn malformed_override_is_not_absence_and_failed_update_preserves_it() {
        let mut s = store();
        let m = post(&mut s, "source", Sender::Human);
        let v = get(s.conn(), &Sender::Human, query(&m)).unwrap();
        crewd_store::meta_set(s.conn(), &key(&m.id), "{broken").unwrap();
        assert!(get(s.conn(), &Sender::Human, query(&m)).is_err());
        assert!(
            crate::run(&mut s, crate::testkit::NOW, |tx| set(
                tx,
                &Sender::Human,
                MessageAttributionSet {
                    crew_id: "crew-a".into(),
                    message_id: m.id.clone(),
                    expected_revision: v.revision,
                    action: AttributionAction::Clear,
                    link: None
                }
            ))
            .is_err()
        );
        assert_eq!(
            crewd_store::meta_get(s.conn(), &key(&m.id))
                .unwrap()
                .as_deref(),
            Some("{broken")
        );
    }
    #[test]
    fn normal_store_reopen_preserves_correction_revision_and_audit() {
        let mut s = store();
        let m = post(&mut s, "source", Sender::Human);
        let first = clear(&mut s, &m);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        s.conn()
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(s);
        let reopened = crewd_store::Store::open(&path).unwrap();
        let next = get(reopened.conn(), &Sender::Human, query(&m)).unwrap();
        assert_eq!(next.revision, first.revision);
        assert_eq!(next.source, AttributionSource::Human);
        let n: i64 = reopened
            .conn()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'message.attribution.audit.v1:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        drop(reopened);
        let reopened = crewd_store::Store::open(&path).unwrap();
        assert_eq!(
            get(reopened.conn(), &Sender::Human, query(&m))
                .unwrap()
                .revision,
            first.revision
        );
    }
}

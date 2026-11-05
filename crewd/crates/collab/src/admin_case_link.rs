//! Evidence-backed removal of one structural edge by its bound case executor.
use crate::admin_case_execution::{self as execution, ActionScope, ExecutionWitness, ReadScope};
use crate::{Result, Tx, judgments, work};
use crewd_proto::WorkEvidenceParam;
use crewd_store::{StoreError, meta_get, meta_set};
use crewd_work::{WorkBook, WorkId, WorkItem};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

pub const UNLINK_TOOL: &str = "admin_case_work_unlink";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Parent,
    Dependency,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnlinkParams {
    pub request_id: String,
    pub id: String,
    pub expected_revision: String,
    pub related_id: String,
    pub expected_related_revision: String,
    pub edge: Edge,
    pub evidence: WorkEvidenceParam,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnlinkReceipt {
    pub case_id: String,
    pub session_id: String,
    pub request_id: String,
    pub id: String,
    pub revision: String,
    pub related_id: String,
    pub related_revision: String,
    pub edge: Edge,
    pub event_seq: String,
    pub evidence: WorkEvidenceParam,
    pub note: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    digest: String,
    receipt: UnlinkReceipt,
    checksum: String,
}

pub(crate) struct ExpectedLink {
    pub after: WorkItem,
    related: WorkItem,
}

impl ExpectedLink {
    pub(crate) fn matches(&self, book: &WorkBook) -> bool {
        let Some(actual) = book.item(self.after.id) else {
            return false;
        };
        let mut expected = self.after.clone();
        expected.revision = actual.revision;
        expected.updated_at = actual.updated_at;
        actual == &expected && book.item(self.related.id) == Some(&self.related)
    }
}

fn revision(input: &str) -> Result<u64> {
    input
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == input)
        .ok_or_else(|| crate::resource_invalid("expected_revision"))
}

pub(crate) fn expected(
    c: &Connection,
    book: &WorkBook,
    crew: &str,
    core: WorkId,
    p: &UnlinkParams,
) -> Result<ExpectedLink> {
    let peer = work::local_plan(c, crew, &p.related_id)?;
    let mut after = book
        .item(core)
        .cloned()
        .ok_or_else(|| crate::resource_invalid("id"))?;
    let related = book
        .item(peer)
        .cloned()
        .ok_or_else(|| crate::resource_invalid("related_id"))?;
    if after.status.is_terminal() || core == peer {
        return Err(crate::resource_invalid("edge"));
    }
    work::check_revision(&after, &p.id, revision(&p.expected_revision)?)?;
    work::check_revision(
        &related,
        &p.related_id,
        revision(&p.expected_related_revision)?,
    )?;
    match p.edge {
        Edge::Parent if after.parent == Some(peer) => after.parent = None,
        Edge::Dependency if after.depends_on.contains(&peer) => {
            after.depends_on.retain(|id| *id != peer)
        }
        _ => return Err(crate::resource_invalid("edge")),
    }
    Ok(ExpectedLink { after, related })
}

fn invalid_receipt() -> crate::CollabError {
    StoreError::Invalid("invalid work unlink receipt".into()).into()
}

fn key(witness: &ExecutionWitness, request: &str) -> String {
    format!("admin.case.unlink.v1:{}:{request}", witness.case_id())
}

fn load_receipt(
    c: &Connection,
    witness: &ExecutionWitness,
    request: &str,
    digest: &str,
) -> Result<Option<UnlinkReceipt>> {
    let Some(raw) = meta_get(c, &key(witness, request))? else {
        return Ok(None);
    };
    let saved = decode_receipt(&raw, witness, request)?;
    if saved.digest != digest {
        return Err(crate::CollabError::Conflict(
            crate::refusal::Refusal::new(crate::refusal::Code::ResourceRequestConflict)
                .arg("request", request),
        ));
    }
    Ok(Some(saved.receipt))
}

fn decode_receipt(raw: &str, witness: &ExecutionWitness, request: &str) -> Result<Stored> {
    let saved: Stored = serde_json::from_str(raw).map_err(StoreError::from)?;
    if saved.receipt.case_id != witness.case_id()
        || saved.receipt.session_id != witness.session_id()
        || saved.receipt.request_id != request
        || saved.checksum != judgments::fingerprint(&(&saved.digest, &saved.receipt))?
    {
        return Err(invalid_receipt());
    }
    Ok(saved)
}

pub(crate) fn report_receipt(
    c: &Connection,
    witness: &ExecutionWitness,
    effect: &execution::EffectReceipt,
) -> Result<Option<UnlinkReceipt>> {
    let event_seq = effect.first_event.to_string();
    if let Some(request) = &effect.link_request_id {
        let raw = meta_get(c, &key(witness, request))?.ok_or_else(invalid_receipt)?;
        let receipt = decode_receipt(&raw, witness, request)?.receipt;
        if receipt.event_seq != event_seq {
            return Err(invalid_receipt());
        }
        return Ok(Some(receipt));
    }
    // Pre-index receipts can be recovered only when the whole case-specific
    // range fits the bounded read and provides a unique event match.
    let prefix = key(witness, "");
    let mut stmt =
        c.prepare("SELECT key,value FROM meta WHERE key>=?1 AND key<?2 ORDER BY key LIMIT 51")?;
    let rows = stmt
        .query_map(params![prefix, format!("{prefix}\u{10ffff}")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 50 {
        return Ok(None);
    }
    let mut found = None;
    for (key, raw) in rows {
        let request = key.strip_prefix(&prefix).ok_or_else(invalid_receipt)?;
        let receipt = decode_receipt(&raw, witness, request)?.receipt;
        if receipt.event_seq == event_seq && found.replace(receipt).is_some() {
            return Err(invalid_receipt());
        }
    }
    Ok(found)
}

pub fn require_committed(
    c: &Connection,
    witness: &ExecutionWitness,
    p: &UnlinkParams,
) -> Result<()> {
    let request = crate::resource_acl::uuid(&p.request_id)?;
    load_receipt(c, witness, &request, &judgments::fingerprint(p)?)?.ok_or_else(invalid_receipt)?;
    Ok(())
}

/// Only an authenticated case port calls this entry. The original attempt,
/// typed event, exact payload and result commit under one transaction.
pub fn unlink(
    tx: &mut Tx<'_>,
    witness: &ExecutionWitness,
    p: UnlinkParams,
) -> Result<UnlinkReceipt> {
    let request = crate::resource_acl::uuid(&p.request_id)?;
    for (name, value) in [("evidence", &p.evidence.value), ("note", &p.note)] {
        if value.trim().is_empty() || value.len() > 4096 {
            return Err(crate::resource_invalid(name));
        }
    }
    for id in [&p.id, &p.related_id] {
        execution::validate_read(
            tx.conn(),
            witness,
            ReadScope::Work { id: id.clone() },
            tx.now_ms(),
        )?;
    }
    let digest = judgments::fingerprint(&p)?;
    if let Some(saved) = load_receipt(tx.conn(), witness, &request, &digest)? {
        return Ok(saved);
    }
    let permit =
        execution::validate_effect(tx, witness, ActionScope::WorkUnlink(Box::new(p.clone())))?;
    let event_seq = permit.apply_unlink(tx)?;
    execution::finish_effect(tx, permit)?;
    let receipt = UnlinkReceipt {
        case_id: witness.case_id().into(),
        session_id: witness.session_id().into(),
        request_id: request,
        revision: work::get(tx.conn(), &p.id)?.revision,
        id: p.id,
        related_id: p.related_id,
        related_revision: p.expected_related_revision,
        edge: p.edge,
        event_seq: event_seq.to_string(),
        evidence: p.evidence,
        note: p.note,
    };
    let checksum = judgments::fingerprint(&(&digest, &receipt))?;
    let saved = Stored {
        digest,
        receipt,
        checksum,
    };
    meta_set(
        tx.conn(),
        &key(witness, &saved.receipt.request_id),
        &serde_json::to_string(&saved).map_err(StoreError::from)?,
    )?;
    Ok(saved.receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, ok, store};
    use crewd_proto::{AdminCaseOutcomePhase, WorkEvidenceKind};

    fn params(s: &crewd_store::Store, edge: Edge) -> UnlinkParams {
        let item = work::get(s.conn(), "W2").unwrap();
        let related = work::get(s.conn(), "W1").unwrap();
        UnlinkParams {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: item.id,
            expected_revision: item.revision,
            related_id: related.id,
            expected_related_revision: related.revision,
            edge,
            evidence: WorkEvidenceParam {
                kind: WorkEvidenceKind::Text,
                value: "fixture source proves independent work".into(),
            },
            note: "the original relation is factually wrong".into(),
        }
    }

    #[test]
    fn unlink_receipt_is_exact_and_durable_and_never_resolves_open_work() {
        for edge in [Edge::Parent, Edge::Dependency] {
            let mut s = store();
            let (case, reg) = execution::tests::running_case_with_edge(&mut s, Some(edge));
            let p = params(&s, edge);
            let before_peer = work::get(s.conn(), &p.related_id).unwrap();
            let before = work::get(s.conn(), &p.id).unwrap();
            let receipt = ok(&mut s, |tx| unlink(tx, &reg.witness, p.clone()));
            let after = work::get(s.conn(), &p.id).unwrap();
            assert_ne!(after.revision, before.revision);
            assert_eq!(
                (after.status, after.owner, after.blocked_on),
                (before.status, before.owner, before.blocked_on)
            );
            assert_eq!(work::get(s.conn(), &p.related_id).unwrap(), before_peer);
            assert!(after.parent.is_none() && after.depends_on.is_empty());
            assert_eq!(
                execution::effect_receipts(s.conn(), &reg.witness)
                    .unwrap()
                    .len(),
                1
            );
            let outcome = ok(&mut s, |tx| {
                crate::admin_case_outcome::verify(tx, "crew-a", &case.id)
            });
            assert_ne!(outcome.phase, AdminCaseOutcomePhase::Resolved);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("crewd.db");
            s.conn()
                .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
                .unwrap();
            drop(s);
            let mut s = crewd_store::Store::open(&path).unwrap();
            assert_eq!(
                ok(&mut s, |tx| unlink(tx, &reg.witness, p.clone())),
                receipt
            );
            let mut changed = p.clone();
            changed.note.push_str(" changed");
            assert!(matches!(
                crate::run(&mut s, NOW, |tx| unlink(tx, &reg.witness, changed)),
                Err(crate::CollabError::Conflict(_))
            ));
            assert_eq!(
                execution::effect_receipts(s.conn(), &reg.witness)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn unlink_rejects_stale_peer_and_rolls_back_exact_event_if_receipt_fails() {
        let mut s = store();
        let (_, reg) = execution::tests::running_case_with_edge(&mut s, Some(Edge::Parent));
        let p = params(&s, Edge::Parent);
        let mut stale = p.clone();
        stale.expected_related_revision = "999".into();
        assert!(crate::run(&mut s, NOW, |tx| unlink(tx, &reg.witness, stale)).is_err());
        let before = work::get(s.conn(), &p.id).unwrap();
        s.conn().execute_batch("CREATE TRIGGER fail_unlink_receipt BEFORE INSERT ON meta WHEN NEW.key LIKE 'admin.case.unlink.v1:%' BEGIN SELECT RAISE(ABORT,'synthetic receipt failure'); END;").unwrap();
        assert!(crate::run(&mut s, NOW, |tx| unlink(tx, &reg.witness, p.clone())).is_err());
        assert_eq!(work::get(s.conn(), &p.id).unwrap(), before);
        assert!(
            execution::effect_receipts(s.conn(), &reg.witness)
                .unwrap()
                .is_empty()
        );
        s.conn()
            .execute_batch("DROP TRIGGER fail_unlink_receipt")
            .unwrap();
        let first = ok(&mut s, |tx| unlink(tx, &reg.witness, p.clone()));
        let mut absent = p;
        absent.request_id = uuid::Uuid::new_v4().to_string();
        absent.expected_revision = first.revision;
        assert!(crate::run(&mut s, NOW, |tx| unlink(tx, &reg.witness, absent)).is_err());
        assert_eq!(
            execution::effect_receipts(s.conn(), &reg.witness)
                .unwrap()
                .len(),
            1
        );
    }
}

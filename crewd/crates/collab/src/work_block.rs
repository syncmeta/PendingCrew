//! The public atomic adapter for existing pure-core block and continuation rules.
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, Sender, Tx, asks, crews, work};
use crewd_proto::{WorkBlockOn, WorkBlockParams, WorkBlockReply, WorkContinuation};
use crewd_store::object_refs::{self, ObjectKind};
use crewd_work::{BlockOn, Continuation, DefaultOption};
pub fn block(tx: &mut Tx<'_>, by: &Sender, p: WorkBlockParams) -> Result<WorkBlockReply> {
    if p.on.is_empty() || p.on.len() > 16 {
        return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
    }
    let revision = p
        .expected_revision
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == p.expected_revision)
        .ok_or_else(|| CollabError::Invalid(Refusal::new(Code::WorkBadRevision)))?;
    let reference = object_refs::get(tx.conn(), ObjectKind::Plan, work::number(&p.id)?)?
        .ok_or_else(|| CollabError::NotFound(Refusal::new(Code::WorkNotFound).arg("id", &p.id)))?;
    crews::refuse_archived(tx.conn(), &reference.crew_id)?;
    let actor = work::actor(tx.conn(), by, &reference.crew_id)?;
    let local = work::core_plan(tx.conn(), &reference)?;
    let before = asks::load_org(tx.conn())?;
    let item = before.item(local).ok_or_else(|| {
        crewd_store::StoreError::Invalid("block target lacks authoritative work".into())
    })?;
    work::check_revision(item, &p.id, revision)?;
    let on = p
        .on
        .into_iter()
        .map(|b| {
            Ok(match b {
                WorkBlockOn::Work { id } => {
                    BlockOn::Item(work::local_plan(tx.conn(), &reference.crew_id, &id)?)
                }
                WorkBlockOn::Human { id } => BlockOn::Ask(
                    work::local_reference(
                        tx.conn(),
                        ObjectKind::HumanTask,
                        &reference.crew_id,
                        &id,
                    )?
                    .parse()
                    .map_err(|_| {
                        crewd_store::StoreError::Invalid("block H core identity is invalid".into())
                    })?,
                ),
                WorkBlockOn::NewHuman { question, default } => BlockOn::NewAsk {
                    question,
                    default: default.map(|d| DefaultOption {
                        text: d.text,
                        after_ms: d.after_minutes.saturating_mul(60_000),
                    }),
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let rest = match p.can_continue {
        WorkContinuation::Nothing => Continuation::Nothing,
        WorkContinuation::Split { items } => {
            if items.is_empty()
                || items.len() > 20
                || items
                    .iter()
                    .any(|p| p.parent.is_some() || p.request_id.is_some() || p.channel_id.is_some())
            {
                return Err(CollabError::Invalid(Refusal::new(Code::WorkBadUpdate)));
            }
            Continuation::Split(
                items
                    .into_iter()
                    .map(|mut p| {
                        if p.crew_id.is_empty() {
                            p.crew_id = reference.crew_id.clone();
                        }
                        let inherits_owner = p.owner.is_none();
                        let mut item = work::prepare_item(tx.conn(), &actor, p)?;
                        if inherits_owner {
                            item.owner = None;
                        }
                        Ok(item)
                    })
                    .collect::<Result<_>>()?,
            )
        }
    };
    let result = asks::with_book(tx, &reference.crew_id, |b, now| {
        b.block(&actor, now, local, on, rest).map(|ap| ap.value)
    })?;
    let human_tasks = asks::announce_block_questions(tx, &result.asks)?;
    let mut split = Vec::with_capacity(result.split.len());
    for id in result.split {
        let r = crewd_store::work_organization::reference_for_core(
            tx.conn(),
            ObjectKind::Plan,
            &id.to_string(),
        )?
        .ok_or_else(|| {
            crewd_store::StoreError::Invalid("split work has no published identity".into())
        })?;
        work::changed(tx, by, &r.crew_id, &r.id())?;
        split.push(r.id());
    }
    work::changed(tx, by, &reference.crew_id, &p.id)?;
    Ok(WorkBlockReply {
        item: work::get(tx.conn(), &p.id)?,
        human_tasks,
        split,
    })
}

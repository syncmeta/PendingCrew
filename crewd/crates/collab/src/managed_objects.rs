//! Organization-readable managed objects, with scoped write authorization.
//! The caller must first authenticate the Human principal or exact attached
//! session, and revalidate the connection and grants in this transaction.
use crate::{
    CollabError, Result, Sender, Tx, crews,
    refusal::{Code, Refusal},
    resource_acl, sessions,
};
use crewd_proto::ResourceActor;
use crewd_proto::managed_objects::{
    Change, Decision, DecisionAbout, DecisionDraft, DecisionPage, DecisionStatus,
    DecisionWithdrawal, Lookup, Object, Receipt, RowPage, Scope, Submit, TablePage,
};
use crewd_store::{StoreError, managed_decisions as decisions_db, managed_objects as db};
use rusqlite::Connection;

#[path = "managed_risks_lessons.rs"]
mod risks_lessons;
pub use risks_lessons::{lessons, risks};

const PAGE_LIMIT: u16 = 16;

#[derive(Clone, Copy)]
pub enum Actor<'a> {
    /// Trusted scope key, derived from the current qualified local or paired
    /// Human connection. Never accept this value from RPC parameters.
    Human(&'a str),
    Session(&'a str),
}
impl Actor<'_> {
    pub(crate) fn principal(self, c: &Connection) -> Result<String> {
        let (tag, id) = match self {
            Self::Human(principal) => ("human", principal),
            Self::Session(id) => {
                let actor = sessions::get(c, id)?;
                crews::get(c, &actor.crew_id)?;
                ("session", id)
            }
        };
        if id.is_empty() || id.len() > 500 || id.chars().any(char::is_control) {
            return Err(forbidden());
        }
        Ok(format!("{tag}:{id}"))
    }

    pub(crate) fn owner(self, c: &Connection, crew: &str, write: bool) -> Result<String> {
        let principal = self.principal(c)?;
        let sender = match self {
            Self::Human(_) => Sender::Human,
            Self::Session(id) => {
                // The new organization read privilege does not let an ordinary
                // or management session write objects owned by another crew.
                if sessions::get(c, id)?.crew_id != crew {
                    return Err(forbidden());
                }
                Sender::Session(id.into())
            }
        };
        resource_acl::actor(c, &sender, crew, write)?;
        Ok(principal)
    }
}

fn invalid(field: &str) -> CollabError {
    resource_acl::invalid(field)
}
fn forbidden() -> CollabError {
    CollabError::Forbidden(Refusal::new(Code::ResourceForbidden))
}
fn missing(id: &str) -> CollabError {
    CollabError::NotFound(Refusal::new(Code::ResourceNotFound).arg("id", id))
}
fn conflict(request: &str) -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ResourceRequestConflict).arg("request", request))
}
fn number(value: &str, field: &str) -> Result<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == value)
        .ok_or_else(|| invalid(field))
}
fn object_number(value: &str, prefix: char, field: &str) -> Result<i64> {
    number(
        value.strip_prefix(prefix).ok_or_else(|| invalid(field))?,
        field,
    )
}

pub fn get(c: &Connection, actor: Actor<'_>, id: &str) -> Result<Object> {
    get_with_defaults(c, actor, id, false)
}

pub fn get_with_defaults(
    c: &Connection,
    actor: Actor<'_>,
    id: &str,
    include_system_defaults: bool,
) -> Result<Object> {
    actor.principal(c)?;
    if id.starts_with('T') {
        let number = object_number(id, 'T', "id")?;
        v1_read_guard(c, number, false)?;
        Ok(Object::Table {
            table: db::table(c, number)?.ok_or_else(|| missing(id))?,
        })
    } else if id.starts_with('D') {
        Ok(Object::Decision {
            decision: decisions_db::get(c, object_number(id, 'D', "id")?)?
                .filter(|decision| {
                    include_system_defaults || decision.author.kind != "systemDefault"
                })
                .ok_or_else(|| missing(id))?,
        })
    } else if id.starts_with('R') || id.starts_with('L') {
        risks_lessons::get(c, id)
    } else {
        let number = object_number(id, 'O', "id")?;
        v1_read_guard(c, number, true)?;
        Ok(Object::Row {
            row: db::row(c, number)?.ok_or_else(|| missing(id))?,
        })
    }
}

/// Current object existence/read permission without forcing a V2 object into
/// the closed legacy Object codec. T/O remain organization-readable.
pub(crate) fn read_authority(c: &Connection, actor: Actor<'_>, id: &str) -> Result<String> {
    actor.principal(c)?;
    if crewd_store::managed_tables_v2::enabled(c)? {
        if id.starts_with('T') {
            return crewd_store::managed_tables_v2::table(c, object_number(id, 'T', "id")?)?
                .map(|t| t.crew_id)
                .ok_or_else(|| missing(id));
        }
        if id.starts_with('O') {
            return crewd_store::managed_tables_v2::row_base(c, object_number(id, 'O', "id")?)?
                .map(|(t, _, _, _)| t.crew_id)
                .ok_or_else(|| missing(id));
        }
    }
    Ok(get_with_defaults(c, actor, id, true)?.crew_id().into())
}

pub fn tables(c: &Connection, actor: Actor<'_>, after: Option<&str>) -> Result<TablePage> {
    actor.principal(c)?;
    let after = after
        .map(|id| object_number(id, 'T', "after"))
        .transpose()?
        .unwrap_or(0);
    let numbers = db::table_numbers(c, after, PAGE_LIMIT + 1)?;
    let next_after = (numbers.len() > PAGE_LIMIT as usize)
        .then(|| format!("T{}", numbers[PAGE_LIMIT as usize - 1]));
    let items = numbers
        .into_iter()
        .take(PAGE_LIMIT as usize)
        .map(|number| db::table(c, number)?.ok_or_else(|| missing(&format!("T{number}"))))
        .collect::<Result<_>>()?;
    Ok(TablePage { items, next_after })
}

pub fn rows(c: &Connection, actor: Actor<'_>, table: &str, after: Option<&str>) -> Result<RowPage> {
    actor.principal(c)?;
    let table_number = object_number(table, 'T', "tableId")?;
    v1_read_guard(c, table_number, false)?;
    let table = db::table(c, table_number)?.ok_or_else(|| missing(table))?;
    let after = after
        .map(|id| object_number(id, 'O', "after"))
        .transpose()?
        .unwrap_or(0);
    let numbers = db::row_numbers(c, table_number, after, PAGE_LIMIT + 1)?;
    let next_after = (numbers.len() > PAGE_LIMIT as usize)
        .then(|| format!("O{}", numbers[PAGE_LIMIT as usize - 1]));
    let items = numbers
        .into_iter()
        .take(PAGE_LIMIT as usize)
        .map(|number| db::row(c, number)?.ok_or_else(|| missing(&format!("O{number}"))))
        .collect::<Result<_>>()?;
    Ok(RowPage {
        table,
        items,
        next_after,
    })
}

pub fn decisions(c: &Connection, actor: Actor<'_>, after: Option<&str>) -> Result<DecisionPage> {
    decisions_with_defaults(c, actor, after, false)
}

pub fn decisions_with_defaults(
    c: &Connection,
    actor: Actor<'_>,
    after: Option<&str>,
    include_system_defaults: bool,
) -> Result<DecisionPage> {
    actor.principal(c)?;
    let after = after
        .map(|id| object_number(id, 'D', "after"))
        .transpose()?;
    let numbers = decisions_db::numbers(c, after, PAGE_LIMIT + 1, include_system_defaults)?;
    let next_after = (numbers.len() > PAGE_LIMIT as usize)
        .then(|| format!("D{}", numbers[PAGE_LIMIT as usize - 1]));
    let items = numbers
        .into_iter()
        .take(PAGE_LIMIT as usize)
        .map(|number| decisions_db::get(c, number)?.ok_or_else(|| missing(&format!("D{number}"))))
        .collect::<Result<_>>()?;
    Ok(DecisionPage { items, next_after })
}

fn decision_actor(actor: Actor<'_>) -> ResourceActor {
    match actor {
        Actor::Human(_) => ResourceActor {
            kind: "human".into(),
            session_id: None,
        },
        Actor::Session(id) => ResourceActor {
            kind: "session".into(),
            session_id: Some(id.into()),
        },
    }
}
fn decision_target(
    c: &Connection,
    actor: Actor<'_>,
    crew: &str,
    id: &str,
) -> Result<(i64, Decision)> {
    let number = object_number(id, 'D', "decisionId")?;
    let decision = decisions_db::get(c, number)?.ok_or_else(|| missing(id))?;
    if decision.crew_id != crew {
        return Err(forbidden());
    }
    let sender = match actor {
        Actor::Human(_) => Sender::Human,
        Actor::Session(id) => {
            let own_author = decision.author.kind == "session"
                && decision.author.session_id.as_deref() == Some(id)
                && sessions::get(c, id)?.crew_id == crew;
            if !own_author && !crews::acts_as_captain_of(c, id, crew)? {
                return Err(forbidden());
            }
            Sender::Session(id.into())
        }
    };
    resource_acl::actor(c, &sender, crew, true).map_err(decision_write_refusal)?;
    Ok((number, decision))
}

fn active_decision(expected: &str, decision: &Decision) -> Result<i64> {
    let revision = number(expected, "expectedRevision")?;
    if decision.status != DecisionStatus::Active || decision.revision != expected {
        return Err(CollabError::Conflict(Refusal::new(
            Code::ManagedDecisionChanged,
        )));
    }
    Ok(revision)
}

/// Artifact edges use the source object's existing editing authority. Ledger
/// associations are annotations and leave the work state untouched.
pub(crate) fn artifact_link_authority(
    c: &Connection,
    actor: Actor<'_>,
    subject: &crewd_proto::managed_artifacts::Subject,
) -> Result<()> {
    use crewd_proto::managed_artifacts::Subject;
    actor.principal(c)?;
    let id = match subject {
        Subject::Work { id } => {
            crate::work::get(c, id)?;
            return Ok(());
        }
        Subject::Requirement { id } => {
            crate::requirements::get(c, id)?;
            return Ok(());
        }
        Subject::Decision { id }
        | Subject::Lesson { id }
        | Subject::Risk { id }
        | Subject::Row { id }
        | Subject::Table { id } => id,
    };
    if crewd_store::managed_tables_v2::enabled(c)? {
        match subject {
            Subject::Row { id } => {
                let (t, _, author, _) = crewd_store::managed_tables_v2::row_base(
                    c,
                    object_number(id, 'O', "subject")?,
                )?
                .ok_or_else(|| missing(id))?;
                return crate::managed_tables_v2::row_authority(c, actor, &t, &author);
            }
            Subject::Table { id } => {
                let t =
                    crewd_store::managed_tables_v2::table(c, object_number(id, 'T', "subject")?)?
                        .ok_or_else(|| missing(id))?;
                return crate::managed_tables_v2::schema_authority(c, actor, &t);
            }
            _ => (),
        }
    }
    let object = get_with_defaults(c, actor, id, true)?;
    match &object {
        Object::Decision { decision } => {
            decision_target(c, actor, &decision.crew_id, id)?;
        }
        Object::Risk { .. } | Object::Lesson { .. } => {
            risks_lessons::artifact_link_authority(c, actor, &object)?;
        }
        Object::Row { .. } | Object::Table { .. } => {
            actor.owner(c, object.crew_id(), true)?;
        }
    }
    Ok(())
}

fn decision_write_refusal(error: CollabError) -> CollabError {
    match error {
        CollabError::Conflict(ref refusal) if refusal.code == Code::CrewRemoved => {
            CollabError::Conflict(Refusal::new(Code::ResourceFrozen))
        }
        error => error,
    }
}

fn about_refusal(error: CollabError, id: &str) -> CollabError {
    match error {
        CollabError::Invalid(_) => invalid("about"),
        CollabError::NotFound(_) => missing(id),
        CollabError::Forbidden(_) => forbidden(),
        error => error,
    }
}

pub(crate) fn check_decision_draft(c: &Connection, draft: &DecisionDraft) -> Result<()> {
    if !decisions_db::draft_valid(draft) {
        return Err(invalid("draft"));
    }
    for reference in &draft.about {
        match reference {
            DecisionAbout::Crew { id } => {
                crews::get(c, id).map_err(|error| about_refusal(error, id))?;
            }
            DecisionAbout::Work { id } => {
                crate::work::get(c, id).map_err(|error| about_refusal(error, id))?;
            }
            DecisionAbout::Requirement { id } => {
                crate::requirements::get(c, id).map_err(|error| about_refusal(error, id))?;
            }
            DecisionAbout::Row { id } => {
                if crewd_store::managed_tables_v2::enabled(c)? {
                    crewd_store::managed_tables_v2::row_base(c, object_number(id, 'O', "about")?)?
                        .ok_or_else(|| missing(id))?;
                } else {
                    db::row(c, object_number(id, 'O', "about")?)?.ok_or_else(|| missing(id))?;
                }
            }
            DecisionAbout::Risk { id } => {
                risks_lessons::risk(c, id, "about")?;
            }
        }
    }
    Ok(())
}

pub fn scope(c: &Connection) -> Result<Scope> {
    let db_id = crewd_store::meta_get(c, "dbId")?
        .filter(|db| !db.is_empty() && db.len() <= 256 && !db.chars().any(char::is_control))
        .ok_or_else(forbidden)?;
    Ok(Scope {
        db_id,
        restore_epoch: crewd_store::personal_drafts::epoch(c)?,
    })
}

pub fn lookup(
    c: &Connection,
    actor: Actor<'_>,
    origin: &Scope,
    crew: &str,
    request: &str,
) -> Result<Lookup> {
    if !crewd_proto::valid_remote_uuid(request)
        || !crewd_proto::valid_remote_uuid(&origin.restore_epoch)
    {
        return Err(invalid("requestId/restoreEpoch"));
    }
    let (foreign, captain) = match actor {
        Actor::Session(id) if sessions::get(c, id)?.crew_id != crew => {
            (true, crews::acts_as_captain_of(c, id, crew)?)
        }
        _ => (false, false),
    };
    let principal = if foreign {
        crews::get(c, crew)?;
        actor.principal(c)?
    } else {
        actor.owner(c, crew, false)?
    };
    let current = scope(c)?;
    if origin.db_id != current.db_id {
        return Err(forbidden());
    }
    let recorded = db::receipt(c, crew, &principal, request)?
        .filter(|(_, receipt, confirmation)| {
            !foreign
                || *confirmation
                || (captain
                    && matches!(
                        &receipt.object,
                        Object::Decision { .. } | Object::Risk { .. } | Object::Lesson { .. }
                    ))
        })
        .map(|(_, receipt, _)| receipt);
    if foreign && !captain && recorded.is_none() {
        return Err(forbidden());
    }
    if origin.restore_epoch != current.restore_epoch {
        let receipt = recorded.filter(|r| r.scope == *origin);
        return Ok(Lookup::Historical { receipt });
    }
    Ok(match recorded {
        Some(receipt) if receipt.scope == *origin => Lookup::Known { receipt },
        Some(_) => Lookup::Historical { receipt: None },
        None => Lookup::NotFound,
    })
}

pub fn submit(tx: &mut Tx<'_>, actor: Actor<'_>, request: Submit) -> Result<Receipt> {
    if !crewd_proto::valid_remote_uuid(&request.request_id) {
        return Err(invalid("requestId"));
    }
    let c = tx.conn();
    let risk_or_lesson = risks_lessons::authorize(c, actor, &request.crew_id, &request.change)?;
    let member_confirmation = matches!(&request.change, Change::ConfirmLesson { .. });
    let target = match &request.change {
        Change::ReplaceDecision { decision_id, .. }
        | Change::WithdrawDecision { decision_id, .. } => {
            Some(decision_target(c, actor, &request.crew_id, decision_id)?)
        }
        _ => None,
    };
    let principal = if let Some((principal, _)) = &risk_or_lesson {
        principal.clone()
    } else if target.is_some() {
        actor.principal(c)?
    } else if matches!(&request.change, Change::RecordDecision { .. }) {
        actor
            .owner(c, &request.crew_id, true)
            .map_err(decision_write_refusal)?
    } else {
        actor.owner(c, &request.crew_id, true)?
    };
    if scope(c)? != request.scope {
        return Err(conflict(&request.request_id));
    }
    let digest: [u8; 32] = resource_acl::digest(&request)?
        .try_into()
        .map_err(|_| StoreError::Invalid("managed object digest length invalid".into()))?;
    if let Some((saved, receipt, _)) =
        db::receipt(c, &request.crew_id, &principal, &request.request_id)?
    {
        return if saved == digest && receipt.scope == request.scope {
            Ok(receipt)
        } else {
            Err(conflict(&request.request_id))
        };
    }
    let now = tx.now_ts();
    let mut occurrence = None;
    let mut replaced_decision_id = None;
    let object = match request.change {
        Change::CreateTable { title, fields } => {
            if !db::definition_valid(&title, &fields) {
                return Err(invalid("fields"));
            }
            Object::Table {
                table: db::create_table_by(
                    c,
                    &request.crew_id,
                    &title,
                    &fields,
                    &crate::managed_links::author(actor),
                    &now,
                )?,
            }
        }
        Change::AddRow { table_id, values } => {
            let table_number = object_number(&table_id, 'T', "tableId")?;
            if crewd_store::managed_tables_v2::enabled(c)? {
                crate::managed_tables_v2::v1_write(c, actor, table_number, None)?;
            }
            let table = db::table(c, table_number)?.ok_or_else(|| missing(&table_id))?;
            if table.crew_id != request.crew_id {
                return Err(forbidden());
            }
            if !db::values_valid(&table.fields, &values)? {
                return Err(invalid("values"));
            }
            Object::Row {
                row: db::add_row_by(
                    c,
                    &request.crew_id,
                    table_number,
                    &values,
                    &crate::managed_links::author(actor),
                    &now,
                )?,
            }
        }
        Change::EditRow {
            row_id,
            expected_revision,
            values,
        } => {
            let row_number = object_number(&row_id, 'O', "rowId")?;
            if crewd_store::managed_tables_v2::enabled(c)? {
                let (t, _, _, _) = crewd_store::managed_tables_v2::row_base(c, row_number)?
                    .ok_or_else(|| missing(&row_id))?;
                crate::managed_tables_v2::v1_write(
                    c,
                    actor,
                    crewd_store::managed_tables_v2::number(&t.id, 'T')?,
                    Some(row_number),
                )?;
            }
            let expected = number(&expected_revision, "expectedRevision")?;
            let row = db::row(c, row_number)?.ok_or_else(|| missing(&row_id))?;
            if row.crew_id != request.crew_id {
                return Err(forbidden());
            }
            if row.revision != expected_revision {
                return Err(CollabError::Conflict(Refusal::new(Code::ManagedRowChanged)));
            }
            let table_number = object_number(&row.table_id, 'T', "tableId")?;
            if crewd_store::managed_tables_v2::enabled(c)? {
                crate::managed_tables_v2::v1_write(c, actor, table_number, Some(row_number))?;
            }
            let table = db::table(c, table_number)?.ok_or_else(|| missing(&row.table_id))?;
            if !db::values_valid(&table.fields, &values)? {
                return Err(invalid("values"));
            }
            Object::Row {
                row: db::edit_row(c, row_number, expected, &values)?,
            }
        }
        Change::RecordDecision { draft } => {
            check_decision_draft(c, &draft)?;
            Object::Decision {
                decision: decisions_db::record(
                    c,
                    &request.crew_id,
                    &draft,
                    &decision_actor(actor),
                    &now,
                    None,
                )?,
            }
        }
        Change::ReplaceDecision {
            expected_revision,
            draft,
            ..
        } => {
            let (number, predecessor) = target.ok_or_else(|| invalid("decisionId"))?;
            let revision = active_decision(&expected_revision, &predecessor)?;
            check_decision_draft(c, &draft)?;
            decisions_db::supersede(c, number, revision)?;
            replaced_decision_id = Some(predecessor.id);
            Object::Decision {
                decision: decisions_db::record(
                    c,
                    &request.crew_id,
                    &draft,
                    &decision_actor(actor),
                    &now,
                    Some(number),
                )?,
            }
        }
        Change::WithdrawDecision {
            expected_revision,
            reason,
            ..
        } => {
            let (number, decision) = target.ok_or_else(|| invalid("decisionId"))?;
            let revision = active_decision(&expected_revision, &decision)?;
            if !decisions_db::valid_text(&reason, 1500) {
                return Err(invalid("reason"));
            }
            let withdrawal = DecisionWithdrawal {
                author: decision_actor(actor),
                reason,
                at: now.clone(),
            };
            Object::Decision {
                decision: decisions_db::withdraw(c, number, revision, &withdrawal)?,
            }
        }
        change => {
            let (_, target) = risk_or_lesson.ok_or_else(|| invalid("change"))?;
            let (object, effect) = risks_lessons::apply(
                c,
                actor,
                &request.crew_id,
                target,
                change,
                tx.now_ms(),
                &now,
            )?;
            occurrence = effect;
            object
        }
    };
    let receipt = Receipt {
        scope: request.scope,
        crew_id: request.crew_id,
        request_id: request.request_id,
        recorded_at: now,
        object,
        occurrence,
    };
    db::record(c, &principal, &digest, &receipt, member_confirmation)?;
    if let Actor::Session(session) = actor {
        sessions::note_progress(tx, session, crate::ProgressKind::Ontology)?;
    }
    tx.note(crate::Change::OntologyChanged {
        crew_id: receipt.crew_id.clone(),
        object_ids: affected_objects(&receipt, replaced_decision_id),
    });
    Ok(receipt)
}

fn affected_objects(receipt: &Receipt, replaced_decision_id: Option<String>) -> Vec<String> {
    match &receipt.object {
        Object::Table { table } => vec![table.id.clone()],
        Object::Row { row } => vec![row.table_id.clone(), row.id.clone()],
        Object::Decision { decision } => replaced_decision_id
            .into_iter()
            .chain(std::iter::once(decision.id.clone()))
            .collect(),
        Object::Risk { risk } => vec![risk.id.clone()],
        Object::Lesson { lesson } => match &receipt.occurrence {
            Some(effect) => vec![effect.risk_id.clone(), lesson.id.clone()],
            None => vec![lesson.id.clone()],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit(tx: &mut Tx<'_>, actor: Actor<'_>, mut request: Submit) -> Result<Receipt> {
        request.scope = scope(tx.conn())?;
        super::submit(tx, actor, request)
    }
    fn lookup(
        c: &Connection,
        actor: Actor<'_>,
        crew: &str,
        request: &str,
    ) -> Result<Option<Receipt>> {
        Ok(match super::lookup(c, actor, &scope(c)?, crew, request)? {
            Lookup::Known { receipt } => Some(receipt),
            Lookup::Historical { .. } | Lookup::NotFound => None,
        })
    }
    use crate::testkit::{NOW, count, err, ok};
    use crewd_proto::managed_objects::{Cell, Field, FieldType, Row, Table};
    use crewd_store::Store;

    const HUMAN: Actor<'static> = Actor::Human("local-anchor");
    const AUTHOR: Actor<'static> = Actor::Session("w-a1");
    const OTHER: Actor<'static> = Actor::Session("w-b1");

    fn store() -> Store {
        let store = crate::testkit::store();
        if crewd_store::meta_get(store.conn(), "dbId")
            .unwrap()
            .is_none()
        {
            crewd_store::meta_set(store.conn(), "dbId", "managed-in-memory-fixture").unwrap();
        }
        store
    }
    fn request(crew: &str, change: Change) -> Submit {
        Submit {
            scope: Scope {
                db_id: String::new(),
                restore_epoch: String::new(),
            },
            crew_id: crew.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change,
        }
    }
    fn definition() -> Change {
        Change::CreateTable {
            title: "部署记录".into(),
            fields: vec![
                Field {
                    name: "版本".into(),
                    field_type: FieldType::Text,
                    nullable: false,
                },
                Field {
                    name: "字节".into(),
                    field_type: FieldType::Integer,
                    nullable: true,
                },
            ],
        }
    }
    fn table(receipt: &Receipt) -> &Table {
        let Object::Table { table } = &receipt.object else {
            panic!("table receipt")
        };
        table
    }
    fn row(receipt: &Receipt) -> &Row {
        let Object::Row { row } = &receipt.object else {
            panic!("row receipt")
        };
        row
    }
    fn add(crew: &str, table_id: &str) -> Submit {
        request(
            crew,
            Change::AddRow {
                table_id: table_id.into(),
                values: vec![
                    Cell::Text("e\u{301}\n".into()),
                    Cell::Integer(i64::MIN.to_string()),
                ],
            },
        )
    }

    #[test]
    fn ontology_first_commit_notes_exact_actor_and_ids_but_replay_and_fault_do_not() {
        use crate::{Change as Fact, ProgressKind, testkit::ok_at};

        let mut s = store();
        let wake_count = count(&s, "SELECT count(*) FROM wakes");
        let message_count = count(&s, "SELECT count(*) FROM messages");
        let (created, facts) = ok_at(&mut s, NOW, |tx| {
            submit(tx, HUMAN, request("crew-a", definition()))
        });
        let table_id = table(&created).id.clone();
        let change = |object_ids| Fact::OntologyChanged {
            crew_id: "crew-a".into(),
            object_ids,
        };
        assert_eq!(facts, [change(vec![table_id.clone()])]);
        assert!(
            sessions::get(s.conn(), "w-a1")
                .unwrap()
                .last_progress_at
                .is_none()
        );

        let row_request = add("crew-a", &table_id);
        let (added, facts) = ok_at(&mut s, NOW + 1, |tx| {
            submit(tx, AUTHOR, row_request.clone())
        });
        let row_id = row(&added).id.clone();
        assert_eq!(
            facts,
            [
                Fact::Progress {
                    session_id: "w-a1".into(),
                    kind: ProgressKind::Ontology
                },
                change(vec![table_id.clone(), row_id.clone()]),
            ]
        );
        let progress = |store: &Store| {
            sessions::get(store.conn(), "w-a1")
                .unwrap()
                .last_progress_at
        };
        let causal =
            |store: &Store| crewd_store::meta_get(store.conn(), "causal.session.v1.w-a1").unwrap();
        let committed_progress = progress(&s);
        let committed_causal = causal(&s);
        assert_eq!(committed_progress, Some(crewd_store::ts_from_ms(NOW + 1)));
        let epoch: serde_json::Value =
            serde_json::from_str(committed_causal.as_ref().unwrap()).unwrap();
        assert_eq!(epoch["value"]["epoch"], 1);
        assert!(
            sessions::get(s.conn(), "w-b1")
                .unwrap()
                .last_progress_at
                .is_none()
        );

        let (replayed, facts) = ok_at(&mut s, NOW + 2, |tx| {
            submit(tx, AUTHOR, row_request.clone())
        });
        assert_eq!(replayed, added);
        assert!(facts.is_empty());
        let mut collision = row_request;
        collision.change = Change::AddRow {
            table_id: table_id.clone(),
            values: vec![Cell::Text("different".into()), Cell::Null],
        };
        assert!(matches!(
            crate::run(&mut s, NOW + 3, |tx| submit(tx, AUTHOR, collision)),
            Err(CollabError::Conflict(r)) if r.code == Code::ResourceRequestConflict
        ));

        let edit = request(
            "crew-a",
            Change::EditRow {
                row_id: row_id.clone(),
                expected_revision: "1".into(),
                values: vec![Cell::Text("edited".into()), Cell::Null],
            },
        );
        s.conn().execute_batch(
            "CREATE TRIGGER reject_ontology_progress BEFORE UPDATE OF last_progress_at ON sessions
             WHEN NEW.id='w-a1' BEGIN SELECT RAISE(ABORT,'synthetic progress fault'); END;",
        ).unwrap();
        assert!(crate::run(&mut s, NOW + 4, |tx| submit(tx, AUTHOR, edit.clone())).is_err());
        assert!(
            lookup(s.conn(), AUTHOR, "crew-a", &edit.request_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(get(s.conn(), OTHER, &row_id).unwrap(), added.object);
        assert_eq!(progress(&s), committed_progress);
        assert_eq!(causal(&s), committed_causal);
        s.conn()
            .execute_batch("DROP TRIGGER reject_ontology_progress")
            .unwrap();

        let (updated, facts) = ok_at(&mut s, NOW + 5, |tx| submit(tx, AUTHOR, edit.clone()));
        assert_eq!(row(&updated).revision, "2");
        assert_eq!(
            facts,
            [
                Fact::Progress {
                    session_id: "w-a1".into(),
                    kind: ProgressKind::Ontology
                },
                change(vec![table_id, row_id]),
            ]
        );
        let stale = Submit {
            request_id: uuid::Uuid::new_v4().to_string(),
            ..edit
        };
        assert!(matches!(
            crate::run(&mut s, NOW + 6, |tx| submit(tx, AUTHOR, stale)),
            Err(CollabError::Conflict(r)) if r.code == Code::ManagedRowChanged
        ));
        assert_eq!(progress(&s), Some(crewd_store::ts_from_ms(NOW + 5)));
        assert_eq!(count(&s, "SELECT count(*) FROM wakes"), wake_count);
        assert_eq!(count(&s, "SELECT count(*) FROM messages"), message_count);
    }

    #[test]
    fn organization_read_uses_exact_t_o_but_only_owner_writes_and_revisions_cas() {
        let mut s = store();
        let create = request("crew-a", definition());
        let made = ok(&mut s, |tx| submit(tx, AUTHOR, create.clone()));
        let t = table(&made);
        assert_eq!(get(s.conn(), OTHER, &t.id).unwrap(), made.object);
        assert_eq!(
            tables(s.conn(), OTHER, None).unwrap().items,
            std::slice::from_ref(t)
        );
        assert!(get(s.conn(), Actor::Session("untrusted-id"), &t.id).is_err());
        assert!(
            lookup(s.conn(), HUMAN, "crew-a", &create.request_id)
                .unwrap()
                .is_none()
        );

        let forbidden_add = add("crew-a", &t.id);
        assert!(matches!(
            err(&mut s, |tx| submit(tx, OTHER, forbidden_add)),
            CollabError::Forbidden(_)
        ));
        let wrong_source = add("crew-b", &t.id);
        assert!(matches!(
            err(&mut s, |tx| submit(tx, OTHER, wrong_source)),
            CollabError::Forbidden(_)
        ));

        let added = add("crew-a", &t.id);
        let original = ok(&mut s, |tx| submit(tx, AUTHOR, added.clone()));
        let first = row(&original);
        assert_ne!(first.id[1..], t.id[1..]);
        assert_eq!(get(s.conn(), OTHER, &first.id).unwrap(), original.object);
        assert_eq!(
            rows(s.conn(), OTHER, &t.id, None).unwrap().items,
            std::slice::from_ref(first)
        );
        let mut wrong = added.clone();
        wrong.change = Change::AddRow {
            table_id: t.id.clone(),
            values: vec![Cell::Text("alternate".into()), Cell::Null],
        };
        assert!(matches!(
            err(&mut s, |tx| submit(tx, AUTHOR, wrong)),
            CollabError::Conflict(_)
        ));

        let edit = request(
            "crew-a",
            Change::EditRow {
                row_id: first.id.clone(),
                expected_revision: "1".into(),
                values: vec![Cell::Text("é".into()), Cell::Null],
            },
        );
        let updated = ok(&mut s, |tx| submit(tx, HUMAN, edit.clone()));
        assert_eq!(row(&updated).id, first.id);
        assert_eq!(row(&updated).revision, "2");
        assert_eq!(
            lookup(s.conn(), AUTHOR, "crew-a", &added.request_id).unwrap(),
            Some(original.clone())
        );
        assert_eq!(ok(&mut s, |tx| submit(tx, AUTHOR, added)), original);
        let mut stale = edit.clone();
        stale.request_id = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            err(&mut s, |tx| submit(tx, HUMAN, stale)),
            CollabError::Conflict(_)
        ));
        assert_eq!(get(s.conn(), OTHER, &first.id).unwrap(), updated.object);
        let invalid = request(
            "crew-a",
            Change::AddRow {
                table_id: t.id.clone(),
                values: vec![Cell::Text("valid".into()), Cell::Integer("01".into())],
            },
        );
        assert!(matches!(
            err(&mut s, |tx| submit(tx, HUMAN, invalid)),
            CollabError::Invalid(_)
        ));
        assert_eq!(count(&s, "SELECT count(*) FROM managed_rows"), 1);
    }

    #[test]
    fn archive_frozen_then_purge_cascades_identity_content_and_receipt_without_reusing_number() {
        let mut s = store();
        let create = request("crew-x", definition());
        let original = ok(&mut s, |tx| submit(tx, HUMAN, create.clone()));
        let t = table(&original);
        let added = add("crew-x", &t.id);
        let first = ok(&mut s, |tx| submit(tx, HUMAN, added));
        let row_id = row(&first).id.clone();

        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        let Object::Table { table: frozen } = get(s.conn(), OTHER, &t.id).unwrap() else {
            panic!("frozen table")
        };
        assert!(frozen.frozen);
        assert!(rows(s.conn(), OTHER, &t.id, None).unwrap().items[0].frozen);
        assert!(crate::run(&mut s, NOW, |tx| submit(tx, HUMAN, add("crew-x", &t.id))).is_err());
        assert_eq!(
            lookup(s.conn(), HUMAN, "crew-x", &create.request_id).unwrap(),
            Some(original.clone())
        );

        ok(&mut s, |tx| crews::unarchive(tx, "crew-x"));
        assert!(!rows(s.conn(), OTHER, &t.id, None).unwrap().items[0].frozen);
        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crews::purge(tx, "crew-x"));
        assert!(get(s.conn(), OTHER, &t.id).is_err());
        assert!(get(s.conn(), OTHER, &row_id).is_err());
        assert!(lookup(s.conn(), HUMAN, "crew-x", &create.request_id).is_err());
        assert!(
            lookup(s.conn(), HUMAN, "crew-a", &create.request_id)
                .unwrap()
                .is_none()
        );
        assert_eq!(count(&s, "SELECT count(*) FROM managed_object_receipts"), 0);
        assert_eq!(count(&s, "SELECT count(*) FROM managed_tables"), 0);
        assert_eq!(count(&s, "SELECT count(*) FROM managed_rows"), 0);
        assert_eq!(
            count(
                &s,
                "SELECT count(*) FROM object_refs WHERE kind IN ('table','custom_row')"
            ),
            0
        );

        let next = ok(&mut s, |tx| {
            submit(
                tx,
                HUMAN,
                Submit {
                    crew_id: "crew-a".into(),
                    ..create
                },
            )
        });
        assert!(
            object_number(&table(&next).id, 'T', "id").unwrap()
                > object_number(&row_id, 'O', "id").unwrap()
        );
        assert_eq!(next.crew_id, "crew-a");
    }

    #[test]
    fn receipt_write_fault_rolls_back_payload_and_alias_then_exact_retry_can_commit() {
        let mut s = store();
        let create = request("crew-a", definition());
        s.conn()
            .execute_batch(
                "CREATE TRIGGER reject_managed_receipt BEFORE INSERT ON managed_object_receipts
             BEGIN SELECT RAISE(ABORT,'synthetic receipt fault'); END;",
            )
            .unwrap();
        assert!(crate::run(&mut s, NOW, |tx| submit(tx, HUMAN, create.clone())).is_err());
        assert_eq!(count(&s, "SELECT count(*) FROM managed_tables"), 0);
        assert_eq!(
            count(&s, "SELECT count(*) FROM object_refs WHERE kind='table'"),
            0
        );
        assert!(
            lookup(s.conn(), HUMAN, "crew-a", &create.request_id)
                .unwrap()
                .is_none()
        );
        s.conn()
            .execute_batch("DROP TRIGGER reject_managed_receipt")
            .unwrap();
        let result = ok(&mut s, |tx| submit(tx, HUMAN, create.clone()));
        assert_eq!(ok(&mut s, |tx| submit(tx, HUMAN, create)), result);
        assert_eq!(count(&s, "SELECT count(*) FROM managed_tables"), 1);
    }

    #[test]
    fn v22_upgrade_keeps_original_work_alias_and_global_number_space() {
        use crewd_store::object_refs::{self, ObjectKind};
        let dir = tempfile::tempdir().unwrap();
        let s = crewd_store::testing::open_legacy_store(22).unwrap();
        s.conn()
            .execute_batch(
                "INSERT INTO crews(id,title,created_at,updated_at) VALUES('crew-a','A','t','t');",
            )
            .unwrap();
        let prior = object_refs::bind(
            s.conn(),
            ObjectKind::Plan,
            "crew-a",
            "legacy-work",
            "original",
        )
        .unwrap();
        s.conn()
            .execute(
                "VACUUM INTO ?1",
                [dir.path().join(crewd_store::DB_FILE).to_str().unwrap()],
            )
            .unwrap();
        drop(s);
        let mut s = Store::open_data_dir(dir.path()).unwrap();
        let version: i64 = s
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, crewd_store::schema::SCHEMA_VERSION);
        assert_eq!(
            object_refs::get(s.conn(), ObjectKind::Plan, prior.number).unwrap(),
            Some(prior.clone())
        );
        let managed = ok(&mut s, |tx| {
            submit(tx, HUMAN, request("crew-a", definition()))
        });
        assert!(object_number(&table(&managed).id, 'T', "id").unwrap() > prior.number);
    }

    #[test]
    fn official_old_snapshot_keeps_purged_number_floor_and_historicizes_original_scope() {
        use crewd_store::backup;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().canonicalize().unwrap();
        let original_snapshot = data.join("original.db");
        let mut s = Store::open_data_dir(&data).unwrap();
        s.conn()
            .execute_batch(
                "INSERT INTO crews(id,title,created_at,updated_at) VALUES
              ('crew-a','A','t','t'),('crew-x','X','t','t');",
            )
            .unwrap();
        let mut saved_request = request("crew-a", definition());
        let original_authority = scope(s.conn()).unwrap();
        saved_request.scope = original_authority.clone();
        let saved = ok(&mut s, |tx| super::submit(tx, HUMAN, saved_request.clone()));
        crewd_store::meta_set(s.conn(), "knowledge.sqlite.v1", "1").unwrap();
        crewd_store::team_data::private_dir(&data.join("knowledge")).unwrap();
        drop(s);
        backup::backup_db_quiescent(&data.join(crewd_store::DB_FILE), &original_snapshot).unwrap();
        let mut s = Store::open_data_dir(&data).unwrap();

        let later = ok(&mut s, |tx| {
            submit(tx, HUMAN, request("crew-x", definition()))
        });
        let mut later_request = add("crew-x", &table(&later).id);
        later_request.scope = original_authority.clone();
        let issued = ok(&mut s, |tx| submit(tx, HUMAN, later_request.clone()));
        let issued_number = object_number(&row(&issued).id, 'O', "id").unwrap();
        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        ok(&mut s, |tx| crews::purge(tx, "crew-x"));
        assert!(get(s.conn(), HUMAN, &row(&issued).id).is_err());
        drop(s);

        let restored = backup::restore(&data, &original_snapshot).unwrap();
        assert!(restored.replaced.unwrap().is_file());
        let mut s = Store::open_data_dir(&data).unwrap();
        let current_authority = scope(s.conn()).unwrap();
        assert_eq!(current_authority.db_id, original_authority.db_id);
        assert_ne!(
            current_authority.restore_epoch,
            original_authority.restore_epoch
        );
        assert_eq!(
            super::lookup(
                s.conn(),
                HUMAN,
                &original_authority,
                "crew-a",
                &saved_request.request_id
            )
            .unwrap(),
            Lookup::Historical {
                receipt: Some(saved.clone())
            }
        );
        assert_eq!(
            super::lookup(
                s.conn(),
                HUMAN,
                &original_authority,
                "crew-x",
                &later_request.request_id
            )
            .unwrap(),
            Lookup::Historical { receipt: None }
        );
        assert_eq!(
            super::lookup(
                s.conn(),
                HUMAN,
                &current_authority,
                "crew-a",
                &saved_request.request_id
            )
            .unwrap(),
            Lookup::Historical { receipt: None }
        );
        assert!(crate::run(&mut s, NOW, |tx| super::submit(tx, HUMAN, later_request)).is_err());
        assert!(
            crate::run(&mut s, NOW, |tx| super::submit(
                tx,
                HUMAN,
                Submit {
                    scope: current_authority.clone(),
                    ..saved_request
                }
            ))
            .is_err()
        );
        let replacement = ok(&mut s, |tx| {
            submit(tx, HUMAN, request("crew-a", definition()))
        });
        assert!(object_number(&table(&replacement).id, 'T', "id").unwrap() > issued_number);
    }

    #[test]
    fn physical_reopen_preserves_original_receipt_and_ordinary_page_cut() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open_data_dir(dir.path()).unwrap();
        s.conn()
            .execute(
                "INSERT INTO crews(id,title,created_at,updated_at) VALUES('crew-a','A','t','t')",
                [],
            )
            .unwrap();
        let made = ok(&mut s, |tx| {
            submit(tx, HUMAN, request("crew-a", definition()))
        });
        let t = table(&made).id.clone();
        let first = add("crew-a", &t);
        let original = ok(&mut s, |tx| submit(tx, HUMAN, first.clone()));
        for _ in 1..=PAGE_LIMIT {
            ok(&mut s, |tx| submit(tx, HUMAN, add("crew-a", &t)));
        }
        drop(s);
        let s = Store::open_data_dir(dir.path()).unwrap();
        assert_eq!(
            lookup(s.conn(), HUMAN, "crew-a", &first.request_id).unwrap(),
            Some(original.clone())
        );
        let page = rows(s.conn(), HUMAN, &t, None).unwrap();
        assert_eq!(page.items.len(), PAGE_LIMIT as usize);
        assert_eq!(page.items[0], *row(&original));
        let tail = rows(s.conn(), HUMAN, &t, page.next_after.as_deref()).unwrap();
        assert_eq!(tail.items.len(), 1);
        assert!(tail.next_after.is_none());
        assert_eq!(
            count(&s, "SELECT count(*) FROM managed_rows"),
            i64::from(PAGE_LIMIT) + 1
        );
    }
}

#[cfg(test)]
mod decision_tests {
    use super::*;
    use crate::testkit::{count, err, ok, org_captain, org_worker, store};
    use crewd_proto::managed_objects::DecisionReversibility;
    use crewd_store::Store;

    const HUMAN: Actor<'static> = Actor::Human("local-anchor");
    const OWNER_CAPTAIN: Actor<'static> = Actor::Session("cap-a");
    const OWNER_AGENT: Actor<'static> = Actor::Session("w-a1");
    const OTHER_WORKER: Actor<'static> = Actor::Session("w-b1");

    fn fixture() -> Store {
        let store = store();
        if crewd_store::meta_get(store.conn(), "dbId")
            .unwrap()
            .is_none()
        {
            crewd_store::meta_set(store.conn(), "dbId", "managed-decision-fixture").unwrap();
        }
        store
    }
    fn draft(crew: &str, choice: &str) -> DecisionDraft {
        DecisionDraft {
            question: "现在采取哪个方案？".into(),
            choice: choice.into(),
            rationale: "已核对当前依赖。".into(),
            alternatives: None,
            reversibility: Some(DecisionReversibility::Reversible),
            about: vec![DecisionAbout::Crew { id: crew.into() }],
        }
    }
    fn request(s: &Store, crew: &str, change: Change) -> Submit {
        Submit {
            scope: scope(s.conn()).unwrap(),
            crew_id: crew.into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            change,
        }
    }
    fn saved(receipt: &Receipt) -> &Decision {
        let Object::Decision { decision } = &receipt.object else {
            panic!("not a decision")
        };
        decision
    }
    fn fetched(s: &Store, id: &str) -> Decision {
        let Object::Decision { decision } = get(s.conn(), OTHER_WORKER, id).unwrap() else {
            panic!("not a decision")
        };
        decision
    }

    #[test]
    fn ontology_pairs_decision_predecessor_and_occurrence_and_credits_foreign_confirmer() {
        use crate::{
            Change as Fact, ProgressKind,
            testkit::{NOW, ok_at},
        };
        use crewd_proto::managed_objects::{LessonDraft, RiskAbout, RiskDraft, RiskLevel};

        let mut s = fixture();
        let invalidation = |object_ids| Fact::OntologyChanged {
            crew_id: "crew-a".into(),
            object_ids,
        };
        let record = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("crew-a", "先暂停"),
            },
        );
        let first = ok(&mut s, |tx| submit(tx, HUMAN, record));
        let first_id = saved(&first).id.clone();
        let replacement = request(
            &s,
            "crew-a",
            Change::ReplaceDecision {
                decision_id: first_id.clone(),
                expected_revision: "1".into(),
                draft: draft("crew-a", "现在恢复"),
            },
        );
        let (next, facts) = ok_at(&mut s, NOW, |tx| submit(tx, OWNER_CAPTAIN, replacement));
        let next_id = saved(&next).id.clone();
        assert_eq!(
            facts,
            [
                Fact::Progress {
                    session_id: "cap-a".into(),
                    kind: ProgressKind::Ontology
                },
                invalidation(vec![first_id, next_id.clone()]),
            ]
        );
        let withdraw = request(
            &s,
            "crew-a",
            Change::WithdrawDecision {
                decision_id: next_id.clone(),
                expected_revision: "1".into(),
                reason: "等待证据".into(),
            },
        );
        let (_, facts) = ok_at(&mut s, NOW + 1, |tx| submit(tx, HUMAN, withdraw));
        assert_eq!(facts, [invalidation(vec![next_id])]);

        let risk = request(
            &s,
            "crew-a",
            Change::RecordRisk {
                owner: None,
                draft: RiskDraft {
                    summary: "旧版本会继续接收数据".into(),
                    impact: RiskLevel::High,
                    likelihood: None,
                    mitigation: None,
                    about: vec![RiskAbout::Crew {
                        id: "crew-a".into(),
                    }],
                    mitigated_by: vec![],
                },
            },
        );
        let (risk, facts) = ok_at(&mut s, NOW + 2, |tx| submit(tx, HUMAN, risk));
        let Object::Risk { risk } = risk.object else {
            panic!("risk")
        };
        assert_eq!(facts, [invalidation(vec![risk.id.clone()])]);
        let occurrence = request(
            &s,
            "crew-a",
            Change::RecordRiskOccurrence {
                risk_id: risk.id.clone(),
                expected_revision: "1".into(),
                lesson_draft: LessonDraft {
                    happened: "切换时旧版本继续接收数据。".into(),
                    lesson: "切换前停止旧版写入".into(),
                    detection: None,
                    learned_from: vec![],
                    evidence: vec![],
                    applies_to: vec![],
                },
            },
        );
        let (result, facts) = ok_at(&mut s, NOW + 3, |tx| submit(tx, HUMAN, occurrence));
        let effect = result.occurrence.unwrap();
        assert_eq!(
            facts,
            [invalidation(vec![risk.id, effect.lesson_id.clone()])]
        );

        let confirm = request(
            &s,
            "crew-a",
            Change::ConfirmLesson {
                lesson_id: effect.lesson_id.clone(),
                expected_revision: "1".into(),
            },
        );
        let (confirmed, facts) = ok_at(&mut s, NOW + 4, |tx| {
            submit(tx, OTHER_WORKER, confirm.clone())
        });
        assert_eq!(
            facts,
            [
                Fact::Progress {
                    session_id: "w-b1".into(),
                    kind: ProgressKind::Ontology
                },
                invalidation(vec![effect.lesson_id]),
            ]
        );
        let (repeat, facts) = ok_at(&mut s, NOW + 5, |tx| submit(tx, OTHER_WORKER, confirm));
        assert_eq!(confirmed, repeat);
        assert!(facts.is_empty());
        assert_eq!(
            sessions::get(s.conn(), "w-b1").unwrap().last_progress_at,
            Some(crewd_store::ts_from_ms(NOW + 4))
        );
    }

    #[test]
    fn decision_receipts_preserve_original_and_captain_replacement_then_withdrawal() {
        let mut s = fixture();
        let original = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("crew-b", "保留当前流程"),
            },
        );
        let original_receipt = ok(&mut s, |tx| submit(tx, HUMAN, original.clone()));
        let first = saved(&original_receipt);
        assert_eq!(fetched(&s, &first.id), first.clone());
        assert_eq!(first.author.kind, "human");
        let replacement = request(
            &s,
            "crew-a",
            Change::ReplaceDecision {
                decision_id: first.id.clone(),
                expected_revision: "1".into(),
                draft: draft("crew-b", "启用新流程"),
            },
        );
        assert!(matches!(
            err(&mut s, |tx| submit(tx, OWNER_AGENT, replacement.clone())),
            CollabError::Forbidden(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| submit(
                tx,
                Actor::Session("cap-b"),
                replacement.clone()
            )),
            CollabError::Forbidden(_)
        ));
        let replaced = ok(&mut s, |tx| submit(tx, OWNER_CAPTAIN, replacement.clone()));
        let next = saved(&replaced);
        assert_eq!(next.supersedes.as_deref(), Some(first.id.as_str()));
        assert_eq!(next.author.session_id.as_deref(), Some("cap-a"));
        let prior = fetched(&s, &first.id);
        assert_eq!(prior.draft, first.draft);
        assert_eq!(prior.status, DecisionStatus::Superseded);
        assert_eq!(prior.revision, "2");
        assert_eq!(prior.superseded_by.as_deref(), Some(next.id.as_str()));
        assert_eq!(
            ok(&mut s, |tx| submit(tx, OWNER_CAPTAIN, replacement.clone())),
            replaced
        );

        let mut competing = replacement;
        competing.request_id = uuid::Uuid::new_v4().to_string();
        assert!(
            matches!(err(&mut s,|tx|submit(tx,HUMAN,competing)),CollabError::Conflict(r) if r.code==Code::ManagedDecisionChanged)
        );
        assert_eq!(
            lookup(
                s.conn(),
                HUMAN,
                &original.scope,
                "crew-a",
                &original.request_id
            )
            .unwrap(),
            Lookup::Known {
                receipt: original_receipt.clone()
            }
        );

        let withdrawal = request(
            &s,
            "crew-a",
            Change::WithdrawDecision {
                decision_id: next.id.clone(),
                expected_revision: "1".into(),
                reason: "新事实推翻原假设。".into(),
            },
        );
        let withdrawn = ok(&mut s, |tx| submit(tx, OWNER_CAPTAIN, withdrawal.clone()));
        assert_eq!(saved(&withdrawn).status, DecisionStatus::Withdrawn);
        assert_eq!(
            saved(&withdrawn).withdrawal.as_ref().unwrap().reason,
            "新事实推翻原假设。"
        );
        assert_eq!(fetched(&s, &first.id).status, DecisionStatus::Superseded);
        assert_eq!(
            ok(&mut s, |tx| submit(tx, OWNER_CAPTAIN, withdrawal)),
            withdrawn
        );
    }

    #[test]
    fn agent_author_and_existing_management_captain_have_exact_permissions() {
        let mut s = fixture();
        let management = org_captain(&mut s);
        let worker = org_worker(&mut s);
        let original = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("crew-a", "开始"),
            },
        );
        let issued = ok(&mut s, |tx| submit(tx, OWNER_AGENT, original));
        let next = request(
            &s,
            "crew-a",
            Change::ReplaceDecision {
                decision_id: saved(&issued).id.clone(),
                expected_revision: "1".into(),
                draft: draft("crew-a", "重开"),
            },
        );
        let own = ok(&mut s, |tx| submit(tx, OWNER_AGENT, next));
        let management_request = request(
            &s,
            "crew-a",
            Change::WithdrawDecision {
                decision_id: saved(&own).id.clone(),
                expected_revision: "1".into(),
                reason: "不再生效。".into(),
            },
        );
        assert!(matches!(
            err(&mut s, |tx| submit(
                tx,
                Actor::Session(&worker),
                management_request.clone()
            )),
            CollabError::Forbidden(_)
        ));
        let committed = ok(&mut s, |tx| {
            submit(tx, Actor::Session(&management), management_request.clone())
        });
        assert_eq!(
            lookup(
                s.conn(),
                Actor::Session(&management),
                &management_request.scope,
                "crew-a",
                &management_request.request_id
            )
            .unwrap(),
            Lookup::Known { receipt: committed }
        );
    }

    #[test]
    fn invalid_about_and_receipt_failure_leave_predecessor_active() {
        let mut s = fixture();
        let request_a = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("missing-crew", "不允许"),
            },
        );
        assert!(
            matches!(err(&mut s,|tx|submit(tx,HUMAN,request_a)),CollabError::NotFound(r) if r.code==Code::ResourceNotFound)
        );
        let mut repeated = draft("crew-a", "不允许");
        repeated.about.push(repeated.about[0].clone());
        let request_b = request(&s, "crew-a", Change::RecordDecision { draft: repeated });
        assert!(
            matches!(err(&mut s,|tx|submit(tx,HUMAN,request_b)),CollabError::Invalid(r) if r.code==Code::ResourceInvalid)
        );
        let issued = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("crew-a", "原决策"),
            },
        );
        let prior = ok(&mut s, |tx| submit(tx, HUMAN, issued));
        let next = request(
            &s,
            "crew-a",
            Change::ReplaceDecision {
                decision_id: saved(&prior).id.clone(),
                expected_revision: "1".into(),
                draft: draft("crew-a", "替换"),
            },
        );
        s.conn()
            .execute_batch(
                "CREATE TRIGGER reject_decision_receipt BEFORE INSERT ON managed_object_receipts
            BEGIN SELECT RAISE(ABORT,'fixture'); END;",
            )
            .unwrap();
        err(&mut s, |tx| submit(tx, HUMAN, next.clone()));
        assert_eq!(fetched(&s, &saved(&prior).id), saved(&prior).clone());
        assert_eq!(count(&s, "SELECT count(*) FROM managed_decisions"), 1);
        assert_eq!(
            count(&s, "SELECT count(*) FROM object_refs WHERE kind='decision'"),
            1
        );
        assert_eq!(
            lookup(s.conn(), HUMAN, &next.scope, "crew-a", &next.request_id).unwrap(),
            Lookup::NotFound
        );
    }

    #[test]
    fn latest_decision_page_includes_the_last_allocatable_number() {
        let mut s = fixture();
        let earlier = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("crew-a", "较早"),
            },
        );
        let older = ok(&mut s, |tx| submit(tx, HUMAN, earlier));
        s.conn()
            .execute(
                "UPDATE sqlite_sequence SET seq=?1 WHERE name='object_refs'",
                [i64::MAX - 1],
            )
            .unwrap();
        let last = request(
            &s,
            "crew-a",
            Change::RecordDecision {
                draft: draft("crew-a", "最后"),
            },
        );
        let newest = ok(&mut s, |tx| submit(tx, HUMAN, last));
        assert_eq!(saved(&newest).id, format!("D{}", i64::MAX));
        assert_eq!(
            decisions(s.conn(), HUMAN, None).unwrap().items[0].id,
            saved(&newest).id
        );
        assert_eq!(
            decisions(s.conn(), HUMAN, Some(&saved(&newest).id))
                .unwrap()
                .items[0]
                .id,
            saved(&older).id
        );
    }

    #[test]
    fn decision_chain_is_frozen_readable_then_purged_and_page_is_newest_first() {
        let mut s = fixture();
        let first = request(
            &s,
            "crew-x",
            Change::RecordDecision {
                draft: draft("crew-x", "旧"),
            },
        );
        let one = ok(&mut s, |tx| submit(tx, HUMAN, first.clone()));
        let change = request(
            &s,
            "crew-x",
            Change::ReplaceDecision {
                decision_id: saved(&one).id.clone(),
                expected_revision: "1".into(),
                draft: draft("crew-x", "新"),
            },
        );
        let two = ok(&mut s, |tx| submit(tx, HUMAN, change));
        let ids = decisions(s.conn(), OTHER_WORKER, None)
            .unwrap()
            .items
            .into_iter()
            .map(|d| d.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, [saved(&two).id.clone(), saved(&one).id.clone()]);
        let earlier = decisions(s.conn(), OTHER_WORKER, Some(&saved(&two).id)).unwrap();
        assert_eq!(earlier.items[0].id, saved(&one).id);
        ok(&mut s, |tx| crews::archive(tx, "crew-x"));
        assert!(fetched(&s, &saved(&two).id).frozen);
        let denied = request(
            &s,
            "crew-x",
            Change::WithdrawDecision {
                decision_id: saved(&two).id.clone(),
                expected_revision: "1".into(),
                reason: "已冻结".into(),
            },
        );
        assert!(
            matches!(err(&mut s,|tx|submit(tx,HUMAN,denied)),CollabError::Conflict(r) if r.code==Code::ResourceFrozen)
        );
        ok(&mut s, |tx| crews::purge(tx, "crew-x"));
        assert!(get(s.conn(), OTHER_WORKER, &saved(&one).id).is_err());
        assert!(get(s.conn(), OTHER_WORKER, &saved(&two).id).is_err());
        assert!(lookup(s.conn(), HUMAN, &first.scope, "crew-x", &first.request_id).is_err());
        assert_eq!(count(&s, "SELECT count(*) FROM managed_decisions"), 0);
        assert_eq!(count(&s, "SELECT count(*) FROM managed_object_receipts"), 0);
    }
}

fn v1_read_guard(c: &Connection, number: i64, is_row: bool) -> Result<()> {
    if crewd_store::managed_tables_v2::enabled(c)? {
        let table = if is_row {
            crewd_store::managed_tables_v2::row_base(c, number)?.map(|v| v.0)
        } else {
            crewd_store::managed_tables_v2::table(c, number)?
        };
        if table.is_some_and(|t| !t.v1_readable) {
            return Err(CollabError::Conflict(Refusal::new(
                Code::ManagedV1Unsupported,
            )));
        }
    }
    Ok(())
}

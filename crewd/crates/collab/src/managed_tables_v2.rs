//! Schema evolution and field-ID row patches. No positional authority in V2.
use crate::{
    CollabError, Result, Tx,
    managed_links::{self as links, ExternalTargets, ResolveMode},
    managed_objects::{self as objects, Actor},
    refusal::{Code, Refusal},
    resource_acl,
};
use crewd_proto::{managed_objects::Scope, managed_tables_v2::*};
use crewd_store::{managed_artifacts, managed_links as graph, managed_tables_v2 as db};
use links::{conflict, forbidden, invalid, missing, num};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
const PAGE: u16 = 16;
fn schema_changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedTableSchemaChanged))
}
fn row_changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedRowChanged))
}
pub fn schema_authority(c: &Connection, a: Actor<'_>, t: &Table) -> Result<()> {
    crew_writer(c, a, &t.crew_id)?;
    if !links::may_author(c, a, &t.crew_id, &t.author)? {
        return Err(forbidden());
    }
    Ok(())
}
pub fn crew_writer(c: &Connection, a: Actor<'_>, crew: &str) -> Result<()> {
    resource_acl::actor(c, &links::sender(a), crew, true)?;
    if let Actor::Session(s) = a
        && crate::sessions::get(c, s)?.crew_id != crew
        && !crate::crews::acts_as_captain_of(c, s, crew)?
    {
        return Err(forbidden());
    }
    Ok(())
}
pub fn row_authority(c: &Connection, a: Actor<'_>, t: &Table, author: &Author) -> Result<()> {
    crew_writer(c, a, &t.crew_id)?;
    if matches!(t.row_write_policy, RowWritePolicy::AuthorCaptainHuman)
        && !links::may_author(c, a, &t.crew_id, author)?
    {
        return Err(forbidden());
    }
    Ok(())
}
pub fn get(c: &Connection, a: Actor<'_>, id: &str, _: &dyn ExternalTargets) -> Result<Table> {
    a.principal(c)?;
    db::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id))
}
pub fn tables(
    c: &Connection,
    a: Actor<'_>,
    after: Option<&str>,
    x: &dyn ExternalTargets,
) -> Result<TablePage> {
    a.principal(c)?;
    let after = after.map(|s| num(s, 'T')).transpose()?.unwrap_or(0);
    let ns = db::numbers(c, None, after, PAGE + 1, false)?;
    let next_after = (ns.len() > PAGE as usize).then(|| format!("T{}", ns[PAGE as usize - 1]));
    let items = ns
        .into_iter()
        .take(PAGE as usize)
        .map(|n| get(c, a, &format!("T{n}"), x))
        .collect::<Result<_>>()?;
    Ok(TablePage { items, next_after })
}
fn hidden(
    c: &Connection,
    a: Actor<'_>,
    r: &str,
    t: &Table,
    revision: &str,
    field: &Field,
    edges: &[graph::Edge],
) -> Result<String> {
    let principal = a.principal(c)?;
    let ids = edges.iter().map(|e| (&e.id, &e.target)).collect::<Vec<_>>();
    Ok(graph::mac(
        c,
        "managed-column-hidden.v1",
        &serde_json::to_vec(&(
            objects::scope(c)?,
            principal,
            r,
            &t.id,
            &field.id,
            revision,
            ids,
        ))?,
    )?)
}
pub fn row(c: &Connection, a: Actor<'_>, id: &str, x: &dyn ExternalTargets) -> Result<Row> {
    a.principal(c)?;
    let n = num(id, 'O')?;
    let (t, revision, author, values) = db::row_base(c, n)?.ok_or_else(|| missing(id))?;
    get(c, a, &t.id, x)?;
    let source = SourceRef::Row { id: id.into() };
    let all = graph::outgoing(c, &source)?;
    let mut projection = links::Projection::new(c, a, x);
    let mut scalar_cells = Vec::new();
    let mut link_cells = Vec::new();
    for f in t.fields.iter().filter(|f| !f.hidden) {
        match &f.kind {
            FieldKind::Link { .. } => {
                let mut visible = Vec::new();
                let mut restricted = Vec::new();
                let origin = format!("column:{}", f.id);
                let es = all
                    .iter()
                    .filter(|e| e.origin == origin)
                    .cloned()
                    .collect::<Vec<_>>();
                for e in &es {
                    match projection.resolve(&e.target, ResolveMode::Read) {
                        Ok(v) => visible.push(VisibleLink {
                            edge_id: e.id.to_string(),
                            target: v.target,
                            summary: v.summary,
                        }),
                        Err(CollabError::Forbidden(_) | CollabError::NotFound(_)) => {
                            restricted.push(e.clone())
                        }
                        Err(e) => return Err(e),
                    }
                }
                let hidden_set = if restricted.is_empty() {
                    None
                } else {
                    Some(HiddenSet {
                        keep_token: hidden(c, a, id, &t, &revision, f, &restricted)?,
                    })
                };
                link_cells.push(LinkCell {
                    field_id: f.id.clone(),
                    visible,
                    hidden_set,
                    incomplete_required_link: !f.nullable && es.is_empty(),
                });
            }
            _ => scalar_cells.push(ScalarValue {
                field_id: f.id.clone(),
                value: values
                    .iter()
                    .find(|v| v.field_id == f.id)
                    .map(|v| v.value.clone())
                    .unwrap_or(Cell::Null),
            }),
        }
    }
    Ok(Row {
        id: id.into(),
        crew_id: t.crew_id,
        table_id: t.id,
        schema_revision: t.schema_revision,
        revision,
        author,
        scalar_cells,
        link_cells,
        frozen: t.frozen,
    })
}
pub fn rows(
    c: &Connection,
    a: Actor<'_>,
    id: &str,
    after: Option<&str>,
    x: &dyn ExternalTargets,
) -> Result<RowPage> {
    let table = get(c, a, id, x)?;
    let after = after.map(|s| num(s, 'O')).transpose()?.unwrap_or(0);
    let ns = db::numbers(c, Some(num(id, 'T')?), after, PAGE + 1, false)?;
    let next_after = (ns.len() > PAGE as usize).then(|| format!("O{}", ns[PAGE as usize - 1]));
    let items = ns
        .into_iter()
        .take(PAGE as usize)
        .map(|n| row(c, a, &format!("O{n}"), x))
        .collect::<Result<_>>()?;
    Ok(RowPage {
        table,
        items,
        next_after,
    })
}
fn constraint(c: &Connection, t: &TargetRef, k: &TargetConstraint) -> Result<bool> {
    Ok(match (k, t) {
        (TargetConstraint::AnyReadable {}, _) => true,
        (TargetConstraint::Work {}, TargetRef::Work { .. })
        | (TargetConstraint::Requirement {}, TargetRef::Requirement { .. })
        | (TargetConstraint::Ask {}, TargetRef::Ask { .. })
        | (TargetConstraint::Table {}, TargetRef::Table { .. })
        | (TargetConstraint::Decision {}, TargetRef::Decision { .. })
        | (TargetConstraint::Risk {}, TargetRef::Risk { .. })
        | (TargetConstraint::Lesson {}, TargetRef::Lesson { .. })
        | (TargetConstraint::Artifact {}, TargetRef::Artifact { .. })
        | (TargetConstraint::Crew {}, TargetRef::Crew { .. })
        | (TargetConstraint::Session {}, TargetRef::Session { .. })
        | (TargetConstraint::Channel {}, TargetRef::Channel { .. })
        | (TargetConstraint::Message {}, TargetRef::Message { .. })
        | (TargetConstraint::Human {}, TargetRef::Human {})
        | (TargetConstraint::Document {}, TargetRef::Document { .. })
        | (TargetConstraint::KnowledgeView {}, TargetRef::Knowledge { .. }) => true,
        (TargetConstraint::Row { table_id }, TargetRef::Row { id }) => {
            table_id.as_ref().is_none_or(|wanted| {
                db::row_base(c, db::number(id, 'O').unwrap_or(0))
                    .ok()
                    .flatten()
                    .is_some_and(|(t, _, _, _)| t.id == *wanted)
            })
        }
        _ => false,
    })
}
fn field_valid(f: &FieldDraft) -> bool {
    db::text(&f.name, 120)
}
fn field_available(
    c: &Connection,
    a: Actor<'_>,
    f: &FieldDraft,
    x: &dyn ExternalTargets,
) -> Result<()> {
    if let FieldKind::Link { target, .. } = &f.kind {
        match target {
            TargetConstraint::Document {} | TargetConstraint::KnowledgeView {}
                if !x.supports_knowledge() =>
            {
                return Err(invalid("unsupportedKnowledgeTarget"));
            }
            TargetConstraint::Row { table_id: Some(id) } => {
                links::resolve(c, a, &TargetRef::Table { id: id.clone() }, x)?;
            }
            _ => (),
        }
    }
    Ok(())
}

fn scalar_same(a: &Cell, b: &Cell) -> bool {
    match (a, b) {
        (Cell::Real(a), Cell::Real(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}
struct Applied {
    cells: Vec<ScalarValue>,
    changed: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
    deps: Vec<i64>,
}
struct ApplyInput<'a> {
    actor: Actor<'a>,
    table: &'a Table,
    row_id: &'a str,
    revision: &'a str,
    values: Vec<ScalarValue>,
    patches: &'a [Patch],
    new: bool,
    external: &'a dyn ExternalTargets,
    now: &'a str,
}
fn apply(c: &Connection, input: ApplyInput<'_>) -> Result<Applied> {
    let ApplyInput {
        actor: a,
        table: t,
        row_id,
        revision,
        values,
        patches,
        new,
        external: x,
        now,
    } = input;
    if patches.len() > 64
        || patches
            .iter()
            .map(|p| &p.field_id)
            .collect::<HashSet<_>>()
            .len()
            != patches.len()
    {
        return Err(invalid("patches"));
    }
    let mut cells = values;
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut deps = Vec::new();
    let source = SourceRef::Row { id: row_id.into() };
    let all = graph::outgoing(c, &source)?;
    let mut projection = links::Projection::new(c, a, x);
    let supplied = patches
        .iter()
        .map(|p| (p.field_id.as_str(), &p.change))
        .collect::<HashMap<_, _>>();
    for p in patches {
        if !t.fields.iter().any(|f| f.id == p.field_id) {
            return Err(invalid("fieldId"));
        }
    }
    for f in &t.fields {
        let patch = supplied.get(f.id.as_str()).copied();
        if f.hidden && patch.is_some_and(|p| !matches!(p, CellPatch::Keep {})) {
            return Err(invalid("hiddenField"));
        }
        match &f.kind {
            FieldKind::Link { target, many } => {
                let origin = format!("column:{}", f.id);
                let existing = all
                    .iter()
                    .filter(|e| e.origin == origin)
                    .cloned()
                    .collect::<Vec<_>>();
                let Some(CellPatch::ReplaceLinks {
                    values,
                    keep_hidden_set_token,
                }) = patch
                else {
                    if patch.is_some_and(|p| !matches!(p, CellPatch::Keep {})) {
                        return Err(invalid("linkPatch"));
                    }
                    if new && !f.nullable {
                        return Err(invalid("requiredLink"));
                    }
                    continue;
                };
                if values.len() > if *many { 64 } else { 1 } {
                    return Err(invalid("links"));
                }
                let mut visible = Vec::new();
                let mut restricted = Vec::new();
                for e in &existing {
                    match projection.resolve(&e.target, ResolveMode::Read) {
                        Ok(_) => visible.push(e.clone()),
                        Err(CollabError::Forbidden(_) | CollabError::NotFound(_)) => {
                            restricted.push(e.clone())
                        }
                        Err(e) => return Err(e),
                    }
                }
                if restricted.is_empty() {
                    if keep_hidden_set_token.is_some() {
                        return Err(links::changed());
                    }
                } else {
                    if keep_hidden_set_token.as_deref()
                        != Some(hidden(c, a, row_id, t, revision, f, &restricted)?.as_str())
                    {
                        return Err(links::changed());
                    }
                    if values.len() > visible.len() {
                        return Err(invalid("restrictedLinkSet"));
                    }
                }
                if !f.nullable && values.is_empty() && restricted.is_empty() {
                    return Err(invalid("requiredLink"));
                }
                let mut targets = Vec::new();
                let mut keys = HashSet::new();
                for value in values {
                    let manual = match value {
                        LinkWrite::Reference { target } => ManualTargetWrite::Reference {
                            target: target.clone(),
                        },
                        LinkWrite::DeclareArtifact { input } => {
                            ManualTargetWrite::DeclareArtifact {
                                input: input.clone(),
                            }
                        }
                    };
                    let target_ref = projection.declare(&t.crew_id, &manual, now)?;
                    if !constraint(c, &target_ref, target)? {
                        return Err(invalid("targetType"));
                    }
                    let key = graph::target_key(&target_ref)?;
                    if !keys.insert(key) {
                        return Err(invalid("duplicateTarget"));
                    }
                    if let Some(n) = graph::target_number(&target_ref)? {
                        deps.push(n)
                    }
                    targets.push(target_ref)
                }
                let mut field_changed = false;
                for e in visible {
                    if !targets
                        .iter()
                        .any(|v| graph::target_key(v).ok() == graph::target_key(&e.target).ok())
                    {
                        graph::remove(c, e.id)?;
                        removed.push(e.id.to_string());
                        field_changed = true
                    }
                }
                for target in targets {
                    let (id, created) = graph::link(c, &t.crew_id, &source, &target, &origin, now)?;
                    if created {
                        added.push(id.to_string());
                        field_changed = true
                    }
                }
                if field_changed {
                    changed.push(f.id.clone())
                }
            }
            _ => {
                let current = cells.iter().position(|v| v.field_id == f.id);
                match patch {
                    Some(CellPatch::SetScalar { value }) => {
                        if !db::scalar_valid(f, value) {
                            return Err(invalid("value"));
                        }
                        let different = !scalar_same(
                            current.map(|i| &cells[i].value).unwrap_or(&Cell::Null),
                            value,
                        );
                        if different {
                            if let Some(i) = current {
                                cells[i].value = value.clone()
                            } else {
                                cells.push(ScalarValue {
                                    field_id: f.id.clone(),
                                    value: value.clone(),
                                })
                            }
                            changed.push(f.id.clone())
                        }
                    }
                    Some(CellPatch::ReplaceLinks { .. }) => return Err(invalid("scalarPatch")),
                    _ => {
                        if new {
                            if !f.nullable {
                                return Err(invalid("requiredField"));
                            }
                            cells.push(ScalarValue {
                                field_id: f.id.clone(),
                                value: Cell::Null,
                            })
                        }
                    }
                }
            }
        }
    }
    if serde_json::to_vec(&cells)?.len() > 65536 {
        return Err(invalid("rowBytes"));
    }
    Ok(Applied {
        cells,
        changed,
        added,
        removed,
        deps,
    })
}
pub fn submit(
    tx: &mut Tx<'_>,
    a: Actor<'_>,
    r: Submit,
    x: &dyn ExternalTargets,
) -> Result<Receipt> {
    let c = tx.conn();
    let p = links::request(c, a, &r.scope, &r.crew_id, &r.request_id, false)?;
    let digest = graph::intent(
        TABLE_SUBMIT,
        &r.scope,
        &r.crew_id,
        &r.request_id,
        &r.operation,
    )?;
    if let Some(saved) = graph::receipt(c, &r.crew_id, &p, &r.request_id)? {
        return match saved {
            graph::ReadReceipt::Active(s) if s.method == TABLE_SUBMIT && s.digest == digest => {
                Ok(serde_json::from_value(s.result)?)
            }
            _ => Err(conflict(&r.request_id)),
        };
    }
    if objects::scope(c)? != r.scope {
        return Err(schema_changed());
    }
    resource_acl::actor(c, &links::sender(a), &r.crew_id, true)?;
    let now = tx.now_ts();
    let mut deps = Vec::new();
    let mut schema_audit: Option<(Table, Table)> = None;
    let (mutated, effect) = match &r.operation {
        Operation::CreateTable {
            title,
            purpose,
            fields,
            title_field_index,
            row_write_policy,
        } => {
            if !db::text(title, 256)
                || !db::text(purpose, 1500)
                || fields.is_empty()
                || fields.len() > 64
                || fields.iter().any(|f| !field_valid(f))
                || fields.iter().map(|f| &f.name).collect::<HashSet<_>>().len() != fields.len()
            {
                return Err(invalid("fields"));
            }
            for f in fields {
                field_available(c, a, f, x)?;
            }
            a.owner(c, &r.crew_id, true)?;
            let index = usize::from(*title_field_index);
            if index >= fields.len() || matches!(fields[index].kind, FieldKind::Link { .. }) {
                return Err(invalid("titleFieldIndex"));
            }
            let fs = fields
                .iter()
                .enumerate()
                .map(|(i, f)| Field {
                    id: format!("p{}", i + 1),
                    name: f.name.clone(),
                    kind: f.kind.clone(),
                    nullable: f.nullable,
                    hidden: false,
                })
                .collect::<Vec<_>>();
            let policy = match row_write_policy {
                NewRowWritePolicy::AuthorCaptainHuman => RowWritePolicy::AuthorCaptainHuman,
                NewRowWritePolicy::OwnerCrewWriters => RowWritePolicy::OwnerCrewWriters,
            };
            let t = db::create(
                c,
                db::NewTable {
                    crew: &r.crew_id,
                    title,
                    purpose: Some(purpose),
                    fields: &fs,
                    title_field: &fs[index].id,
                    policy,
                    author: &links::author(a),
                    now: &now,
                },
            )?;
            deps.push(num(&t.id, 'T')?);
            (
                true,
                Effect::TableCreated {
                    table_id: t.id,
                    schema_revision: t.schema_revision,
                    field_ids: fs.iter().map(|f| f.id.clone()).collect(),
                    title_field_id: fs[index].id.clone(),
                },
            )
        }
        Operation::AlterTable {
            table_id,
            expected_schema_revision,
            change,
        } => {
            let mut t = get(c, a, table_id, x)?;
            if t.crew_id != r.crew_id {
                return Err(forbidden());
            }
            schema_authority(c, a, &t)?;
            if t.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            let before = t.clone();
            let mut affected = Vec::new();
            match change {
                SchemaChange::AddField { field } => {
                    if t.fields.len() >= 64
                        || !field_valid(field)
                        || t.fields.iter().any(|f| f.name == field.name)
                    {
                        return Err(invalid("field"));
                    }
                    if !field.nullable
                        && !db::numbers(c, Some(num(table_id, 'T')?), 0, 1, false)?.is_empty()
                    {
                        return Err(invalid("requiredNewField"));
                    }
                    field_available(c, a, field, x)?;
                    let n = db::revision(&t.next_field_number)?;
                    let id = format!("p{n}");
                    t.next_field_number = n
                        .checked_add(1)
                        .ok_or_else(|| invalid("fieldId"))?
                        .to_string();
                    t.fields.push(Field {
                        id: id.clone(),
                        name: field.name.clone(),
                        kind: field.kind.clone(),
                        nullable: field.nullable,
                        hidden: false,
                    });
                    affected.push(id)
                }
                SchemaChange::RenameTable { title } => {
                    if !db::text(title, 256) {
                        return Err(invalid("title"));
                    }
                    t.title = title.clone()
                }
                SchemaChange::SetPurpose { purpose } => {
                    if !db::text(purpose, 1500) {
                        return Err(invalid("purpose"));
                    }
                    t.purpose = Some(purpose.clone())
                }
                SchemaChange::SetTitleField { field_id } => {
                    if !t.fields.iter().any(|f| {
                        f.id == *field_id && !f.hidden && !matches!(f.kind, FieldKind::Link { .. })
                    }) {
                        return Err(invalid("titleFieldId"));
                    }
                    t.title_field_id = field_id.clone();
                    affected.push(field_id.clone())
                }
                SchemaChange::RenameField { field_id, name } => {
                    if !db::text(name, 120)
                        || t.fields
                            .iter()
                            .any(|f| f.id != *field_id && f.name == *name)
                    {
                        return Err(invalid("name"));
                    }
                    let f = t
                        .fields
                        .iter_mut()
                        .find(|f| f.id == *field_id)
                        .ok_or_else(|| invalid("fieldId"))?;
                    f.name = name.clone();
                    affected.push(field_id.clone())
                }
                SchemaChange::HideField { field_id } | SchemaChange::RestoreField { field_id } => {
                    let hide = matches!(change, SchemaChange::HideField { .. });
                    if hide && t.title_field_id == *field_id {
                        return Err(invalid("titleFieldId"));
                    }
                    let f = t
                        .fields
                        .iter_mut()
                        .find(|f| f.id == *field_id)
                        .ok_or_else(|| invalid("fieldId"))?;
                    if hide && !f.nullable {
                        return Err(invalid("requiredHiddenField"));
                    }
                    f.hidden = hide;
                    affected.push(field_id.clone())
                }
                SchemaChange::WidenSingleLink { field_id } => {
                    let f = t
                        .fields
                        .iter_mut()
                        .find(|f| f.id == *field_id)
                        .ok_or_else(|| invalid("fieldId"))?;
                    match &mut f.kind {
                        FieldKind::Link { many, .. } => *many = true,
                        _ => return Err(invalid("linkField")),
                    }
                    affected.push(field_id.clone())
                }
            }
            let mutated = t != before;
            let old = db::revision(&before.schema_revision)?;
            if mutated {
                t.schema_revision = old
                    .checked_add(1)
                    .ok_or_else(|| invalid("schemaRevision"))?
                    .to_string();
                db::save_table(c, &t, old)?;
                schema_audit = Some((before.clone(), t.clone()));
            }
            deps.push(num(table_id, 'T')?);
            (
                mutated,
                Effect::SchemaChanged {
                    table_id: table_id.clone(),
                    original_schema_revision: before.schema_revision,
                    result_schema_revision: t.schema_revision,
                    changed_field_ids: if mutated { affected } else { Vec::new() },
                    changed: mutated,
                },
            )
        }
        Operation::AddRow {
            table_id,
            expected_schema_revision,
            patches,
        } => {
            let t = get(c, a, table_id, x)?;
            if t.crew_id != r.crew_id {
                return Err(forbidden());
            }
            crew_writer(c, a, &t.crew_id)?;
            if t.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            let n = db::add(c, &t, &links::author(a), &[], &now)?;
            let id = format!("O{n}");
            let effect = apply(
                c,
                ApplyInput {
                    actor: a,
                    table: &t,
                    row_id: &id,
                    revision: "1",
                    values: Vec::new(),
                    patches,
                    new: true,
                    external: x,
                    now: &now,
                },
            )?;
            c.execute(
                "UPDATE managed_rows SET values_json=?2 WHERE number=?1",
                rusqlite::params![n, serde_json::to_string(&effect.cells)?],
            )?;
            deps.extend(effect.deps);
            deps.extend([num(table_id, 'T')?, n]);
            (
                true,
                Effect::RowAdded {
                    table_id: table_id.clone(),
                    row_id: id,
                    schema_revision: t.schema_revision,
                    row_revision: "1".into(),
                    edge_ids: effect.added,
                },
            )
        }
        Operation::PatchRow {
            row_id,
            expected_schema_revision,
            expected_row_revision,
            patches,
        } => {
            let n = num(row_id, 'O')?;
            let (t, rev, author, values) = db::row_base(c, n)?.ok_or_else(|| missing(row_id))?;
            if t.crew_id != r.crew_id {
                return Err(forbidden());
            }
            row_authority(c, a, &t, &author)?;
            if t.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            if rev != *expected_row_revision {
                return Err(row_changed());
            }
            let effect = apply(
                c,
                ApplyInput {
                    actor: a,
                    table: &t,
                    row_id,
                    revision: &rev,
                    values,
                    patches,
                    new: false,
                    external: x,
                    now: &now,
                },
            )?;
            let mutated = !effect.changed.is_empty();
            let result = if mutated {
                db::save_row(c, n, db::revision(&rev)?, &effect.cells)?;
                db::revision(&rev)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("rowRevision"))?
                    .to_string()
            } else {
                rev.clone()
            };
            deps.extend(effect.deps);
            deps.extend([num(&t.id, 'T')?, n]);
            (
                mutated,
                Effect::RowPatched {
                    table_id: t.id,
                    row_id: row_id.clone(),
                    schema_revision: t.schema_revision,
                    original_row_revision: rev,
                    result_row_revision: result,
                    changed_field_ids: effect.changed,
                    added_edge_ids: effect.added,
                    removed_edge_ids: effect.removed,
                    changed: mutated,
                },
            )
        }
    };
    let kind = match &r.operation {
        Operation::CreateTable { .. } => OperationKind::CreateTable,
        Operation::AlterTable { .. } => OperationKind::AlterTable,
        Operation::AddRow { .. } => OperationKind::AddRow,
        Operation::PatchRow { .. } => OperationKind::PatchRow,
    };
    let receipt = Receipt {
        scope: r.scope.clone(),
        crew_id: r.crew_id.clone(),
        request_id: r.request_id.clone(),
        recorded_at: now,
        operation_kind: kind,
        intent_digest: digest.clone(),
        event_id: mutated.then(|| uuid::Uuid::new_v4().to_string()),
        effect,
    };
    graph::record(
        c,
        graph::ReceiptInput {
            crew: &r.crew_id,
            principal: &p,
            uuid: &r.request_id,
            method: TABLE_SUBMIT,
            digest: &digest,
            input: &r,
            result: &receipt,
            objects: &deps,
        },
    )?;
    if let Some((before, after)) = schema_audit {
        c.execute(
            "INSERT INTO managed_table_schema_events VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                receipt
                    .event_id
                    .as_ref()
                    .ok_or_else(|| invalid("eventId"))?,
                num(&after.id, 'T')?,
                serde_json::to_string(&before)?,
                serde_json::to_string(&after)?
            ],
        )?;
    }
    if mutated {
        links::finish(tx, a, &r.crew_id)?;
        managed_artifacts::gc(tx.conn())?;
    }
    Ok(receipt)
}
pub fn lookup(
    c: &Connection,
    a: Actor<'_>,
    scope: &Scope,
    crew: &str,
    uuid: &str,
) -> Result<RequestLookup> {
    let p = links::request(c, a, scope, crew, uuid, false)?;
    let current = objects::scope(c)?;
    let mut retired = false;
    let receipt = match graph::receipt(c, crew, &p, uuid)? {
        Some(graph::ReadReceipt::Active(s)) if s.method == TABLE_SUBMIT => {
            Some(serde_json::from_value::<Receipt>(s.result)?)
        }
        Some(graph::ReadReceipt::Active(_)) => return Err(conflict(uuid)),
        Some(graph::ReadReceipt::Retired) => {
            retired = true;
            None
        }
        None => None,
    };
    let receipt = receipt.filter(|r| r.scope == *scope);
    Ok(if current != *scope || retired {
        RequestLookup::Historical { receipt }
    } else {
        match receipt {
            Some(receipt) => RequestLookup::Known { receipt },
            None => RequestLookup::NotFound {},
        }
    })
}
/// Called only after the original V1 receipt fast path, for fresh positional writes.
pub fn v1_write(c: &Connection, a: Actor<'_>, table: i64, row: Option<i64>) -> Result<()> {
    let t = db::table(c, table)?.ok_or_else(|| missing("table"))?;
    if !t.v1_writable {
        return Err(CollabError::Conflict(Refusal::new(
            Code::ManagedV1Unsupported,
        )));
    }
    if let Some(n) = row {
        let (_, _, author, _) = db::row_base(c, n)?.ok_or_else(|| missing("row"))?;
        row_authority(c, a, &t, &author)?;
    } else {
        crew_writer(c, a, &t.crew_id)?;
    }
    Ok(())
}
#[cfg(test)]
#[path = "managed_tables_v2_tests.rs"]
mod tests;

//! Typed free-table values and lifecycle. All decisions remain transactional;
//! V3 does not create a parallel WorkBook, SQL table, graph or receipt authority.
use crate::{
    CollabError, Result, Tx,
    managed_links::{self as links, ExternalTargets},
    managed_objects::{self as objects, Actor},
    managed_tables_v2 as old_domain,
    refusal::{Code, Refusal},
};
use crewd_proto::{
    ResourceActor, managed_objects::Scope, managed_tables_v2 as v2, managed_tables_v3::*,
};
use crewd_store::{managed_links as graph, managed_tables_v2 as old_db, managed_tables_v3 as db};
use db::values;
use links::{conflict, forbidden, invalid, missing, num};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
const PAGE: u16 = 16;
fn schema_changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedTableSchemaChanged))
}
fn row_changed() -> CollabError {
    CollabError::Conflict(Refusal::new(Code::ManagedRowChanged))
}
fn actor_view(a: Actor<'_>) -> ResourceActor {
    match a {
        Actor::Human(_) => ResourceActor {
            kind: "human".into(),
            session_id: None,
        },
        Actor::Session(s) => ResourceActor {
            kind: "session".into(),
            session_id: Some(s.into()),
        },
    }
}
fn table_writable(t: &Table) -> Result<()> {
    if t.frozen_reasons.is_empty() {
        Ok(())
    } else {
        Err(forbidden())
    }
}
fn schema_authority(c: &Connection, a: Actor<'_>, t: &Table) -> Result<()> {
    old_domain::crew_writer(c, a, &t.crew_id)?;
    if !links::may_author(c, a, &t.crew_id, &t.author)? {
        return Err(forbidden());
    }
    Ok(())
}
fn row_authority(c: &Connection, a: Actor<'_>, t: &Table, author: &v2::Author) -> Result<()> {
    old_domain::crew_writer(c, a, &t.crew_id)?;
    if matches!(t.row_write_policy, v2::RowWritePolicy::AuthorCaptainHuman)
        && !links::may_author(c, a, &t.crew_id, author)?
    {
        return Err(forbidden());
    }
    Ok(())
}
/// Permission-only bridge. It never serializes a V3 object through an older
/// closed Table/Row codec, and does not turn organization read into write.
pub(crate) fn read_authority(c: &Connection, a: Actor<'_>, id: &str) -> Result<String> {
    a.principal(c)?;
    if id.starts_with('T') {
        return db::table(c, num(id, 'T')?)?
            .map(|t| t.crew_id)
            .ok_or_else(|| missing(id));
    }
    db::row_base(c, num(id, 'O')?)?
        .map(|r| r.table.crew_id)
        .ok_or_else(|| missing(id))
}
pub(crate) fn write_authority(c: &Connection, a: Actor<'_>, id: &str) -> Result<()> {
    if id.starts_with('T') {
        let table = db::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id))?;
        schema_authority(c, a, &table)?;
        table_writable(&table)
    } else {
        let row = db::row_base(c, num(id, 'O')?)?.ok_or_else(|| missing(id))?;
        row_authority(c, a, &row.table, &row.author)?;
        table_writable(&row.table)?;
        if row.lifecycle.archived_at.is_some() {
            return Err(forbidden());
        }
        Ok(())
    }
}
pub fn get(c: &Connection, a: Actor<'_>, id: &str, _x: &dyn ExternalTargets) -> Result<Table> {
    a.principal(c)?;
    db::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id))
}
pub fn tables(
    c: &Connection,
    a: Actor<'_>,
    q: TableList,
    x: &dyn ExternalTargets,
) -> Result<TablePage> {
    a.principal(c)?;
    let after = q
        .after
        .as_deref()
        .map(|s| num(s, 'T'))
        .transpose()?
        .unwrap_or(0);
    let ns = db::numbers(c, None, after, PAGE + 1, q.include_archived)?;
    let mut items = Vec::new();
    let mut bytes = 2;
    let mut next = None;
    for n in ns {
        let t = get(c, a, &format!("T{n}"), x)?;
        let size = serde_json::to_vec(&t)?.len();
        if items.len() >= PAGE as usize || bytes + size > TABLE_PAGE_ENCODED_BYTES - 1024 {
            if items.is_empty() {
                return Err(invalid("tableBytes"));
            }
            next = items.last().map(|t: &Table| t.id.clone());
            break;
        }
        bytes += size + 1;
        items.push(t);
    }
    Ok(TablePage {
        items,
        next_after: next,
    })
}
pub fn row(c: &Connection, a: Actor<'_>, id: &str, x: &dyn ExternalTargets) -> Result<Row> {
    a.principal(c)?;
    let r = db::row_base(c, num(id, 'O')?)?.ok_or_else(|| missing(id))?;
    get(c, a, &r.table.id, x)?;
    let mut frozen = r.table.frozen_reasons.clone();
    if r.lifecycle.archived_at.is_some() {
        frozen.push(FrozenReason::RowArchived {})
    }
    Ok(Row {
        id: id.into(),
        crew_id: r.table.crew_id,
        table_id: r.table.id,
        schema_revision: r.table.schema_revision,
        revision: r.revision,
        author: r.author,
        scalar_cells: r
            .table
            .fields
            .iter()
            .filter(|f| !f.hidden && !values::graph_field(f))
            .map(|f| ScalarValue {
                field_id: f.id.clone(),
                value: r
                    .logical
                    .iter()
                    .find(|v| v.field_id == f.id)
                    .map(|v| v.value.clone())
                    .unwrap_or(Cell::Null),
            })
            .collect(),
        link_field_ids: r
            .table
            .fields
            .iter()
            .filter(|f| !f.hidden && values::graph_field(f))
            .map(|f| f.id.clone())
            .collect(),
        lifecycle: r.lifecycle,
        frozen_reasons: frozen,
    })
}
pub fn rows(c: &Connection, a: Actor<'_>, q: RowList, x: &dyn ExternalTargets) -> Result<RowPage> {
    let table = get(c, a, &q.table_id, x)?;
    let after = q
        .after
        .as_deref()
        .map(|s| num(s, 'O'))
        .transpose()?
        .unwrap_or(0);
    let ns = db::numbers(
        c,
        Some(num(&q.table_id, 'T')?),
        after,
        PAGE + 1,
        q.include_archived,
    )?;
    let mut bytes = serde_json::to_vec(&table)?.len() + 2048;
    let mut items = Vec::new();
    let mut next = None;
    for n in ns {
        let r = row(c, a, &format!("O{n}"), x)?;
        let size = serde_json::to_vec(&r)?.len();
        if items.len() >= PAGE as usize || bytes + size > ROW_PAGE_ENCODED_BYTES {
            if items.is_empty() {
                return Err(invalid("rowBytes"));
            }
            next = items.last().map(|r: &Row| r.id.clone());
            break;
        }
        bytes += size + 1;
        items.push(r);
    }
    Ok(RowPage {
        table,
        items,
        next_after: next,
    })
}
/// Only graph fields are adapted to the existing graph algorithm. New scalar
/// kinds never enter the old closed Cell codec or its positional authority.
fn graph_table(t: &Table) -> v2::Table {
    v2::Table {
        id: t.id.clone(),
        crew_id: t.crew_id.clone(),
        schema_revision: t.schema_revision.clone(),
        title: t.title.clone(),
        purpose: t.purpose.clone(),
        title_field_id: t.title_field_id.clone(),
        row_write_policy: t.row_write_policy,
        author: t.author.clone(),
        fields: t
            .fields
            .iter()
            .filter_map(|f| {
                let (target, many) = match &f.kind {
                    FieldKind::Link { target, many } => (target.clone(), *many),
                    FieldKind::Person { .. } => (v2::TargetConstraint::AnyReadable {}, false),
                    _ => return None,
                };
                Some(v2::Field {
                    id: f.id.clone(),
                    name: f.name.clone(),
                    nullable: f.nullable,
                    hidden: f.hidden,
                    kind: v2::FieldKind::Link { target, many },
                })
            })
            .collect(),
        next_field_number: t.next_field_number.clone(),
        frozen: !t.frozen_reasons.is_empty(),
        v1_readable: false,
        v1_writable: false,
    }
}
pub fn row_links(
    c: &Connection,
    a: Actor<'_>,
    q: RowLinks,
    x: &dyn ExternalTargets,
) -> Result<RowLinkCell> {
    let r = db::row_base(c, num(&q.row_id, 'O')?)?.ok_or_else(|| missing(&q.row_id))?;
    get(c, a, &r.table.id, x)?;
    if r.table.schema_revision != q.expected_schema_revision {
        return Err(schema_changed());
    }
    if r.revision != q.expected_row_revision {
        return Err(row_changed());
    }
    let f = r
        .table
        .fields
        .iter()
        .find(|f| f.id == q.field_id && !f.hidden && values::graph_field(f))
        .ok_or_else(|| invalid("fieldId"))?;
    let t = graph_table(&r.table);
    let field = t
        .fields
        .iter()
        .find(|v| v.id == f.id)
        .ok_or_else(|| invalid("fieldId"))?;
    let cell = old_domain::link_cell(c, a, &q.row_id, &t, &r.revision, field, x)?;
    let result = RowLinkCell {
        scope: objects::scope(c)?,
        row_id: q.row_id,
        schema_revision: r.table.schema_revision,
        row_revision: r.revision,
        cell,
    };
    if serde_json::to_vec(&result)?.len() > ROW_LINK_CELL_ENCODED_BYTES {
        return Err(invalid("linkCellBytes"));
    }
    Ok(result)
}
fn check_request(c: &Connection, a: Actor<'_>, q: &DefineCheck) -> Result<(String, i64)> {
    let principal = a.principal(c)?;
    if q.excluding_table_id.is_some() {
        old_domain::crew_writer(c, a, &q.crew_id)?;
    } else {
        a.owner(c, &q.crew_id, true)?;
    }
    if objects::scope(c)? != q.scope {
        return Err(schema_changed());
    }
    if !old_db::text(&q.title, TITLE_SCALARS)
        || q.purpose
            .as_ref()
            .is_some_and(|p| !old_db::text(p, PURPOSE_SCALARS))
    {
        return Err(invalid("definition"));
    }
    if let Some(id) = &q.excluding_table_id {
        let t = db::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id))?;
        if t.crew_id != q.crew_id {
            return Err(forbidden());
        }
        schema_authority(c, a, &t)?;
        table_writable(&t)?;
        // Null is only a way to preserve a legitimate old table's absent
        // purpose on Rename, not an empty/sentinel purpose for fresh creation.
        if q.purpose.is_none() && t.purpose.is_some() {
            return Err(invalid("purpose"));
        }
    } else if q.purpose.is_none() {
        return Err(invalid("purpose"));
    }
    Ok((principal, db::definition_revision(c, &q.crew_id)?))
}
fn definition_digest(
    title: &str,
    purpose: Option<&str>,
    excluding: Option<&str>,
) -> Result<String> {
    Ok(graph::hash(&serde_json::to_vec(&(
        title, purpose, excluding,
    ))?))
}
fn definition_token(c: &Connection, domain: &str, p: &db::DefinitionProof) -> Result<String> {
    let token = graph::seal(c, domain, &serde_json::to_vec(p)?)?;
    if token.len() > 16384 {
        return Err(invalid("definitionTokenBytes"));
    }
    Ok(token)
}
pub fn define_check(c: &Connection, a: Actor<'_>, q: DefineCheck) -> Result<DefinitionCheck> {
    let (principal, revision) = check_request(c, a, &q)?;
    let query_digest = definition_digest(
        &q.title,
        q.purpose.as_deref(),
        q.excluding_table_id.as_deref(),
    )?;
    let mut proof = if let Some(cursor) = &q.cursor {
        let p: db::DefinitionProof = serde_json::from_slice(
            &graph::open(c, "managed-table-definition-cursor.v1", cursor)
                .map_err(|_| schema_changed())?,
        )?;
        if p.scope != q.scope
            || p.principal != principal
            || p.crew_id != q.crew_id
            || p.namespace_revision != revision
            || p.query_digest != query_digest
            || p.excluding_table_id != q.excluding_table_id
            || p.after <= 0
            || p.candidates.len() > NEAR_CANDIDATES_MAX
        {
            return Err(schema_changed());
        }
        p
    } else {
        db::DefinitionProof {
            scope: q.scope.clone(),
            principal,
            crew_id: q.crew_id.clone(),
            namespace_revision: revision,
            excluding_table_id: q.excluding_table_id.clone(),
            query_digest,
            after: 0,
            exact: false,
            candidates: Vec::new(),
            more: false,
        }
    };
    // Reserve eight point reads for the final, actual candidate header page.
    // Neither continuation nor submit stores/carries eight large purpose texts.
    let budget = CHECK_SOURCE_BUDGET - NEAR_CANDIDATES_MAX;
    let candidates = db::definition_candidates(c, &q.crew_id, proof.after, budget as u16)?;
    let has_more = candidates.len() == budget;
    for candidate in candidates {
        proof.after = num(&candidate.table_id, 'T')?;
        if Some(candidate.table_id.as_str()) == q.excluding_table_id.as_deref() {
            continue;
        }
        if candidate.title.eq_ignore_ascii_case(&q.title) {
            proof.exact = true
        }
        let title = db::similarity(&q.title, &candidate.title);
        let purpose = q
            .purpose
            .as_deref()
            .zip(candidate.purpose.as_deref())
            .map(|(a, b)| db::similarity(a, b))
            .unwrap_or((0, 1));
        if 6 * title.0 >= title.1 || 3 * purpose.0 >= purpose.1 {
            proof.candidates.push(db::DefinitionScore {
                table_id: candidate.table_id,
                title_intersection: title.0,
                title_sum: title.1,
                purpose_intersection: purpose.0,
                purpose_sum: purpose.1,
            });
            proof.candidates.sort_by(|a, b| {
                (6 * b.title_intersection >= b.title_sum)
                    .cmp(&(6 * a.title_intersection >= a.title_sum))
                    .then_with(|| {
                        (b.title_intersection * a.title_sum)
                            .cmp(&(a.title_intersection * b.title_sum))
                    })
                    .then_with(|| {
                        (b.purpose_intersection * a.purpose_sum)
                            .cmp(&(a.purpose_intersection * b.purpose_sum))
                    })
                    .then_with(|| {
                        num(&a.table_id, 'T')
                            .unwrap_or(0)
                            .cmp(&num(&b.table_id, 'T').unwrap_or(0))
                    })
            });
            if proof.candidates.len() > NEAR_CANDIDATES_MAX {
                proof.candidates.truncate(NEAR_CANDIDATES_MAX);
                proof.more = true
            }
        }
    }
    let mut near_candidates = Vec::new();
    if !has_more {
        for score in &proof.candidates {
            let n = num(&score.table_id, 'T')?;
            let mut row = db::definition_candidates(c, &q.crew_id, n - 1, 1)?;
            if row.len() != 1 || row[0].table_id != score.table_id {
                return Err(schema_changed());
            }
            near_candidates.push(row.remove(0));
        }
    }
    let next = if has_more {
        Some(definition_token(
            c,
            "managed-table-definition-cursor.v1",
            &proof,
        )?)
    } else {
        None
    };
    let token = if !has_more && !proof.exact {
        Some(definition_token(
            c,
            "managed-table-definition-proof.v1",
            &proof,
        )?)
    } else {
        None
    };
    Ok(DefinitionCheck {
        scope: q.scope,
        crew_id: q.crew_id,
        state: if has_more {
            CheckState::Checking
        } else {
            CheckState::Complete
        },
        exact_name_taken: proof.exact,
        near_candidates,
        more_similar: proof.more,
        definition_check_token: token,
        next_cursor: next,
    })
}

struct CheckIntent<'a> {
    crew: &'a str,
    title: &'a str,
    purpose: Option<&'a str>,
    excluding: Option<&'a str>,
    token: &'a str,
    confirm: bool,
}
fn consume_check(c: &Connection, a: Actor<'_>, r: CheckIntent<'_>) -> Result<()> {
    let proof: db::DefinitionProof = serde_json::from_slice(
        &graph::open(c, "managed-table-definition-proof.v1", r.token)
            .map_err(|_| schema_changed())?,
    )?;
    if proof.scope != objects::scope(c)?
        || proof.principal != a.principal(c)?
        || proof.crew_id != r.crew
        || proof.namespace_revision != db::definition_revision(c, r.crew)?
        || proof.excluding_table_id.as_deref() != r.excluding
        || proof.query_digest != definition_digest(r.title, r.purpose, r.excluding)?
        || proof.exact
    {
        return Err(schema_changed());
    }
    if (!proof.candidates.is_empty() || proof.more) && !r.confirm {
        return Err(invalid("confirmNew"));
    }
    Ok(())
}
pub(crate) fn legacy_definition(
    c: &Connection,
    crew: &str,
    title: &str,
    purpose: Option<&str>,
    excluding: Option<&str>,
) -> Result<()> {
    let candidates = db::definition_candidates(c, crew, 0, CHECK_SOURCE_BUDGET as u16)?;
    if candidates.len() == CHECK_SOURCE_BUDGET {
        return Err(CollabError::Conflict(Refusal::new(
            Code::ManagedTableDefinitionReviewRequired,
        )));
    }
    if candidates
        .iter()
        .filter(|t| Some(t.table_id.as_str()) != excluding)
        .any(|t| {
            t.title.eq_ignore_ascii_case(title)
                || db::near(title, &t.title, false)
                || purpose
                    .zip(t.purpose.as_deref())
                    .is_some_and(|(a, b)| db::near(a, b, true))
        })
    {
        return Err(CollabError::Conflict(Refusal::new(
            Code::ManagedTableDefinitionReviewRequired,
        )));
    }
    Ok(())
}
pub(crate) fn legacy_definition_gate(
    c: &Connection,
    a: Actor<'_>,
    crew: &str,
    operation: &v2::Operation,
) -> Result<()> {
    match operation {
        v2::Operation::CreateTable { title, purpose, .. } => {
            a.owner(c, crew, true)?;
            legacy_definition(c, crew, title, Some(purpose), None)
        }
        v2::Operation::AlterTable {
            table_id, change, ..
        } => {
            let t = db::table(c, num(table_id, 'T')?)?.ok_or_else(|| missing(table_id))?;
            schema_authority(c, a, &t)?;
            match change {
                v2::SchemaChange::RenameTable { title } if *title != t.title => {
                    legacy_definition(c, &t.crew_id, title, t.purpose.as_deref(), Some(table_id))
                }
                v2::SchemaChange::SetPurpose { purpose } if Some(purpose) != t.purpose.as_ref() => {
                    legacy_definition(c, &t.crew_id, &t.title, Some(purpose), Some(table_id))
                }
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    }
}
fn issue(next: &mut i64) -> Result<String> {
    let id = format!("e{next}");
    *next = next.checked_add(1).ok_or_else(|| invalid("optionId"))?;
    Ok(id)
}
fn field_from_draft(
    c: &Connection,
    a: Actor<'_>,
    draft: &FieldDraft,
    id: String,
    next: &mut i64,
    x: &dyn ExternalTargets,
) -> Result<Field> {
    if !old_db::text(&draft.name, FIELD_NAME_SCALARS) {
        return Err(invalid("fieldName"));
    }
    let kind = match &draft.kind {
        KindDraft::Text {} => FieldKind::Text {
            bounds: TextBounds {
                max_utf8_bytes: TEXT_UTF8_BYTES,
                max_unicode_scalars: Some(TEXT_SCALARS),
            },
        },
        KindDraft::LongText {} => FieldKind::LongText {
            bounds: TextBounds {
                max_utf8_bytes: TEXT_UTF8_BYTES,
                max_unicode_scalars: Some(LONG_TEXT_SCALARS),
            },
        },
        KindDraft::Integer {} => FieldKind::Integer {},
        KindDraft::Real {} => FieldKind::Real {},
        KindDraft::Decimal {
            unit,
            display_scale,
        } => {
            if unit.as_ref().is_some_and(|s| !old_db::text(s, 32))
                || display_scale.is_some_and(|s| s > DECIMAL_MAX_SCALE)
            {
                return Err(invalid("decimalDisplay"));
            }
            FieldKind::Decimal {
                precision: DECIMAL_PRECISION,
                max_scale: DECIMAL_MAX_SCALE,
                unit: unit.clone(),
                display_scale: *display_scale,
            }
        }
        KindDraft::SingleSelect { options } => {
            if options.len() > OPTION_ACTIVE_MAX
                || options.iter().any(|o| !old_db::text(&o.label, 120))
                || options
                    .iter()
                    .map(|o| o.label.to_ascii_lowercase())
                    .collect::<HashSet<_>>()
                    .len()
                    != options.len()
            {
                return Err(invalid("options"));
            }
            FieldKind::SingleSelect {
                options: options
                    .iter()
                    .map(|o| {
                        Ok(OptionDefinition {
                            id: issue(next)?,
                            label: o.label.clone(),
                            tone: o.tone,
                            active: true,
                        })
                    })
                    .collect::<Result<_>>()?,
            }
        }
        KindDraft::Date {} => FieldKind::Date {},
        KindDraft::Boolean {} => FieldKind::Boolean {},
        KindDraft::Url {} => FieldKind::Url {},
        KindDraft::Person { allow_human } => FieldKind::Person {
            allow_human: *allow_human,
        },
        KindDraft::Link { target, many } => {
            old_domain::field_available(
                c,
                a,
                &v2::FieldDraft {
                    name: draft.name.clone(),
                    kind: v2::FieldKind::Link {
                        target: target.clone(),
                        many: *many,
                    },
                    nullable: draft.nullable,
                },
                x,
            )?;
            FieldKind::Link {
                target: target.clone(),
                many: *many,
            }
        }
    };
    Ok(Field {
        id,
        name: draft.name.clone(),
        kind,
        nullable: draft.nullable,
        hidden: false,
    })
}
fn field_mut<'a>(t: &'a mut Table, id: &str) -> Result<&'a mut Field> {
    t.fields
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| invalid("fieldId"))
}
fn option_mut<'a>(
    t: &'a mut Table,
    field_id: &str,
    reference: &OptionRef,
) -> Result<&'a mut OptionDefinition> {
    if reference.table_id != t.id || reference.field_id != field_id {
        return Err(invalid("option"));
    }
    match &mut field_mut(t, field_id)?.kind {
        FieldKind::SingleSelect { options } => options
            .iter_mut()
            .find(|o| o.id == reference.option_id)
            .ok_or_else(|| invalid("option")),
        _ => Err(invalid("optionField")),
    }
}
struct Altered {
    table: Table,
    fields: Vec<String>,
    issued: Vec<IssuedOption>,
    numeric: Option<(String, String)>,
}
fn alter(
    c: &Connection,
    a: Actor<'_>,
    mut t: Table,
    change: &SchemaChange,
    x: &dyn ExternalTargets,
) -> Result<Altered> {
    let mut fields = Vec::new();
    let mut issued = Vec::new();
    let mut numeric = None;
    match change {
        SchemaChange::AddField { field } => {
            if t.fields.len() >= MAX_FIELDS || t.fields.iter().any(|f| f.name == field.name) {
                return Err(invalid("field"));
            }
            if !field.nullable
                && !old_db::numbers(c, Some(num(&t.id, 'T')?), 0, 1, false)?.is_empty()
            {
                return Err(invalid("requiredNewField"));
            }
            let p = old_db::revision(&t.next_field_number)?;
            let mut next = old_db::revision(&t.next_option_number)?;
            let f = field_from_draft(c, a, field, format!("p{p}"), &mut next, x)?;
            if let FieldKind::SingleSelect { options } = &f.kind {
                issued.extend(options.iter().map(|o| IssuedOption {
                    field_id: f.id.clone(),
                    option_id: o.id.clone(),
                }))
            }
            fields.push(f.id.clone());
            t.fields.push(f);
            t.next_field_number = p
                .checked_add(1)
                .ok_or_else(|| invalid("fieldId"))?
                .to_string();
            t.next_option_number = next.to_string();
        }
        SchemaChange::RenameTable {
            title,
            definition_check_token,
            confirm_new,
        } => {
            if !old_db::text(title, TITLE_SCALARS) {
                return Err(invalid("title"));
            }
            consume_check(
                c,
                a,
                CheckIntent {
                    crew: &t.crew_id,
                    title,
                    purpose: t.purpose.as_deref(),
                    excluding: Some(&t.id),
                    token: definition_check_token,
                    confirm: *confirm_new,
                },
            )?;
            t.title = title.clone();
        }
        SchemaChange::SetPurpose {
            purpose,
            definition_check_token,
            confirm_new,
        } => {
            if !old_db::text(purpose, PURPOSE_SCALARS) {
                return Err(invalid("purpose"));
            }
            consume_check(
                c,
                a,
                CheckIntent {
                    crew: &t.crew_id,
                    title: &t.title,
                    purpose: Some(purpose),
                    excluding: Some(&t.id),
                    token: definition_check_token,
                    confirm: *confirm_new,
                },
            )?;
            t.purpose = Some(purpose.clone());
        }
        SchemaChange::RenameField { field_id, name } => {
            if !old_db::text(name, FIELD_NAME_SCALARS)
                || t.fields
                    .iter()
                    .any(|f| f.id != *field_id && f.name == *name)
            {
                return Err(invalid("fieldName"));
            }
            field_mut(&mut t, field_id)?.name = name.clone();
            fields.push(field_id.clone());
        }
        SchemaChange::HideField { field_id } | SchemaChange::RestoreField { field_id } => {
            let hide = matches!(change, SchemaChange::HideField { .. });
            if hide && t.title_field_id == *field_id {
                return Err(invalid("titleFieldId"));
            }
            let f = field_mut(&mut t, field_id)?;
            if hide && !f.nullable {
                return Err(invalid("requiredHiddenField"));
            }
            f.hidden = hide;
            fields.push(field_id.clone());
        }
        SchemaChange::SetTitleField { field_id } => {
            if !t
                .fields
                .iter()
                .any(|f| f.id == *field_id && !f.hidden && !values::graph_field(f))
            {
                return Err(invalid("titleFieldId"));
            }
            t.title_field_id = field_id.clone();
            fields.push(field_id.clone());
        }
        SchemaChange::WidenSingleLink { field_id } => {
            match &mut field_mut(&mut t, field_id)?.kind {
                FieldKind::Link { many, .. } => *many = true,
                _ => return Err(invalid("linkField")),
            }
            fields.push(field_id.clone());
        }
        SchemaChange::WidenText { field_id } => {
            let f = field_mut(&mut t, field_id)?;
            match &f.kind {
                FieldKind::Text { bounds } => {
                    let mut widened = bounds.clone();
                    if let Some(n) = &mut widened.max_unicode_scalars {
                        *n = (*n).max(LONG_TEXT_SCALARS)
                    }
                    f.kind = FieldKind::LongText { bounds: widened };
                }
                FieldKind::LongText { .. } => {}
                _ => return Err(invalid("textField")),
            }
            fields.push(field_id.clone());
        }
        SchemaChange::WidenNumberToText { field_id } => {
            let f = field_mut(&mut t, field_id)?;
            let kind = match f.kind {
                FieldKind::Integer {} => "integer",
                FieldKind::Real {} => "real",
                FieldKind::Decimal { .. } => "decimal",
                _ => return Err(invalid("numericField")),
            };
            numeric = Some((field_id.clone(), kind.into()));
            f.kind = FieldKind::Text {
                bounds: values::legacy_bounds(),
            };
            fields.push(field_id.clone());
        }
        SchemaChange::AddOption { field_id, option } => {
            let mut next = old_db::revision(&t.next_option_number)?;
            let f = field_mut(&mut t, field_id)?;
            let FieldKind::SingleSelect { options } = &mut f.kind else {
                return Err(invalid("optionField"));
            };
            if options.len() >= OPTION_HISTORY_PER_FIELD_MAX
                || options.iter().filter(|o| o.active).count() >= OPTION_ACTIVE_MAX
                || !old_db::text(&option.label, 120)
                || options
                    .iter()
                    .any(|o| o.label.eq_ignore_ascii_case(&option.label))
            {
                return Err(invalid("options"));
            }
            let id = issue(&mut next)?;
            options.push(OptionDefinition {
                id: id.clone(),
                label: option.label.clone(),
                tone: option.tone,
                active: true,
            });
            t.next_option_number = next.to_string();
            fields.push(field_id.clone());
            issued.push(IssuedOption {
                field_id: field_id.clone(),
                option_id: id,
            });
        }
        SchemaChange::RenameOption {
            field_id,
            option,
            label,
        } => {
            if !old_db::text(label, 120) {
                return Err(invalid("optionLabel"));
            }
            if let FieldKind::SingleSelect { options } = &field_mut(&mut t, field_id)?.kind
                && options
                    .iter()
                    .any(|o| o.id != option.option_id && o.label.eq_ignore_ascii_case(label))
            {
                return Err(invalid("optionLabel"));
            }
            option_mut(&mut t, field_id, option)?.label = label.clone();
            fields.push(field_id.clone());
        }
        SchemaChange::DeactivateOption { field_id, option }
        | SchemaChange::RestoreOption { field_id, option } => {
            let restore = matches!(change, SchemaChange::RestoreOption { .. });
            if restore {
                let f = field_mut(&mut t, field_id)?;
                if let FieldKind::SingleSelect { options } = &f.kind
                    && options.iter().filter(|o| o.active).count() >= OPTION_ACTIVE_MAX
                    && options
                        .iter()
                        .any(|o| o.id == option.option_id && !o.active)
                {
                    return Err(invalid("options"));
                }
            }
            option_mut(&mut t, field_id, option)?.active = restore;
            fields.push(field_id.clone());
        }
        SchemaChange::SetOptionTone {
            field_id,
            option,
            tone,
        } => {
            option_mut(&mut t, field_id, option)?.tone = *tone;
            fields.push(field_id.clone());
        }
    }
    if serde_json::to_vec(&t.fields)?.len() > DEFINITION_ENCODED_BYTES {
        return Err(invalid("definitionBytes"));
    }
    Ok(Altered {
        table: t,
        fields,
        issued,
        numeric,
    })
}
struct Applied {
    stored: Vec<ScalarValue>,
    changed: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
    deps: Vec<i64>,
}
struct Apply<'a> {
    actor: Actor<'a>,
    table: &'a Table,
    row_id: &'a str,
    revision: &'a str,
    stored: Vec<ScalarValue>,
    logical: Vec<ScalarValue>,
    patches: &'a [Patch],
    new: bool,
    external: &'a dyn ExternalTargets,
    now: &'a str,
}
fn apply(c: &Connection, r: Apply<'_>) -> Result<Applied> {
    if r.patches.len() > MAX_FIELDS
        || r.patches
            .iter()
            .map(|p| &p.field_id)
            .collect::<HashSet<_>>()
            .len()
            != r.patches.len()
    {
        return Err(invalid("patches"));
    }
    let mut stored = r.stored;
    let mut changed = Vec::new();
    let mut graph_patches = Vec::new();
    for patch in r.patches {
        let f = r
            .table
            .fields
            .iter()
            .find(|f| f.id == patch.field_id)
            .ok_or_else(|| invalid("fieldId"))?;
        if f.hidden && !matches!(patch.change, CellPatch::Keep {}) {
            return Err(invalid("hiddenField"));
        }
        if values::graph_field(f) {
            let change = match &patch.change {
                CellPatch::Keep {} => v2::CellPatch::Keep {},
                CellPatch::SetScalar { .. } => return Err(invalid("linkPatch")),
                CellPatch::ReplaceLinks {
                    values,
                    keep_hidden_set_token,
                } => {
                    if let FieldKind::Person { allow_human } = &f.kind {
                        for v in values {
                            match v {
                                v2::LinkWrite::Reference {
                                    target: v2::TargetRef::Session { .. },
                                } => {}
                                v2::LinkWrite::Reference {
                                    target: v2::TargetRef::Human {},
                                } if *allow_human => {}
                                _ => return Err(invalid("personTarget")),
                            }
                        }
                    }
                    v2::CellPatch::ReplaceLinks {
                        values: values.clone(),
                        keep_hidden_set_token: keep_hidden_set_token.clone(),
                    }
                }
            };
            graph_patches.push(v2::Patch {
                field_id: f.id.clone(),
                change,
            });
            continue;
        }
        match &patch.change {
            CellPatch::Keep {} => {}
            CellPatch::ReplaceLinks { .. } => return Err(invalid("scalarPatch")),
            CellPatch::SetScalar { value } => {
                if !values::scalar_valid(&r.table.id, f, value, true) {
                    return Err(invalid("value"));
                }
                let old = r
                    .logical
                    .iter()
                    .find(|v| v.field_id == f.id)
                    .map(|v| &v.value)
                    .unwrap_or(&Cell::Null);
                if old != value {
                    if let Some(existing) = stored.iter_mut().find(|v| v.field_id == f.id) {
                        existing.value = value.clone()
                    } else {
                        stored.push(ScalarValue {
                            field_id: f.id.clone(),
                            value: value.clone(),
                        })
                    }
                    changed.push(f.id.clone());
                }
            }
        }
    }
    if r.new {
        for f in r
            .table
            .fields
            .iter()
            .filter(|f| !values::graph_field(f) && !f.nullable)
        {
            if !r
                .patches
                .iter()
                .any(|p| p.field_id == f.id && matches!(p.change, CellPatch::SetScalar { .. }))
            {
                return Err(invalid("requiredField"));
            }
        }
    }
    // Check the expanded typed representation too, preserving the original
    // whole-row 64KiB domain rather than admitting oversized option envelopes.
    if serde_json::to_vec(&stored)?.len() > ROW_SCALARS_ENCODED_BYTES {
        return Err(invalid("rowBytes"));
    }
    db::scalar_storage(&stored)?;
    let gt = graph_table(r.table);
    let links = old_domain::apply(
        c,
        old_domain::ApplyInput {
            actor: r.actor,
            table: &gt,
            row_id: r.row_id,
            revision: r.revision,
            values: Vec::new(),
            patches: &graph_patches,
            new: r.new,
            external: r.external,
            now: r.now,
        },
    )?;
    changed.extend(links.changed);
    Ok(Applied {
        stored,
        changed,
        added: links.added,
        removed: links.removed,
        deps: links.deps,
    })
}
struct Audit {
    source: i64,
    action: HistoryAction,
    reason: Option<String>,
    fields: Vec<String>,
    original: Option<String>,
    result: Option<String>,
    before_schema: Option<Table>,
    after_schema: Option<Table>,
    before_values: Vec<ScalarValue>,
    after_values: Vec<ScalarValue>,
    before_links: Vec<graph::Edge>,
    after_links: Vec<graph::Edge>,
    legacy: bool,
    before_schema_raw: Option<String>,
}
impl Audit {
    fn new(source: i64, action: HistoryAction) -> Self {
        Self {
            source,
            action,
            reason: None,
            fields: Vec::new(),
            original: None,
            result: None,
            before_schema: None,
            after_schema: None,
            before_values: Vec::new(),
            after_values: Vec::new(),
            before_links: Vec::new(),
            after_links: Vec::new(),
            legacy: false,
            before_schema_raw: None,
        }
    }
}
fn graph_edges(c: &Connection, id: &str) -> Result<Vec<graph::Edge>> {
    Ok(graph::outgoing(c, &v2::SourceRef::Row { id: id.into() })?)
}
fn save_audit(c: &Connection, a: Actor<'_>, event: &str, at: &str, audit: Audit) -> Result<()> {
    db::record_audit(
        c,
        db::AuditInput {
            event,
            source: audit.source,
            action: audit.action.clone(),
            actor: &actor_view(a),
            at,
            reason: audit.reason.as_deref(),
            fields: &audit.fields,
            original: audit.original.as_deref(),
            result: audit.result.as_deref(),
            legacy: audit.legacy,
        },
    )?;
    for (side, table) in [
        (HistorySide::Before, audit.before_schema.as_ref()),
        (HistorySide::After, audit.after_schema.as_ref()),
    ] {
        if let Some(_table) = table {
            let raw = if side == HistorySide::Before {
                audit
                    .before_schema_raw
                    .clone()
                    .ok_or_else(|| invalid("historyEvidence"))?
            } else {
                db::schema_snapshot(c, audit.source)?
            };
            db::record_audit_value(
                c,
                event,
                &HistoryPath::Schema { side },
                HistoryValueType::Schema,
                &raw,
            )?
        }
    }
    let row_table = db::row_base(c, audit.source)?.map(|r| r.table);
    for field in &audit.fields {
        let is_graph = row_table
            .as_ref()
            .and_then(|t| t.fields.iter().find(|f| f.id == *field))
            .is_some_and(values::graph_field);
        let is_scalar = row_table
            .as_ref()
            .and_then(|t| t.fields.iter().find(|f| f.id == *field))
            .is_some_and(|f| !values::graph_field(f));
        for (side, cells, edges) in [
            (
                HistorySide::Before,
                &audit.before_values,
                &audit.before_links,
            ),
            (HistorySide::After, &audit.after_values, &audit.after_links),
        ] {
            if is_scalar && (side == HistorySide::After || audit.original.is_some()) {
                let value = cells
                    .iter()
                    .find(|v| v.field_id == *field)
                    .map(|v| &v.value)
                    .unwrap_or(&Cell::Null);
                let (kind, text) = values::scalar_text(value)?;
                db::record_audit_value(
                    c,
                    event,
                    &HistoryPath::Scalar {
                        field_id: field.clone(),
                        side,
                    },
                    kind,
                    &text,
                )?;
            }
            let origin = format!("column:{field}");
            let links = edges
                .iter()
                .filter(|e| e.origin == origin)
                .map(|e| serde_json::json!({"edgeId":e.id.to_string(),"target":e.target}))
                .collect::<Vec<_>>();
            if is_graph && (side == HistorySide::After || audit.original.is_some()) {
                // Target identities stay private. Every history read separately
                // re-runs current target ACL and emits one aggregate restriction.
                db::record_audit_value(
                    c,
                    event,
                    &HistoryPath::Links {
                        field_id: field.clone(),
                        side,
                    },
                    HistoryValueType::Schema,
                    &serde_json::to_string(&links)?,
                )?;
            }
        }
    }
    Ok(())
}
pub(crate) fn old_request_exists(
    c: &Connection,
    crew: &str,
    principal: &str,
    uuid: &str,
) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM managed_object_receipts WHERE crew_id=?1 AND principal=?2 AND request_id=?3 AND json_extract(result_json,'$.object.kind') IN('table','row'))",params![crew,principal,uuid],|r|r.get(0))?)
}
pub(crate) fn graph_table_request_exists(
    c: &Connection,
    crew: &str,
    principal: &str,
    uuid: &str,
) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM managed_v2_receipts WHERE owner_crew=?1 AND principal=?2 AND request_id=?3 AND method IN(?4,?5))",params![crew,principal,uuid,v2::TABLE_SUBMIT,SUBMIT],|r|r.get(0))?)
}
pub(crate) struct LegacyBefore {
    table: Option<Table>,
    row: Option<db::RowBase>,
    links: Vec<graph::Edge>,
    schema_raw: Option<String>,
}
fn legacy_before(c: &Connection, table: Option<&str>, row: Option<&str>) -> Result<LegacyBefore> {
    let schema_raw = table
        .map(|id| Ok::<_, CollabError>(db::schema_snapshot(c, num(id, 'T')?)?))
        .transpose()?;
    let table = table
        .map(|id| db::table(c, num(id, 'T')?)?.ok_or_else(|| missing(id)))
        .transpose()?;
    let value = row
        .map(|id| db::row_base(c, num(id, 'O')?)?.ok_or_else(|| missing(id)))
        .transpose()?;
    let links = row
        .map(|id| graph_edges(c, id))
        .transpose()?
        .unwrap_or_default();
    Ok(LegacyBefore {
        table,
        row: value,
        links,
        schema_raw,
    })
}
pub(crate) fn legacy_v2_before(c: &Connection, operation: &v2::Operation) -> Result<LegacyBefore> {
    match operation {
        v2::Operation::AlterTable { table_id, .. } => legacy_before(c, Some(table_id), None),
        v2::Operation::PatchRow { row_id, .. } => legacy_before(c, None, Some(row_id)),
        _ => legacy_before(c, None, None),
    }
}
pub(crate) fn legacy_v1_before(
    c: &Connection,
    change: &crewd_proto::managed_objects::Change,
) -> Result<LegacyBefore> {
    match change {
        crewd_proto::managed_objects::Change::EditRow { row_id, .. } => {
            legacy_before(c, None, Some(row_id))
        }
        _ => legacy_before(c, None, None),
    }
}
pub(crate) fn audit_v2(
    c: &Connection,
    a: Actor<'_>,
    input: &v2::Submit,
    receipt: &v2::Receipt,
    before: LegacyBefore,
) -> Result<()> {
    let Some(event) = receipt.event_id.as_deref() else {
        return Ok(());
    };
    let mut record = match &receipt.effect {
        v2::Effect::TableCreated {
            table_id,
            field_ids,
            schema_revision,
            ..
        } => {
            let n = num(table_id, 'T')?;
            let mut r = Audit::new(n, HistoryAction::CreateTable);
            r.fields = field_ids.clone();
            r.result = Some(schema_revision.clone());
            r.after_schema = db::table(c, n)?;
            r
        }
        v2::Effect::SchemaChanged {
            table_id,
            changed_field_ids,
            original_schema_revision,
            result_schema_revision,
            ..
        } => {
            let n = num(table_id, 'T')?;
            let mut r = Audit::new(n, HistoryAction::AlterTable);
            r.fields = changed_field_ids.clone();
            r.original = Some(original_schema_revision.clone());
            r.result = Some(result_schema_revision.clone());
            r.before_schema = before.table;
            r.before_schema_raw = before.schema_raw;
            r.after_schema = db::table(c, n)?;
            r
        }
        v2::Effect::RowAdded {
            row_id,
            row_revision,
            ..
        } => {
            let n = num(row_id, 'O')?;
            let row = db::row_base(c, n)?.ok_or_else(|| missing(row_id))?;
            let mut r = Audit::new(n, HistoryAction::AddRow);
            r.result = Some(row_revision.clone());
            r.fields = match &input.operation {
                v2::Operation::AddRow { patches, .. } => {
                    patches.iter().map(|p| p.field_id.clone()).collect()
                }
                _ => return Err(invalid("historyEvidence")),
            };
            r.after_values = row.logical;
            r.after_links = graph_edges(c, row_id)?;
            r
        }
        v2::Effect::RowPatched {
            row_id,
            changed_field_ids,
            original_row_revision,
            result_row_revision,
            ..
        } => {
            let n = num(row_id, 'O')?;
            let row = db::row_base(c, n)?.ok_or_else(|| missing(row_id))?;
            let old = before.row.ok_or_else(|| invalid("historyEvidence"))?;
            let mut r = Audit::new(n, HistoryAction::PatchRow);
            r.fields = changed_field_ids.clone();
            r.original = Some(original_row_revision.clone());
            r.result = Some(result_row_revision.clone());
            r.before_values = old.logical;
            r.after_values = row.logical;
            r.before_links = before.links;
            r.after_links = graph_edges(c, row_id)?;
            r
        }
    };
    record.legacy = true;
    save_audit(c, a, event, &receipt.recorded_at, record)
}
pub(crate) fn audit_v1(
    c: &Connection,
    a: Actor<'_>,
    receipt: &crewd_proto::managed_objects::Receipt,
    before: LegacyBefore,
) -> Result<()> {
    let mut record = match &receipt.object {
        crewd_proto::managed_objects::Object::Table { table } => {
            let n = num(&table.id, 'T')?;
            let current = db::table(c, n)?.ok_or_else(|| missing(&table.id))?;
            let mut r = Audit::new(n, HistoryAction::CreateTable);
            r.fields = current.fields.iter().map(|f| f.id.clone()).collect();
            r.result = Some(current.schema_revision.clone());
            r.after_schema = Some(current);
            r
        }
        crewd_proto::managed_objects::Object::Row { row } => {
            let n = num(&row.id, 'O')?;
            let current = db::row_base(c, n)?.ok_or_else(|| missing(&row.id))?;
            let mut r = Audit::new(
                n,
                if before.row.is_some() {
                    HistoryAction::LegacyRowWrite
                } else {
                    HistoryAction::AddRow
                },
            );
            r.result = Some(current.revision.clone());
            r.fields = current
                .table
                .fields
                .iter()
                .filter(|f| !values::graph_field(f))
                .map(|f| f.id.clone())
                .collect();
            r.after_values = current.logical;
            if let Some(old) = before.row {
                r.original = Some(old.revision);
                r.before_values = old.logical;
            }
            r
        }
        _ => return Ok(()),
    };
    record.legacy = true;
    let event = uuid::Uuid::new_v4().to_string();
    save_audit(c, a, &event, &receipt.recorded_at, record)?;
    c.execute(
        "INSERT INTO managed_table_audit_legacy VALUES(?1,?2,?3,'managedObjectsV1',?4)",
        params![receipt.crew_id, a.principal(c)?, receipt.request_id, event],
    )?;
    Ok(())
}
fn reason_valid(s: &str) -> Result<()> {
    if old_db::text(s, REASON_SCALARS) {
        Ok(())
    } else {
        Err(invalid("reason"))
    }
}
pub fn submit(
    tx: &mut Tx<'_>,
    a: Actor<'_>,
    r: Submit,
    x: &dyn ExternalTargets,
) -> Result<Receipt> {
    let c = tx.conn();
    let principal = links::request(c, a, &r.scope, &r.crew_id, &r.request_id, false)?;
    let digest = graph::intent(SUBMIT, &r.scope, &r.crew_id, &r.request_id, &r.operation)?;
    if let Some(saved) = graph::receipt(c, &r.crew_id, &principal, &r.request_id)? {
        return match saved {
            graph::ReadReceipt::Active(s) if s.method == SUBMIT && s.digest == digest => {
                Ok(serde_json::from_value(s.result)?)
            }
            _ => Err(conflict(&r.request_id)),
        };
    }
    if old_request_exists(c, &r.crew_id, &principal, &r.request_id)? {
        return Err(conflict(&r.request_id));
    }
    if objects::scope(c)? != r.scope {
        return Err(schema_changed());
    }
    old_domain::crew_writer(c, a, &r.crew_id)?;
    let before_schema_raw = match &r.operation {
        Operation::AlterTable { table_id, .. }
        | Operation::ArchiveTable { table_id, .. }
        | Operation::RestoreTable { table_id, .. } => {
            Some(db::schema_snapshot(c, num(table_id, 'T')?)?)
        }
        _ => None,
    };
    let now = tx.now_ts();
    let mut deps = Vec::new();
    let mut audit = None;
    let mut codec = None;
    let (kind, mutated, effect) = match &r.operation {
        Operation::CreateTable {
            title,
            purpose,
            fields,
            title_field_index,
            row_write_policy,
            definition_check_token,
            confirm_new,
        } => {
            a.owner(c, &r.crew_id, true)?;
            if !old_db::text(title, TITLE_SCALARS)
                || !old_db::text(purpose, PURPOSE_SCALARS)
                || fields.is_empty()
                || fields.len() > MAX_FIELDS
                || fields.iter().map(|f| &f.name).collect::<HashSet<_>>().len() != fields.len()
            {
                return Err(invalid("definition"));
            }
            consume_check(
                c,
                a,
                CheckIntent {
                    crew: &r.crew_id,
                    title,
                    purpose: Some(purpose),
                    excluding: None,
                    token: definition_check_token,
                    confirm: *confirm_new,
                },
            )?;
            let mut next = 1;
            let fs = fields
                .iter()
                .enumerate()
                .map(|(i, f)| field_from_draft(c, a, f, format!("p{}", i + 1), &mut next, x))
                .collect::<Result<Vec<_>>>()?;
            let index = *title_field_index as usize;
            if index >= fs.len() || values::graph_field(&fs[index]) {
                return Err(invalid("titleFieldId"));
            }
            if serde_json::to_vec(&fs)?.len() > DEFINITION_ENCODED_BYTES {
                return Err(invalid("definitionBytes"));
            }
            let policy = match row_write_policy {
                v2::NewRowWritePolicy::AuthorCaptainHuman => v2::RowWritePolicy::AuthorCaptainHuman,
                v2::NewRowWritePolicy::OwnerCrewWriters => v2::RowWritePolicy::OwnerCrewWriters,
            };
            let table = db::create(
                c,
                db::NewTable {
                    crew: &r.crew_id,
                    title,
                    purpose,
                    fields: &fs,
                    title_field: &fs[index].id,
                    next_option: next,
                    policy,
                    author: &links::author(a),
                    now: &now,
                },
            )?;
            let n = num(&table.id, 'T')?;
            deps.push(n);
            let mut record = Audit::new(n, HistoryAction::CreateTable);
            record.fields = fs.iter().map(|f| f.id.clone()).collect();
            record.result = Some("1".into());
            record.after_schema = Some(table.clone());
            audit = Some(record);
            let issued = fs
                .iter()
                .flat_map(|f| match &f.kind {
                    FieldKind::SingleSelect { options } => options
                        .iter()
                        .map(|o| IssuedOption {
                            field_id: f.id.clone(),
                            option_id: o.id.clone(),
                        })
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                })
                .collect();
            (
                OperationKind::CreateTable,
                true,
                Effect::TableCreated {
                    table_id: table.id,
                    schema_revision: table.schema_revision,
                    field_ids: fs.iter().map(|f| f.id.clone()).collect(),
                    title_field_id: fs[index].id.clone(),
                    issued_options: issued,
                },
            )
        }
        Operation::AlterTable {
            table_id,
            expected_schema_revision,
            reason,
            change,
        } => {
            reason_valid(reason)?;
            let before = get(c, a, table_id, x)?;
            if before.crew_id != r.crew_id {
                return Err(forbidden());
            }
            schema_authority(c, a, &before)?;
            table_writable(&before)?;
            if before.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            let mut altered = alter(c, a, before.clone(), change, x)?;
            let changed = altered.table != before;
            if changed {
                altered.table.schema_revision = old_db::revision(&before.schema_revision)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("schemaRevision"))?
                    .to_string();
                db::save_table(
                    c,
                    &altered.table,
                    old_db::revision(&before.schema_revision)?,
                )?;
                codec = altered
                    .numeric
                    .map(|(f, k)| (num(table_id, 'T').unwrap_or(0), f, k));
                let mut record = Audit::new(num(table_id, 'T')?, HistoryAction::AlterTable);
                record.reason = Some(reason.clone());
                record.fields = altered.fields.clone();
                record.original = Some(before.schema_revision.clone());
                record.result = Some(altered.table.schema_revision.clone());
                record.before_schema = Some(before.clone());
                record.after_schema = Some(altered.table.clone());
                audit = Some(record);
            }
            deps.push(num(table_id, 'T')?);
            (
                OperationKind::AlterTable,
                changed,
                Effect::SchemaChanged {
                    table_id: table_id.clone(),
                    original_schema_revision: before.schema_revision,
                    result_schema_revision: altered.table.schema_revision,
                    changed_field_ids: if changed { altered.fields } else { Vec::new() },
                    issued_options: if changed { altered.issued } else { Vec::new() },
                    changed,
                },
            )
        }
        Operation::AddRow {
            table_id,
            expected_schema_revision,
            patches,
        } => {
            let table = get(c, a, table_id, x)?;
            if table.crew_id != r.crew_id {
                return Err(forbidden());
            }
            old_domain::crew_writer(c, a, &table.crew_id)?;
            table_writable(&table)?;
            if table.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            let n = db::add(c, &table, &links::author(a), &[], &now)?;
            let id = format!("O{n}");
            let applied = apply(
                c,
                Apply {
                    actor: a,
                    table: &table,
                    row_id: &id,
                    revision: "1",
                    stored: Vec::new(),
                    logical: Vec::new(),
                    patches,
                    new: true,
                    external: x,
                    now: &now,
                },
            )?;
            c.execute(
                "UPDATE managed_rows SET values_json=?2 WHERE number=?1",
                params![n, db::scalar_storage(&applied.stored)?],
            )?;
            let mut record = Audit::new(n, HistoryAction::AddRow);
            record.result = Some("1".into());
            record.fields = patches.iter().map(|p| p.field_id.clone()).collect();
            record.after_values = db::logical_scalars(c, &table, n)?;
            record.after_links = graph_edges(c, &id)?;
            audit = Some(record);
            deps.extend(applied.deps);
            deps.extend([n, num(table_id, 'T')?]);
            (
                OperationKind::AddRow,
                true,
                Effect::RowAdded {
                    table_id: table_id.clone(),
                    row_id: id,
                    schema_revision: table.schema_revision,
                    row_revision: "1".into(),
                    edge_ids: applied.added,
                },
            )
        }
        Operation::PatchRow {
            row_id,
            expected_schema_revision,
            expected_row_revision,
            reason,
            patches,
        } => {
            reason_valid(reason)?;
            let n = num(row_id, 'O')?;
            let before = db::row_base(c, n)?.ok_or_else(|| missing(row_id))?;
            if before.table.crew_id != r.crew_id {
                return Err(forbidden());
            }
            row_authority(c, a, &before.table, &before.author)?;
            table_writable(&before.table)?;
            if before.lifecycle.archived_at.is_some() {
                return Err(forbidden());
            }
            if before.table.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            if before.revision != *expected_row_revision {
                return Err(row_changed());
            }
            let old_links = graph_edges(c, row_id)?;
            let applied = apply(
                c,
                Apply {
                    actor: a,
                    table: &before.table,
                    row_id,
                    revision: &before.revision,
                    stored: before.stored,
                    logical: before.logical.clone(),
                    patches,
                    new: false,
                    external: x,
                    now: &now,
                },
            )?;
            let changed = !applied.changed.is_empty();
            let mut result = before.revision.clone();
            if changed {
                db::save_row(c, n, old_db::revision(&before.revision)?, &applied.stored)?;
                result = old_db::revision(&before.revision)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("rowRevision"))?
                    .to_string();
                let mut record = Audit::new(n, HistoryAction::PatchRow);
                record.reason = Some(reason.clone());
                record.fields = applied.changed.clone();
                record.original = Some(before.revision.clone());
                record.result = Some(result.clone());
                record.before_values = before.logical;
                record.after_values = db::logical_scalars(c, &before.table, n)?;
                record.before_links = old_links;
                record.after_links = graph_edges(c, row_id)?;
                audit = Some(record);
            }
            deps.extend(applied.deps);
            deps.extend([n, num(&before.table.id, 'T')?]);
            (
                OperationKind::PatchRow,
                changed,
                Effect::RowPatched {
                    table_id: before.table.id,
                    row_id: row_id.clone(),
                    schema_revision: before.table.schema_revision,
                    original_row_revision: before.revision,
                    result_row_revision: result,
                    changed_field_ids: applied.changed,
                    added_edge_ids: applied.added,
                    removed_edge_ids: applied.removed,
                    changed,
                },
            )
        }
        Operation::ArchiveTable {
            table_id,
            expected_schema_revision,
            reason,
        }
        | Operation::RestoreTable {
            table_id,
            expected_schema_revision,
            reason,
        } => {
            reason_valid(reason)?;
            let before = get(c, a, table_id, x)?;
            if before.crew_id != r.crew_id {
                return Err(forbidden());
            }
            schema_authority(c, a, &before)?;
            if before.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            let archived = matches!(r.operation, Operation::ArchiveTable { .. });
            let changed = before.lifecycle.archived_at.is_some() != archived;
            let mut result = before.schema_revision.clone();
            if changed {
                result = old_db::revision(&result)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("schemaRevision"))?
                    .to_string();
                let mut after = before.clone();
                after.schema_revision = result.clone();
                after.lifecycle = if archived {
                    Lifecycle {
                        archived_at: Some(now.clone()),
                        archived_by: Some(actor_view(a)),
                        reason: Some(reason.clone()),
                    }
                } else {
                    Lifecycle {
                        archived_at: None,
                        archived_by: None,
                        reason: None,
                    }
                };
                db::save_table(c, &after, old_db::revision(&before.schema_revision)?)?;
                db::set_lifecycle(c, num(table_id, 'T')?, false, &after.lifecycle)?;
                let mut record = Audit::new(
                    num(table_id, 'T')?,
                    if archived {
                        HistoryAction::ArchiveTable
                    } else {
                        HistoryAction::RestoreTable
                    },
                );
                record.reason = Some(reason.clone());
                record.original = Some(before.schema_revision.clone());
                record.result = Some(result.clone());
                record.before_schema = Some(before.clone());
                record.after_schema = db::table(c, num(table_id, 'T')?)?;
                audit = Some(record);
            }
            deps.push(num(table_id, 'T')?);
            (
                if archived {
                    OperationKind::ArchiveTable
                } else {
                    OperationKind::RestoreTable
                },
                changed,
                Effect::TableLifecycle {
                    table_id: table_id.clone(),
                    original_schema_revision: before.schema_revision,
                    result_schema_revision: result,
                    archived,
                    changed,
                },
            )
        }
        Operation::ArchiveRow {
            row_id,
            expected_schema_revision,
            expected_row_revision,
            reason,
        }
        | Operation::RestoreRow {
            row_id,
            expected_schema_revision,
            expected_row_revision,
            reason,
        } => {
            reason_valid(reason)?;
            let n = num(row_id, 'O')?;
            let before = db::row_base(c, n)?.ok_or_else(|| missing(row_id))?;
            if before.table.crew_id != r.crew_id {
                return Err(forbidden());
            }
            row_authority(c, a, &before.table, &before.author)?;
            table_writable(&before.table)?;
            if before.table.schema_revision != *expected_schema_revision {
                return Err(schema_changed());
            }
            if before.revision != *expected_row_revision {
                return Err(row_changed());
            }
            let archived = matches!(r.operation, Operation::ArchiveRow { .. });
            let changed = before.lifecycle.archived_at.is_some() != archived;
            let mut result = before.revision.clone();
            if changed {
                db::save_row(c, n, old_db::revision(&before.revision)?, &before.stored)?;
                result = old_db::revision(&before.revision)?
                    .checked_add(1)
                    .ok_or_else(|| invalid("rowRevision"))?
                    .to_string();
                db::set_lifecycle(
                    c,
                    n,
                    true,
                    &if archived {
                        Lifecycle {
                            archived_at: Some(now.clone()),
                            archived_by: Some(actor_view(a)),
                            reason: Some(reason.clone()),
                        }
                    } else {
                        Lifecycle {
                            archived_at: None,
                            archived_by: None,
                            reason: None,
                        }
                    },
                )?;
                let mut record = Audit::new(
                    n,
                    if archived {
                        HistoryAction::ArchiveRow
                    } else {
                        HistoryAction::RestoreRow
                    },
                );
                record.reason = Some(reason.clone());
                record.original = Some(before.revision.clone());
                record.result = Some(result.clone());
                audit = Some(record);
            }
            deps.extend([n, num(&before.table.id, 'T')?]);
            (
                if archived {
                    OperationKind::ArchiveRow
                } else {
                    OperationKind::RestoreRow
                },
                changed,
                Effect::RowLifecycle {
                    table_id: before.table.id,
                    row_id: row_id.clone(),
                    schema_revision: before.table.schema_revision,
                    original_row_revision: before.revision,
                    result_row_revision: result,
                    archived,
                    changed,
                },
            )
        }
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
    db::validate_receipt(&r, &receipt)?;
    graph::record(
        c,
        graph::ReceiptInput {
            crew: &r.crew_id,
            principal: &principal,
            uuid: &r.request_id,
            method: SUBMIT,
            digest: &digest,
            input: &r,
            result: &receipt,
            objects: &deps,
        },
    )?;
    if let Some((n, field, kind)) = codec {
        db::record_numeric_codec(
            c,
            n,
            &field,
            &kind,
            receipt
                .event_id
                .as_deref()
                .ok_or_else(|| invalid("eventId"))?,
        )?
    }
    if let Some(mut record) = audit {
        if record.before_schema.is_some() {
            record.before_schema_raw = before_schema_raw;
        }
        if record.before_schema.is_some() && record.after_schema.is_some() {
            c.execute(
                "INSERT INTO managed_table_schema_events VALUES(?1,?2,?3,?4)",
                params![
                    receipt.event_id,
                    record.source,
                    record
                        .before_schema_raw
                        .as_ref()
                        .ok_or_else(|| invalid("historyEvidence"))?,
                    db::schema_snapshot(c, record.source)?
                ],
            )?;
        }
        save_audit(
            c,
            a,
            receipt
                .event_id
                .as_deref()
                .ok_or_else(|| invalid("eventId"))?,
            &receipt.recorded_at,
            record,
        )?;
    }
    if mutated {
        links::finish(tx, a, &r.crew_id)?;
        crewd_store::managed_artifacts::gc(tx.conn())?
    }
    Ok(receipt)
}
pub fn lookup(
    c: &Connection,
    a: Actor<'_>,
    scope: &Scope,
    crew: &str,
    uuid: &str,
) -> Result<Lookup> {
    let p = links::request(c, a, scope, crew, uuid, false)?;
    if graph::receipt_method(c, crew, &p, uuid)?.is_some_and(|method| method != SUBMIT) {
        return Err(conflict(uuid));
    }
    let mut retired = false;
    let receipt = match graph::receipt(c, crew, &p, uuid)? {
        Some(graph::ReadReceipt::Active(s)) if s.method == SUBMIT => {
            Some(serde_json::from_value::<Receipt>(s.result)?)
        }
        Some(graph::ReadReceipt::Active(_)) => return Err(conflict(uuid)),
        Some(graph::ReadReceipt::Retired) => {
            retired = true;
            None
        }
        None => {
            if old_request_exists(c, crew, &p, uuid)? {
                return Err(conflict(uuid));
            }
            None
        }
    }
    .filter(|r| r.scope == *scope);
    Ok(if objects::scope(c)? != *scope || retired {
        Lookup::Historical { receipt }
    } else if let Some(receipt) = receipt {
        Lookup::Known { receipt }
    } else {
        Lookup::NotFound {}
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryCursor {
    scope: Scope,
    principal: String,
    source: HistorySource,
    phase: u8,
    after: String,
}
fn history_number(c: &Connection, a: Actor<'_>, source: &HistorySource) -> Result<i64> {
    let id = match source {
        HistorySource::Table { id } | HistorySource::Row { id } => id,
    };
    read_authority(c, a, id)?;
    num(
        id,
        if matches!(source, HistorySource::Table { .. }) {
            'T'
        } else {
            'O'
        },
    )
}
pub fn history(
    c: &Connection,
    a: Actor<'_>,
    q: History,
    _x: &dyn ExternalTargets,
) -> Result<HistoryPage> {
    let n = history_number(c, a, &q.source)?;
    let scope = objects::scope(c)?;
    let principal = a.principal(c)?;
    let mut cursor = if let Some(raw) = &q.cursor {
        let v: HistoryCursor = serde_json::from_slice(
            &graph::open(c, "managed-table-history-page.v1", raw).map_err(|_| schema_changed())?,
        )?;
        if v.scope != scope || v.principal != principal || v.source != q.source || v.phase > 2 {
            return Err(schema_changed());
        }
        v
    } else {
        HistoryCursor {
            scope: scope.clone(),
            principal,
            source: q.source.clone(),
            phase: 0,
            after: String::new(),
        }
    };
    let mut items = Vec::new();
    let mut bytes = serde_json::to_vec(&q.source)?.len() + 1024;
    let mut next = false;
    // Old codec adapters are selected in bounded phases. Fresh legacy writes
    // are journaled into the same new audit stream; no migration fabricates
    // authors, reasons, before-values or business events for old receipts.
    let mut physical = 0;
    while physical < HISTORY_RECORD_BUDGET && cursor.phase < 3 {
        if cursor.phase == 0 {
            let before = if cursor.after.is_empty() {
                None
            } else {
                Some(old_db::revision(&cursor.after).map_err(|_| schema_changed())?)
            };
            let budget = HISTORY_RECORD_BUDGET - physical;
            let rows = db::audit_page(c, n, before, budget as u16)?;
            let exhausted_budget = rows.len() == budget;
            if rows.is_empty() {
                cursor.phase = 1;
                cursor.after = String::new();
                continue;
            }
            for (ordinal, item) in rows {
                let size = serde_json::to_vec(&item)?.len();
                if physical >= HISTORY_RECORD_BUDGET || bytes + size > HISTORY_ENCODED_BYTES {
                    next = true;
                    break;
                }
                cursor.after = ordinal.to_string();
                physical += 1;
                bytes += size + 1;
                items.push(item);
            }
            if exhausted_budget {
                next = true
            }
            if next {
                break;
            }
            cursor.phase = 1;
            cursor.after = String::new();
        } else {
            let budget = HISTORY_RECORD_BUDGET - physical;
            let rows = legacy_history(
                c,
                a,
                n,
                &q.source,
                cursor.phase,
                &cursor.after,
                budget as u16,
            )?;
            let exhausted_budget = rows.len() == budget;
            if rows.is_empty() {
                cursor.phase += 1;
                cursor.after = String::new();
                continue;
            }
            for (ordinal, item) in rows {
                let size = serde_json::to_vec(&item)?.len();
                if physical >= HISTORY_RECORD_BUDGET || bytes + size > HISTORY_ENCODED_BYTES {
                    next = true;
                    break;
                }
                cursor.after = ordinal;
                physical += 1;
                bytes += size + 1;
                items.push(item);
            }
            if exhausted_budget {
                next = true
            }
            if next {
                break;
            }
            cursor.phase += 1;
            cursor.after = String::new();
        }
    }
    let next_cursor = if next || cursor.phase < 3 {
        Some(graph::seal(
            c,
            "managed-table-history-page.v1",
            &serde_json::to_vec(&cursor)?,
        )?)
    } else {
        None
    };
    Ok(HistoryPage {
        scope,
        source: q.source,
        items,
        next_cursor,
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyLocator {
    scope: Scope,
    viewer: String,
    crew: String,
    principal: String,
    request: String,
    source: HistorySource,
    codec: LegacyCodec,
}
fn legacy_actor(principal: &str) -> ResourceActor {
    if let Some(session) = principal.strip_prefix("session:") {
        ResourceActor {
            kind: "session".into(),
            session_id: Some(session.into()),
        }
    } else {
        ResourceActor {
            kind: "human".into(),
            session_id: None,
        }
    }
}
fn legacy_history(
    c: &Connection,
    a: Actor<'_>,
    _n: i64,
    source: &HistorySource,
    phase: u8,
    after: &str,
    limit: u16,
) -> Result<Vec<(String, HistorySummary)>> {
    let scope = objects::scope(c)?;
    let viewer = a.principal(c)?;
    let id = match source {
        HistorySource::Table { id } | HistorySource::Row { id } => id,
    };
    if phase == 1 {
        let mut query=c.prepare("SELECT e.id,e.owner_crew,e.principal,e.request_id,e.created_at FROM managed_v2_receipts r JOIN managed_link_events e ON e.owner_crew=r.owner_crew AND e.principal=r.principal AND e.request_id=r.request_id WHERE r.retired=0 AND r.method=?1 AND NOT EXISTS(SELECT 1 FROM managed_table_audit a WHERE a.event_id=e.id) AND COALESCE(json_extract(r.result_json,'$.result.effect.rowId'),json_extract(r.result_json,'$.result.effect.tableId'))=?2 AND e.id>?3 ORDER BY e.id LIMIT ?4")?;
        let rows = query
            .query_map(params![v2::TABLE_SUBMIT, id, after, limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(event, crew, principal, request, at)| {
                let Some(graph::ReadReceipt::Active(saved)) =
                    graph::receipt(c, &crew, &principal, &request)?
                else {
                    return Err(invalid("historyEvidence"));
                };
                let receipt: v2::Receipt = serde_json::from_value(saved.result)?;
                if receipt.event_id.as_deref() != Some(&event) {
                    return Err(invalid("historyEvidence"));
                }
                let (action, fields, original, result) = match receipt.effect {
                    v2::Effect::TableCreated {
                        field_ids,
                        schema_revision,
                        ..
                    } => (
                        HistoryAction::CreateTable,
                        field_ids,
                        None,
                        Some(schema_revision),
                    ),
                    v2::Effect::SchemaChanged {
                        changed_field_ids,
                        original_schema_revision,
                        result_schema_revision,
                        ..
                    } => (
                        HistoryAction::AlterTable,
                        changed_field_ids,
                        Some(original_schema_revision),
                        Some(result_schema_revision),
                    ),
                    v2::Effect::RowAdded { row_revision, .. } => {
                        (HistoryAction::AddRow, Vec::new(), None, Some(row_revision))
                    }
                    v2::Effect::RowPatched {
                        changed_field_ids,
                        original_row_revision,
                        result_row_revision,
                        ..
                    } => (
                        HistoryAction::PatchRow,
                        changed_field_ids,
                        Some(original_row_revision),
                        Some(result_row_revision),
                    ),
                };
                let locator = LegacyLocator {
                    scope: scope.clone(),
                    viewer: viewer.clone(),
                    crew,
                    principal: principal.clone(),
                    request: request.clone(),
                    source: source.clone(),
                    codec: LegacyCodec::ManagedTablesV2,
                };
                Ok((
                    event,
                    HistorySummary {
                        key: HistoryKey::LegacyReceipt {
                            codec: LegacyCodec::ManagedTablesV2,
                            request_id: request,
                            entry_locator: graph::seal(
                                c,
                                "managed-table-history-legacy.v1",
                                &serde_json::to_vec(&locator)?,
                            )?,
                        },
                        action,
                        actor: legacy_actor(&principal),
                        recorded_at: at,
                        reason_preview: None,
                        reason_available: false,
                        changed_field_ids: fields,
                        original_revision: original,
                        result_revision: result,
                        legacy_details_unavailable: true,
                        restricted_details: false,
                    },
                ))
            })
            .collect()
    } else {
        let key: Vec<String> = if after.is_empty() {
            vec![String::new(), String::new(), String::new()]
        } else {
            serde_json::from_str(after)?
        };
        if key.len() != 3 {
            return Err(schema_changed());
        }
        let mut query=c.prepare("SELECT crew_id,principal,request_id FROM managed_object_receipts r WHERE COALESCE(json_extract(result_json,'$.object.row.id'),json_extract(result_json,'$.object.table.id'))=?1 AND (crew_id,principal,request_id)>(?2,?3,?4) AND NOT EXISTS(SELECT 1 FROM managed_table_audit_legacy a WHERE a.crew_id=r.crew_id AND a.principal=r.principal AND a.request_id=r.request_id AND a.codec='managedObjectsV1') ORDER BY crew_id,principal,request_id LIMIT ?5")?;
        let rows = query
            .query_map(params![id, key[0], key[1], key[2], limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(crew, principal, request)| {
                let next = serde_json::to_string(&(&crew, &principal, &request))?;
                let (_, receipt, _) =
                    crewd_store::managed_objects::receipt(c, &crew, &principal, &request)?
                        .ok_or_else(|| invalid("historyEvidence"))?;
                let result = match receipt.object {
                    crewd_proto::managed_objects::Object::Row { row } => Some(row.revision),
                    _ => None,
                };
                let locator = LegacyLocator {
                    scope: scope.clone(),
                    viewer: viewer.clone(),
                    crew,
                    principal: principal.clone(),
                    request: request.clone(),
                    source: source.clone(),
                    codec: LegacyCodec::ManagedObjectsV1,
                };
                Ok((
                    next,
                    HistorySummary {
                        key: HistoryKey::LegacyReceipt {
                            codec: LegacyCodec::ManagedObjectsV1,
                            request_id: request,
                            entry_locator: graph::seal(
                                c,
                                "managed-table-history-legacy.v1",
                                &serde_json::to_vec(&locator)?,
                            )?,
                        },
                        action: if matches!(source, HistorySource::Table { .. }) {
                            HistoryAction::CreateTable
                        } else {
                            HistoryAction::LegacyRowWrite
                        },
                        actor: legacy_actor(&principal),
                        recorded_at: receipt.recorded_at,
                        reason_preview: None,
                        reason_available: false,
                        changed_field_ids: Vec::new(),
                        original_revision: None,
                        result_revision: result,
                        legacy_details_unavailable: true,
                        restricted_details: false,
                    },
                ))
            })
            .collect()
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValueCursor {
    scope: Scope,
    principal: String,
    source: HistorySource,
    key: HistoryKey,
    path: HistoryPath,
    digest: String,
    offset: usize,
}
fn value_offset(c: &Connection, a: Actor<'_>, q: &HistoryValue, digest: &str) -> Result<usize> {
    let Some(raw) = &q.cursor else { return Ok(0) };
    let cursor: ValueCursor = serde_json::from_slice(
        &graph::open(c, "managed-table-history-value.v1", raw).map_err(|_| schema_changed())?,
    )?;
    if cursor.scope != objects::scope(c)?
        || cursor.principal != a.principal(c)?
        || cursor.source != q.source
        || cursor.key != q.key
        || cursor.path != q.path
        || cursor.digest != digest
    {
        return Err(schema_changed());
    }
    Ok(cursor.offset)
}
fn value_cursor(
    c: &Connection,
    a: Actor<'_>,
    q: &HistoryValue,
    digest: String,
    offset: usize,
) -> Result<String> {
    let token = graph::seal(
        c,
        "managed-table-history-value.v1",
        &serde_json::to_vec(&ValueCursor {
            scope: objects::scope(c)?,
            principal: a.principal(c)?,
            source: q.source.clone(),
            key: q.key.clone(),
            path: q.path.clone(),
            digest,
            offset,
        })?,
    )?;
    if token.len() > 16384 {
        return Err(invalid("historyCursorBytes"));
    }
    Ok(token)
}
pub fn history_value(
    c: &Connection,
    a: Actor<'_>,
    q: HistoryValue,
    x: &dyn ExternalTargets,
) -> Result<ValueChunk> {
    let source = history_number(c, a, &q.source)?;
    let value = match &q.key {
        HistoryKey::Event { event_id } => {
            if !crewd_proto::valid_remote_uuid(event_id) {
                return Err(invalid("eventId"));
            }
            db::audit_value(c, source, event_id, &q.path)?
        }
        HistoryKey::LegacyReceipt {
            codec,
            request_id,
            entry_locator,
        } => {
            let locator: LegacyLocator = serde_json::from_slice(
                &graph::open(c, "managed-table-history-legacy.v1", entry_locator)
                    .map_err(|_| schema_changed())?,
            )?;
            if locator.scope != objects::scope(c)?
                || locator.viewer != a.principal(c)?
                || locator.request != *request_id
                || locator.codec != *codec
                || locator.source != q.source
            {
                return Err(schema_changed());
            }
            // A legacy scalar/schema snapshot is available only when the
            // original codec actually retained it. Never fabricate a before.
            legacy_value(c, &locator, &q.path)?
        }
    };
    let Some((kind, body)) = value else {
        return Ok(ValueChunk::Unavailable {});
    };
    if matches!(q.path, HistoryPath::Links { .. }) {
        let refs: Vec<serde_json::Value> = serde_json::from_str(&body)?;
        let mut projection = links::Projection::new(c, a, x);
        let mut visible = Vec::new();
        let mut restricted = false;
        for item in refs {
            let target: v2::TargetRef = serde_json::from_value(
                item.get("target")
                    .cloned()
                    .ok_or_else(|| invalid("historyEvidence"))?,
            )?;
            match projection.resolve(&target, links::ResolveMode::Read) {
                Ok(v) => visible.push(v2::VisibleLink {
                    edge_id: item
                        .get("edgeId")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| invalid("historyEvidence"))?
                        .into(),
                    target: v.target,
                    summary: v.summary,
                }),
                Err(CollabError::Forbidden(_) | CollabError::NotFound(_)) => restricted = true,
                Err(e) => return Err(e),
            }
        }
        let digest = graph::hash(&serde_json::to_vec(&(&visible, restricted))?);
        let offset = value_offset(c, a, &q, &digest)?;
        if offset > visible.len() {
            return Err(schema_changed());
        }
        let mut best = None;
        for count in 0..=visible.len() - offset {
            let next = offset + count;
            let result = ValueChunk::Links {
                scope: objects::scope(c)?,
                source: q.source.clone(),
                key: q.key.clone(),
                path: q.path.clone(),
                visible: visible[offset..next].to_vec(),
                restricted_details: restricted,
                next_cursor: if next < visible.len() {
                    Some(value_cursor(c, a, &q, digest.clone(), next)?)
                } else {
                    None
                },
            };
            if serde_json::to_vec(&result)?.len() > HISTORY_ENCODED_BYTES {
                break;
            }
            best = Some(result);
        }
        let result = best.ok_or_else(|| invalid("historyTargetBytes"))?;
        if matches!(&result,ValueChunk::Links{visible:items,..}if items.is_empty()&&offset<visible.len())
        {
            return Err(invalid("historyTargetBytes"));
        }
        return Ok(result);
    }

    let digest = graph::hash(body.as_bytes());
    let offset = value_offset(c, a, &q, &digest)?;
    if offset > body.len() || !body.is_char_boundary(offset) {
        return Err(schema_changed());
    }
    let cap = offset
        .saturating_add(HISTORY_CHUNK_UTF8_BYTES)
        .min(body.len());
    let boundaries = (offset..=cap)
        .filter(|n| body.is_char_boundary(*n))
        .collect::<Vec<_>>();
    let mut low = 0;
    let mut high = boundaries.len();
    let mut best = None;
    while low < high {
        let index = low + (high - low) / 2;
        let end = boundaries[index];
        let result = ValueChunk::Scalar {
            scope: objects::scope(c)?,
            source: q.source.clone(),
            key: q.key.clone(),
            path: q.path.clone(),
            value_type: kind,
            utf8: body[offset..end].into(),
            next_cursor: if end < body.len() {
                Some(value_cursor(c, a, &q, digest.clone(), end)?)
            } else {
                None
            },
        };
        if serde_json::to_vec(&result)?.len() <= HISTORY_ENCODED_BYTES {
            best = Some((end, result));
            low = index + 1
        } else {
            high = index
        }
    }
    let (end, result) = best.ok_or_else(|| invalid("historyValueBytes"))?;
    if end == offset && offset < body.len() {
        return Err(invalid("historyValueBytes"));
    }
    Ok(result)
}
fn legacy_value(
    c: &Connection,
    l: &LegacyLocator,
    path: &HistoryPath,
) -> Result<Option<(HistoryValueType, String)>> {
    match l.codec {
        LegacyCodec::ManagedTablesV2 => {
            let Some(graph::ReadReceipt::Active(saved)) =
                graph::receipt(c, &l.crew, &l.principal, &l.request)?
            else {
                return Ok(None);
            };
            if saved.method != v2::TABLE_SUBMIT {
                return Err(invalid("historyEvidence"));
            }
            let r: v2::Receipt = serde_json::from_value(saved.result)?;
            if let (Some(event), HistoryPath::Schema { side }) = (r.event_id, path) {
                let column = if *side == HistorySide::Before {
                    "before_json"
                } else {
                    "after_json"
                };
                let sql =
                    format!("SELECT {column} FROM managed_table_schema_events WHERE event_id=?1");
                return Ok(c
                    .query_row(&sql, [event], |r| r.get::<_, String>(0))
                    .optional()?
                    .map(|s| (HistoryValueType::Schema, s)));
            }
            Ok(None)
        }
        LegacyCodec::ManagedObjectsV1 => {
            let Some((_, receipt, _)) =
                crewd_store::managed_objects::receipt(c, &l.crew, &l.principal, &l.request)?
            else {
                return Ok(None);
            };
            if let (
                crewd_proto::managed_objects::Object::Row { row },
                HistoryPath::Scalar {
                    field_id,
                    side: HistorySide::After,
                },
            ) = (receipt.object, path)
            {
                // V1 positions can only be recovered when the current original
                // layout is still exactly witnessed; no guessed position IDs.
                let t = db::table(c, num(&row.table_id, 'T')?)?
                    .ok_or_else(|| missing(&row.table_id))?;
                if !t.v1_writable {
                    return Ok(None);
                }
                let index = t
                    .fields
                    .iter()
                    .position(|f| f.id == *field_id)
                    .ok_or_else(|| invalid("fieldId"))?;
                if let Some(value) = row.values.get(index) {
                    return Ok(Some(values::scalar_text(&values::from_v2_cell(
                        &old_db::from_v1(value),
                    ))?));
                }
            }
            Ok(None)
        }
    }
}
#[cfg(test)]
#[path = "managed_tables_v3_tests.rs"]
mod tests;

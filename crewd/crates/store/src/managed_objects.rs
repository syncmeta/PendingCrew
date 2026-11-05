//! Core-only storage for explicitly declared organization objects. Content,
//! identity and original receipt are joined by the collaboration transaction.
use crate::{
    Result, StoreError,
    object_refs::{self, ObjectKind, ObjectReference},
};
use crewd_proto::managed_objects::{Cell, Field, FieldType, Object, Receipt, Row, Table};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

pub const DDL: &str = r#"
CREATE TABLE managed_tables (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    title TEXT NOT NULL,
    fields_json TEXT NOT NULL CHECK(json_valid(fields_json) AND json_type(fields_json)='array')
);
CREATE TABLE managed_rows (
    number INTEGER PRIMARY KEY REFERENCES object_refs(number) ON DELETE CASCADE,
    table_number INTEGER NOT NULL REFERENCES managed_tables(number) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    values_json TEXT NOT NULL CHECK(json_valid(values_json) AND json_type(values_json)='array')
);
CREATE INDEX managed_rows_by_table ON managed_rows(table_number,number);
CREATE TABLE managed_object_receipts (
    crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
    principal TEXT NOT NULL CHECK(length(principal) BETWEEN 1 AND 512),
    request_id TEXT NOT NULL,
    digest BLOB NOT NULL CHECK(length(digest)=32),
    result_json TEXT NOT NULL CHECK(json_valid(result_json) AND json_type(result_json)='object'),
    PRIMARY KEY(crew_id,principal,request_id)
) WITHOUT ROWID;
"#;

const MAX_FIELDS: usize = 32;
const MAX_ROW_BYTES: usize = 65_536;

fn invalid(reason: &str) -> StoreError {
    StoreError::Invalid(reason.into())
}
fn title_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
pub fn definition_valid(title: &str, fields: &[Field]) -> bool {
    title_valid(title)
        && !fields.is_empty()
        && fields.len() <= MAX_FIELDS
        && fields.iter().all(|f| title_valid(&f.name))
        && fields.iter().map(|f| &f.name).collect::<HashSet<_>>().len() == fields.len()
}
pub fn values_valid(fields: &[Field], values: &[Cell]) -> Result<bool> {
    if fields.len() != values.len()
        || !fields
            .iter()
            .zip(values)
            .all(|(field, value)| match (field.field_type, value) {
                (_, Cell::Null) => field.nullable,
                (FieldType::Text, Cell::Text(text)) => text.len() <= MAX_ROW_BYTES,
                (FieldType::Integer, Cell::Integer(text)) => {
                    text.parse::<i64>().is_ok_and(|n| n.to_string() == *text)
                }
                (FieldType::Real, Cell::Real(n)) => n.is_finite(),
                _ => false,
            })
    {
        return Ok(false);
    }
    Ok(serde_json::to_vec(values)?.len() <= MAX_ROW_BYTES)
}
pub(crate) fn transaction(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(invalid(
            "managed object mutation requires a Core transaction",
        ));
    }
    Ok(())
}
pub(crate) fn source(kind: ObjectKind, id: &str) -> bool {
    id.strip_prefix(match kind {
        ObjectKind::Table => "managed-table:",
        ObjectKind::Decision => "managed-decision:",
        ObjectKind::Lesson => "managed-lesson:",
        ObjectKind::Risk => "managed-risk:",
        ObjectKind::CustomRow => "managed-row:",
        _ => return false,
    })
    .is_some_and(crewd_proto::valid_remote_uuid)
}
pub fn table(c: &Connection, number: i64) -> Result<Option<Table>> {
    let record = c
        .query_row(
            "SELECT r.crew_id,r.source_id,t.title,t.fields_json,c.archived_at IS NOT NULL
         FROM managed_tables t JOIN object_refs r ON t.number=r.number AND r.kind='table'
         JOIN crews c ON c.id=r.crew_id WHERE t.number=?1",
            [number],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            },
        )
        .optional()?;
    record
        .map(|(crew_id, raw_source, title, fields, frozen)| {
            if !source(ObjectKind::Table, &raw_source) {
                return Err(invalid("managed table source identity is invalid"));
            }
            let fields: Vec<Field> = serde_json::from_str(&fields)?;
            if !definition_valid(&title, &fields) {
                return Err(invalid("managed table definition is invalid"));
            }
            Ok(Table {
                id: format!("T{number}"),
                crew_id,
                title,
                fields,
                frozen,
            })
        })
        .transpose()
}
pub fn row(c: &Connection, number: i64) -> Result<Option<Row>> {
    let record = c.query_row(
        "SELECT r.crew_id,r.source_id,v.table_number,t.source_id,v.revision,v.values_json,c.archived_at IS NOT NULL,m.fields_json
         FROM managed_rows v JOIN object_refs r ON v.number=r.number AND r.kind='custom_row'
         JOIN managed_tables m ON m.number=v.table_number
         JOIN object_refs t ON t.number=m.number AND t.kind='table' AND t.crew_id=r.crew_id
         JOIN crews c ON c.id=r.crew_id WHERE v.number=?1",
        [number],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?,r.get::<_,bool>(6)?,r.get::<_,String>(7)?)),
    ).optional()?;
    record
        .map(
            |(crew_id, raw_source, table, table_source, revision, values, frozen, fields)| {
                if !source(ObjectKind::CustomRow, &raw_source)
                    || !source(ObjectKind::Table, &table_source)
                {
                    return Err(invalid("managed row source identity is invalid"));
                }
                let fields: Vec<Field> = serde_json::from_str(&fields)?;
                let values: Vec<Cell> = serde_json::from_str(&values)?;
                if !values_valid(&fields, &values)? {
                    return Err(invalid("managed row values are invalid"));
                }
                Ok(Row {
                    id: format!("O{number}"),
                    crew_id,
                    table_id: format!("T{table}"),
                    revision: revision.to_string(),
                    values,
                    frozen,
                })
            },
        )
        .transpose()
}

pub fn table_numbers(c: &Connection, after: i64, limit: u16) -> Result<Vec<i64>> {
    let mut q =
        c.prepare("SELECT number FROM managed_tables WHERE number>?1 ORDER BY number LIMIT ?2")?;
    Ok(q.query_map(params![after, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn row_numbers(c: &Connection, table_number: i64, after: i64, limit: u16) -> Result<Vec<i64>> {
    let mut q = c.prepare("SELECT number FROM managed_rows WHERE table_number=?1 AND number>?2 ORDER BY number LIMIT ?3")?;
    Ok(
        q.query_map(params![table_number, after, limit], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?,
    )
}
pub(crate) fn allocate(
    c: &Connection,
    kind: ObjectKind,
    crew: &str,
    at: &str,
) -> Result<ObjectReference> {
    let tag = match kind {
        ObjectKind::Table => "managed-table",
        ObjectKind::Decision => "managed-decision",
        ObjectKind::Lesson => "managed-lesson",
        ObjectKind::Risk => "managed-risk",
        ObjectKind::CustomRow => "managed-row",
        _ => return Err(invalid("invalid managed object allocation kind")),
    };
    let source = format!("{tag}:{}", uuid::Uuid::new_v4());
    // Keep an independent floor in the same transaction as the managed
    // binding. Crew purge can remove the last alias but never this witness.
    object_refs::preserve_restore_floor(c, 0)?;
    let reference = object_refs::bind(c, kind, crew, &source, at)?;
    object_refs::preserve_restore_floor(c, reference.number)?;
    Ok(reference)
}
pub fn create_table(
    c: &Connection,
    crew: &str,
    title: &str,
    fields: &[Field],
    at: &str,
) -> Result<Table> {
    transaction(c)?;
    let r = allocate(c, ObjectKind::Table, crew, at)?;
    c.execute(
        "INSERT INTO managed_tables(number,title,fields_json) VALUES(?1,?2,?3)",
        params![r.number, title, serde_json::to_string(fields)?],
    )?;
    table(c, r.number)?.ok_or_else(|| invalid("committed table source missing"))
}
pub fn add_row(
    c: &Connection,
    crew: &str,
    table_number: i64,
    values: &[Cell],
    at: &str,
) -> Result<Row> {
    transaction(c)?;
    let r = allocate(c, ObjectKind::CustomRow, crew, at)?;
    c.execute(
        "INSERT INTO managed_rows(number,table_number,revision,values_json) VALUES(?1,?2,1,?3)",
        params![r.number, table_number, serde_json::to_string(values)?],
    )?;
    row(c, r.number)?.ok_or_else(|| invalid("committed row source missing"))
}
pub fn edit_row(c: &Connection, number: i64, expected: i64, values: &[Cell]) -> Result<Row> {
    transaction(c)?;
    let revision = expected
        .checked_add(1)
        .ok_or_else(|| invalid("managed row revision exhausted"))?;
    let updated = c.execute(
        "UPDATE managed_rows SET values_json=?3,revision=?4 WHERE number=?1 AND revision=?2",
        params![number, expected, serde_json::to_string(values)?, revision],
    )?;
    if updated != 1 {
        return Err(invalid("managed row version changed"));
    }
    row(c, number)?.ok_or_else(|| invalid("committed row source missing"))
}

pub fn receipt(
    c: &Connection,
    crew: &str,
    principal: &str,
    request: &str,
) -> Result<Option<([u8; 32], Receipt, bool)>> {
    let record = c.query_row(
        "SELECT digest,result_json,member_confirmation FROM managed_object_receipts WHERE crew_id=?1 AND principal=?2 AND request_id=?3",
        params![crew,principal,request],
        |r| Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?)),
    ).optional()?;
    record
        .map(|(digest, raw, member_confirmation)| {
            let digest = digest
                .try_into()
                .map_err(|_| invalid("managed object receipt digest is invalid"))?;
            let receipt: Receipt = serde_json::from_str(&raw)?;
            let actual_crew = match &receipt.object {
                Object::Table { table } => &table.crew_id,
                Object::Row { row } => &row.crew_id,
                Object::Decision { decision } => &decision.crew_id,
                Object::Risk { risk } => &risk.crew_id,
                Object::Lesson { lesson } => &lesson.crew_id,
            };
            if receipt.crew_id != crew || receipt.request_id != request || actual_crew != crew {
                return Err(invalid("managed object receipt scope mismatch"));
            }
            match (&receipt.object, &receipt.occurrence) {
                (Object::Lesson { lesson }, Some(effect)) if !member_confirmation
                    && lesson.id == effect.lesson_id
                    && lesson.revision == effect.lesson_revision
                    && lesson.risk_id.as_ref() == Some(&effect.risk_id)
                    && next_revision(&effect.original_risk_revision, &effect.result_risk_revision) => {}
                (_, None) if !member_confirmation || matches!(&receipt.object, Object::Lesson { .. }) => {}
                _ => return Err(invalid("managed object receipt effect mismatch")),
            }
            Ok((digest, receipt, member_confirmation))
        })
        .transpose()
}
fn next_revision(original: &str, result: &str) -> bool {
    original.parse::<i64>().ok()
        .filter(|n| *n > 0 && n.to_string() == original)
        .and_then(|n| n.checked_add(1))
        .is_some_and(|n| n.to_string() == result)
}

pub fn record(
    c: &Connection,
    principal: &str,
    digest: &[u8; 32],
    receipt: &Receipt,
    member_confirmation: bool,
) -> Result<()> {
    transaction(c)?;
    let result = serde_json::to_string(receipt)?;
    c.execute(
        "INSERT INTO managed_object_receipts(crew_id,principal,request_id,digest,result_json,member_confirmation) VALUES(?1,?2,?3,?4,?5,?6)",
        params![receipt.crew_id,principal,receipt.request_id,digest.as_slice(),result,member_confirmation],
    )?;
    Ok(())
}

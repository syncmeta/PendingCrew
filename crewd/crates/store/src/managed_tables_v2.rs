//! Stable table fields. `fields_json` and `values_json` remain the sole schema
//! and scalar cell authorities; links live only in managed_links.
use crate::{Result, StoreError, managed_objects as v1, object_refs::ObjectKind};
use crewd_proto::{managed_objects as old, managed_tables_v2::*};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
pub const DDL: &str = r#"
CREATE TABLE managed_table_attributes(
 number INTEGER PRIMARY KEY REFERENCES managed_tables(number) ON DELETE CASCADE,
 schema_revision INTEGER NOT NULL CHECK(schema_revision>0),next_field_number INTEGER NOT NULL CHECK(next_field_number>0),
 purpose TEXT,title_field_id TEXT NOT NULL,row_policy TEXT NOT NULL,author_json TEXT NOT NULL CHECK(json_valid(author_json)),
 v1_layout INTEGER NOT NULL CHECK(v1_layout IN(0,1))
);
CREATE TABLE managed_row_authors(number INTEGER PRIMARY KEY REFERENCES managed_rows(number) ON DELETE CASCADE,author_json TEXT NOT NULL CHECK(json_valid(author_json)));
"#;
fn bad() -> StoreError {
    StoreError::Invalid("invalid stable table schema, values or identity".into())
}
pub fn enabled(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='managed_table_attributes')",[],|r|r.get(0))?)
}
pub fn number(s: &str, prefix: char) -> Result<i64> {
    let v = s.strip_prefix(prefix).ok_or_else(bad)?;
    let n = v.parse::<i64>().map_err(|_| bad())?;
    if n <= 0 || n.to_string() != v {
        return Err(bad());
    }
    Ok(n)
}
pub fn revision(s: &str) -> Result<i64> {
    let n = s.parse::<i64>().map_err(|_| bad())?;
    if n <= 0 || n.to_string() != s {
        return Err(bad());
    }
    Ok(n)
}
pub fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && s.trim() == s && !s.chars().any(char::is_control)
}
pub fn valid_fields(fields: &[Field], title: &str, next: i64) -> bool {
    !fields.is_empty()
        && fields.len() <= 64
        && next > 0
        && fields
            .iter()
            .all(|f| text(&f.name, 120) && number(&f.id, 'p').is_ok_and(|n| n < next))
        && fields.iter().map(|f| &f.id).collect::<HashSet<_>>().len() == fields.len()
        && fields.iter().map(|f| &f.name).collect::<HashSet<_>>().len() == fields.len()
        && fields
            .iter()
            .any(|f| f.id == title && !f.hidden && !matches!(f.kind, FieldKind::Link { .. }))
}
pub fn scalar_valid(f: &Field, v: &Cell) -> bool {
    match (&f.kind, v) {
        (_, Cell::Null) => f.nullable,
        (FieldKind::Text {}, Cell::Text(t)) => t.len() <= 65536,
        (FieldKind::Integer {}, Cell::Integer(s)) => {
            s.parse::<i64>().is_ok_and(|n| n.to_string() == *s)
        }
        (FieldKind::Real {}, Cell::Real(n)) => n.is_finite(),
        _ => false,
    }
}
fn convert_field(i: usize, f: &old::Field) -> Field {
    Field {
        id: format!("p{}", i + 1),
        name: f.name.clone(),
        kind: match f.field_type {
            old::FieldType::Text => FieldKind::Text {},
            old::FieldType::Integer => FieldKind::Integer {},
            old::FieldType::Real => FieldKind::Real {},
        },
        nullable: f.nullable,
        hidden: false,
    }
}
pub fn from_v1(v: &old::Cell) -> Cell {
    match v {
        old::Cell::Null => Cell::Null,
        old::Cell::Text(s) => Cell::Text(s.clone()),
        old::Cell::Integer(s) => Cell::Integer(s.clone()),
        old::Cell::Real(n) => Cell::Real(*n),
    }
}
pub fn to_v1(v: &Cell) -> old::Cell {
    match v {
        Cell::Null => old::Cell::Null,
        Cell::Text(s) => old::Cell::Text(s.clone()),
        Cell::Integer(s) => old::Cell::Integer(s.clone()),
        Cell::Real(n) => old::Cell::Real(*n),
    }
}
pub fn migrate_v1(c: &Connection) -> Result<()> {
    v1::transaction(c)?;
    let carried_floor = crate::managed_field_allocator::allocator_floor(c)?;
    let tables = {
        let mut q =
            c.prepare("SELECT number,title,fields_json FROM managed_tables ORDER BY number")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (n, title, raw) in tables {
        let fs: Vec<old::Field> = serde_json::from_str(&raw)?;
        if !v1::definition_valid(&title, &fs) {
            return Err(bad());
        }
        let fields = fs
            .iter()
            .enumerate()
            .map(|(i, f)| convert_field(i, f))
            .collect::<Vec<_>>();
        let rows = {
            let mut q = c.prepare(
                "SELECT number,values_json FROM managed_rows WHERE table_number=?1 ORDER BY number",
            )?;
            q.query_map([n], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (r, raw) in rows {
            let vs: Vec<old::Cell> = serde_json::from_str(&raw)?;
            if !v1::values_valid(&fs, &vs)? {
                return Err(bad());
            }
            let cells = vs
                .iter()
                .enumerate()
                .map(|(i, v)| ScalarValue {
                    field_id: fields[i].id.clone(),
                    value: from_v1(v),
                })
                .collect::<Vec<_>>();
            c.execute(
                "UPDATE managed_rows SET values_json=?2 WHERE number=?1",
                params![r, serde_json::to_string(&cells)?],
            )?;
            c.execute(
                "INSERT INTO managed_row_authors VALUES(?1,?2)",
                params![r, serde_json::to_string(&Author::LegacyUnknown {})?],
            )?;
        }
        c.execute(
            "UPDATE managed_tables SET fields_json=?2 WHERE number=?1",
            params![n, serde_json::to_string(&fields)?],
        )?;
        let next = (fields.len() as i64 + 1).max(carried_floor);
        c.execute("INSERT INTO managed_table_attributes VALUES(?1,1,?2,NULL,'p1','legacyCrewWriters',?3,1)",params![n,next,serde_json::to_string(&Author::LegacyUnknown {})?])?;
        crate::managed_field_allocator::remember_floor(c, next)?;
    }
    Ok(())
}
pub fn table(c: &Connection, n: i64) -> Result<Option<Table>> {
    let record=c.query_row("SELECT r.crew_id,r.source_id,t.title,t.fields_json,a.schema_revision,a.next_field_number,a.purpose,a.title_field_id,a.row_policy,a.author_json,a.v1_layout,c.archived_at IS NOT NULL FROM managed_tables t JOIN managed_table_attributes a ON a.number=t.number JOIN object_refs r ON r.number=t.number AND r.kind='table' JOIN crews c ON c.id=r.crew_id WHERE t.number=?1",[n],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,bool>(10)?,r.get::<_,bool>(11)?))).optional()?;
    record
        .map(
            |(
                crew,
                source,
                title,
                fs,
                rev,
                next,
                purpose,
                title_field_id,
                policy,
                author,
                layout,
                frozen,
            )| {
                let fields: Vec<Field> = serde_json::from_str(&fs)?;
                if !v1::source(ObjectKind::Table, &source)
                    || !text(&title, 256)
                    || rev <= 0
                    || !valid_fields(&fields, &title_field_id, next)
                {
                    return Err(bad());
                }
                let compatible = fields
                    .iter()
                    .all(|f| !f.hidden && !matches!(f.kind, FieldKind::Link { .. }));
                Ok(Table {
                    id: format!("T{n}"),
                    crew_id: crew,
                    schema_revision: rev.to_string(),
                    title,
                    purpose,
                    title_field_id,
                    row_write_policy: serde_json::from_value(serde_json::Value::String(policy))?,
                    author: serde_json::from_str(&author)?,
                    fields,
                    next_field_number: next.to_string(),
                    frozen,
                    v1_readable: compatible,
                    v1_writable: compatible && layout && rev == 1,
                })
            },
        )
        .transpose()
}
pub fn scalars(c: &Connection, n: i64) -> Result<Vec<ScalarValue>> {
    let raw: String = c.query_row(
        "SELECT values_json FROM managed_rows WHERE number=?1",
        [n],
        |r| r.get(0),
    )?;
    let values: Vec<ScalarValue> = serde_json::from_str(&raw)?;
    if values
        .iter()
        .map(|v| &v.field_id)
        .collect::<HashSet<_>>()
        .len()
        != values.len()
    {
        return Err(bad());
    }
    Ok(values)
}
pub type RowBase = (Table, String, Author, Vec<ScalarValue>);
pub fn row_base(c: &Connection, n: i64) -> Result<Option<RowBase>> {
    let r=c.query_row("SELECT r.crew_id,r.source_id,v.table_number,v.revision,a.author_json FROM managed_rows v JOIN object_refs r ON r.number=v.number AND r.kind='custom_row' JOIN managed_row_authors a ON a.number=v.number WHERE v.number=?1",[n],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?))).optional()?;
    r.map(|(crew, source, tn, rev, author)| {
        let t = table(c, tn)?.ok_or_else(bad)?;
        if t.crew_id != crew || !v1::source(ObjectKind::CustomRow, &source) || rev <= 0 {
            return Err(bad());
        }
        let vs = scalars(c, n)?;
        for v in &vs {
            let f = t
                .fields
                .iter()
                .find(|f| f.id == v.field_id)
                .ok_or_else(bad)?;
            if !scalar_valid(f, &v.value) {
                return Err(bad());
            }
        }
        Ok((t, rev.to_string(), serde_json::from_str(&author)?, vs))
    })
    .transpose()
}
pub fn v1_table(c: &Connection, n: i64) -> Result<Option<old::Table>> {
    table(c, n)?
        .map(|t| {
            if !t.v1_readable {
                return Err(bad());
            }
            Ok(old::Table {
                id: t.id,
                crew_id: t.crew_id,
                title: t.title,
                fields: t
                    .fields
                    .iter()
                    .map(|f| {
                        Ok(old::Field {
                            name: f.name.clone(),
                            field_type: match f.kind {
                                FieldKind::Text {} => old::FieldType::Text,
                                FieldKind::Integer {} => old::FieldType::Integer,
                                FieldKind::Real {} => old::FieldType::Real,
                                _ => return Err(bad()),
                            },
                            nullable: f.nullable,
                        })
                    })
                    .collect::<Result<_>>()?,
                frozen: t.frozen,
            })
        })
        .transpose()
}
pub fn v1_row(c: &Connection, n: i64) -> Result<Option<old::Row>> {
    row_base(c, n)?
        .map(|(t, rev, _, vs)| {
            if !t.v1_readable {
                return Err(bad());
            }
            Ok(old::Row {
                id: format!("O{n}"),
                crew_id: t.crew_id,
                table_id: t.id,
                revision: rev,
                values: t
                    .fields
                    .iter()
                    .map(|f| {
                        vs.iter()
                            .find(|v| v.field_id == f.id)
                            .map(|v| to_v1(&v.value))
                            .unwrap_or(old::Cell::Null)
                    })
                    .collect(),
                frozen: t.frozen,
            })
        })
        .transpose()
}
pub fn numbers(
    c: &Connection,
    table: Option<i64>,
    after: i64,
    limit: u16,
    v1_only: bool,
) -> Result<Vec<i64>> {
    let sql = if table.is_some() {
        "SELECT r.number FROM managed_rows r WHERE r.table_number=?1 AND r.number>?2 ORDER BY r.number LIMIT ?3"
    } else if v1_only {
        "SELECT t.number FROM managed_tables t JOIN managed_table_attributes a ON a.number=t.number WHERE t.number>?2 AND NOT EXISTS(SELECT 1 FROM json_each(t.fields_json) f WHERE json_extract(f.value,'$.hidden')=1 OR json_extract(f.value,'$.kind.kind')='link') ORDER BY t.number LIMIT ?3"
    } else {
        "SELECT number FROM managed_tables WHERE number>?2 ORDER BY number LIMIT ?3"
    };
    let mut q = c.prepare(sql)?;
    Ok(q.query_map(params![table, after, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
pub struct NewTable<'a> {
    pub crew: &'a str,
    pub title: &'a str,
    pub purpose: Option<&'a str>,
    pub fields: &'a [Field],
    pub title_field: &'a str,
    pub policy: RowWritePolicy,
    pub author: &'a Author,
    pub now: &'a str,
}
pub fn create(c: &Connection, input: NewTable<'_>) -> Result<Table> {
    let NewTable {
        crew,
        title,
        purpose,
        fields,
        title_field,
        policy,
        author,
        now,
    } = input;
    v1::transaction(c)?;
    let r = v1::allocate(c, ObjectKind::Table, crew, now)?;
    c.execute(
        "INSERT INTO managed_tables VALUES(?1,?2,?3)",
        params![r.number, title, serde_json::to_string(fields)?],
    )?;
    c.execute(
        "INSERT INTO managed_table_attributes VALUES(?1,1,?2,?3,?4,?5,?6,1)",
        params![
            r.number,
            fields.len() as i64 + 1,
            purpose,
            title_field,
            serde_json::to_value(policy)?.as_str().ok_or_else(bad)?,
            serde_json::to_string(author)?
        ],
    )?;
    crate::managed_field_allocator::remember_floor(c, fields.len() as i64 + 1)?;
    table(c, r.number)?.ok_or_else(bad)
}
pub fn save_table(c: &Connection, t: &Table, expected: i64) -> Result<()> {
    v1::transaction(c)?;
    let n = number(&t.id, 'T')?;
    let rev = revision(&t.schema_revision)?;
    if c.execute("UPDATE managed_table_attributes SET schema_revision=?3,next_field_number=?4,purpose=?5,title_field_id=?6,v1_layout=0 WHERE number=?1 AND schema_revision=?2",params![n,expected,rev,revision(&t.next_field_number)?,t.purpose,t.title_field_id])?!=1{return Err(bad())}
    c.execute(
        "UPDATE managed_tables SET title=?2,fields_json=?3 WHERE number=?1",
        params![n, t.title, serde_json::to_string(&t.fields)?],
    )?;
    crate::managed_field_allocator::remember_floor(c, revision(&t.next_field_number)?)?;
    Ok(())
}
pub fn add(
    c: &Connection,
    t: &Table,
    author: &Author,
    cells: &[ScalarValue],
    now: &str,
) -> Result<i64> {
    v1::transaction(c)?;
    let r = v1::allocate(c, ObjectKind::CustomRow, &t.crew_id, now)?;
    c.execute(
        "INSERT INTO managed_rows VALUES(?1,?2,1,?3)",
        params![r.number, number(&t.id, 'T')?, serde_json::to_string(cells)?],
    )?;
    c.execute(
        "INSERT INTO managed_row_authors VALUES(?1,?2)",
        params![r.number, serde_json::to_string(author)?],
    )?;
    Ok(r.number)
}
pub fn save_row(c: &Connection, n: i64, expected: i64, values: &[ScalarValue]) -> Result<()> {
    v1::transaction(c)?;
    let next = expected.checked_add(1).ok_or_else(bad)?;
    if c.execute(
        "UPDATE managed_rows SET revision=?3,values_json=?4 WHERE number=?1 AND revision=?2",
        params![n, expected, next, serde_json::to_string(values)?],
    )? != 1
    {
        return Err(bad());
    }
    Ok(())
}
pub fn set_row_author(c: &Connection, n: i64, author: &Author) -> Result<()> {
    c.execute(
        "UPDATE managed_row_authors SET author_json=?2 WHERE number=?1",
        params![n, serde_json::to_string(author)?],
    )?;
    Ok(())
}

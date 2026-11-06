//! Authoritative V3 table metadata. Existing fields_json / values_json and graph
//! links remain the only definitions, scalar values and relationship authority.
use crate::{Result, StoreError, managed_objects as objects, managed_tables_v2 as v2};
use crewd_proto::{managed_tables_v2 as old, managed_tables_v3::*};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;

#[path = "managed_tables_v3_receipts.rs"]
mod original;
#[path = "managed_table_values.rs"]
pub mod values;
pub use original::{event_witness, validate_receipt};

pub const DDL: &str = r#"
CREATE TABLE managed_table_v3_meta(
 number INTEGER PRIMARY KEY REFERENCES managed_tables(number) ON DELETE CASCADE,
 next_option_number INTEGER NOT NULL DEFAULT 1 CHECK(typeof(next_option_number)='integer' AND next_option_number>0),
 archived_at TEXT,archived_by TEXT CHECK(archived_by IS NULL OR json_valid(archived_by)),archive_reason TEXT,
 CHECK((archived_at IS NULL AND archived_by IS NULL AND archive_reason IS NULL) OR (archived_at IS NOT NULL AND archived_by IS NOT NULL AND archive_reason IS NOT NULL))
);
CREATE TABLE managed_row_v3_meta(
 number INTEGER PRIMARY KEY REFERENCES managed_rows(number) ON DELETE CASCADE,
 archived_at TEXT,archived_by TEXT CHECK(archived_by IS NULL OR json_valid(archived_by)),archive_reason TEXT,
 CHECK((archived_at IS NULL AND archived_by IS NULL AND archive_reason IS NULL) OR (archived_at IS NOT NULL AND archived_by IS NOT NULL AND archive_reason IS NOT NULL))
);
CREATE TABLE managed_table_scalar_codecs(
 table_number INTEGER NOT NULL REFERENCES managed_tables(number) ON DELETE CASCADE,
 field_id TEXT NOT NULL,original_kind TEXT NOT NULL CHECK(original_kind IN('integer','real','decimal')),
 event_id TEXT NOT NULL REFERENCES managed_link_events(id) ON DELETE CASCADE,
 PRIMARY KEY(table_number,field_id)
) WITHOUT ROWID;
CREATE TABLE managed_table_definition_clock(
 crew_id TEXT PRIMARY KEY REFERENCES crews(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0)
) WITHOUT ROWID;
CREATE TABLE managed_table_audit(
 ordinal INTEGER PRIMARY KEY AUTOINCREMENT CHECK(ordinal>0),
 event_id TEXT NOT NULL UNIQUE,
 source_number INTEGER NOT NULL REFERENCES object_refs(number) ON DELETE CASCADE,
 action TEXT NOT NULL,actor_json TEXT NOT NULL CHECK(json_valid(actor_json)),recorded_at TEXT NOT NULL,
 reason TEXT,changed_fields_json TEXT NOT NULL CHECK(json_valid(changed_fields_json)),
 original_revision TEXT,result_revision TEXT,legacy_details INTEGER NOT NULL CHECK(legacy_details IN(0,1))
);
CREATE INDEX managed_table_audit_source ON managed_table_audit(source_number,ordinal);
CREATE TABLE managed_table_audit_values(
 event_id TEXT NOT NULL REFERENCES managed_table_audit(event_id) ON DELETE CASCADE,
 path TEXT NOT NULL,body TEXT NOT NULL,value_type TEXT NOT NULL,
 PRIMARY KEY(event_id,path)
) WITHOUT ROWID;
CREATE TABLE managed_table_audit_legacy(
 crew_id TEXT NOT NULL,principal TEXT NOT NULL,request_id TEXT NOT NULL,codec TEXT NOT NULL,
 event_id TEXT NOT NULL REFERENCES managed_table_audit(event_id) ON DELETE CASCADE,
 PRIMARY KEY(crew_id,principal,request_id,codec)
) WITHOUT ROWID;
CREATE INDEX managed_table_v3_legacy_v1_history ON managed_object_receipts(
 COALESCE(json_extract(result_json,'$.object.row.id'),json_extract(result_json,'$.object.table.id')),crew_id,principal,request_id
);
CREATE INDEX managed_table_v3_legacy_v2_history ON managed_v2_receipts(
 method,COALESCE(json_extract(result_json,'$.result.effect.rowId'),json_extract(result_json,'$.result.effect.tableId'))
) WHERE retired=0;
CREATE TRIGGER managed_table_v3_insert AFTER INSERT ON managed_tables BEGIN
 INSERT INTO managed_table_v3_meta(number) VALUES(NEW.number);
 INSERT INTO managed_table_definition_clock VALUES((SELECT crew_id FROM object_refs WHERE number=NEW.number),1)
 ON CONFLICT(crew_id) DO UPDATE SET revision=CASE WHEN revision<9223372036854775807 THEN revision+1 ELSE NULL END;
END;
CREATE TRIGGER managed_row_v3_insert AFTER INSERT ON managed_rows BEGIN
 INSERT INTO managed_row_v3_meta(number) VALUES(NEW.number);
END;
CREATE TRIGGER managed_table_v3_definition AFTER UPDATE OF title ON managed_tables WHEN NEW.title IS NOT OLD.title BEGIN
 INSERT INTO managed_table_definition_clock VALUES((SELECT crew_id FROM object_refs WHERE number=NEW.number),1)
 ON CONFLICT(crew_id) DO UPDATE SET revision=CASE WHEN revision<9223372036854775807 THEN revision+1 ELSE NULL END;
END;
CREATE TRIGGER managed_table_v3_purpose AFTER UPDATE OF purpose ON managed_table_attributes WHEN NEW.purpose IS NOT OLD.purpose BEGIN
 INSERT INTO managed_table_definition_clock VALUES((SELECT crew_id FROM object_refs WHERE number=NEW.number),1)
 ON CONFLICT(crew_id) DO UPDATE SET revision=CASE WHEN revision<9223372036854775807 THEN revision+1 ELSE NULL END;
END;
CREATE TRIGGER managed_table_v3_lifecycle AFTER UPDATE OF archived_at ON managed_table_v3_meta WHEN NEW.archived_at IS NOT OLD.archived_at BEGIN
 INSERT INTO managed_table_definition_clock VALUES((SELECT crew_id FROM object_refs WHERE number=NEW.number),1)
 ON CONFLICT(crew_id) DO UPDATE SET revision=CASE WHEN revision<9223372036854775807 THEN revision+1 ELSE NULL END;
END;
CREATE TRIGGER managed_table_v3_delete BEFORE DELETE ON object_refs WHEN OLD.kind='table' BEGIN
 INSERT INTO managed_table_definition_clock SELECT OLD.crew_id,1 WHERE EXISTS(SELECT 1 FROM crews WHERE id=OLD.crew_id)
 ON CONFLICT(crew_id) DO UPDATE SET revision=CASE WHEN revision<9223372036854775807 THEN revision+1 ELSE NULL END;
END;
"#;
fn bad() -> StoreError {
    StoreError::Invalid("invalid managed table V3 authority or original evidence".into())
}
pub fn enabled(c: &Connection) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='managed_table_v3_meta')",[],|r|r.get(0))?)
}
/// Called in the schema32 protected migration transaction, after DDL. No
/// business source row, revision, value, event, message, receipt or wake changes.
pub fn migrate(c: &Connection) -> Result<()> {
    objects::transaction(c)?;
    let floor = option_allocator_floor(c)?;
    c.execute("INSERT INTO managed_table_v3_meta(number,next_option_number) SELECT number,?1 FROM managed_tables WHERE true ON CONFLICT(number) DO NOTHING",[floor.max(1)])?;
    c.execute("INSERT INTO managed_row_v3_meta(number) SELECT number FROM managed_rows WHERE true ON CONFLICT(number) DO NOTHING",[])?;
    install_search(c)?;
    Ok(())
}
fn install_search(c: &Connection) -> Result<()> {
    if !crate::organization_search::enabled(c)? {
        return Ok(());
    }
    c.execute_batch("CREATE TABLE organization_search_table_fanout(table_number INTEGER PRIMARY KEY REFERENCES managed_tables(number) ON DELETE CASCADE,after_number INTEGER NOT NULL DEFAULT 0 CHECK(typeof(after_number)='integer' AND after_number>=0),revision INTEGER NOT NULL CHECK(typeof(revision)='integer' AND revision>0));CREATE INDEX organization_search_option_slot ON organization_search_slots(field_id,body,version_id) WHERE substr(body,1,8)='@option:';CREATE INDEX organization_search_person_slot ON organization_search_slots(body,field_id,version_id) WHERE substr(body,1,8)='@person:';")?;
    let queue = crate::organization_search::queue_sql("'o:'||printf('%020d',NEW.number)", true);
    c.execute_batch(&format!("CREATE TRIGGER organization_search_v3_row_lifecycle AFTER UPDATE OF archived_at ON managed_row_v3_meta WHEN OLD.archived_at IS NOT NEW.archived_at BEGIN {queue} END;CREATE TRIGGER organization_search_v3_table_lifecycle AFTER UPDATE OF archived_at ON managed_table_v3_meta WHEN OLD.archived_at IS NOT NEW.archived_at BEGIN {queue} INSERT INTO organization_search_table_fanout VALUES(NEW.number,0,1) ON CONFLICT(table_number) DO UPDATE SET after_number=0,revision=CASE WHEN revision<9223372036854775807 THEN revision+1 ELSE NULL END; END;"))?;
    c.execute_batch("CREATE TRIGGER organization_search_option_alias AFTER INSERT ON managed_table_schema_events BEGIN INSERT INTO organization_search_aliases(table_number,event_id,field_id,label,visible_from) SELECT NEW.table_number,NEW.event_id,'@option:'||json_extract(b.value,'$.id')||':'||json_extract(bo.value,'$.id'),json_extract(bo.value,'$.label'),(SELECT clock+1 FROM organization_search_state WHERE id=1) FROM json_each(NEW.before_json,'$.fields') b JOIN json_each(NEW.after_json,'$.fields') a ON json_extract(a.value,'$.id')=json_extract(b.value,'$.id') JOIN json_each(b.value,'$.kind.options') bo JOIN json_each(a.value,'$.kind.options') ao ON json_extract(ao.value,'$.id')=json_extract(bo.value,'$.id') WHERE json_extract(bo.value,'$.label') IS NOT json_extract(ao.value,'$.label'); END;")?;
    Ok(())
}
pub fn search_fanout_pending(c: &Connection) -> Result<bool> {
    if !enabled(c)? {
        return Ok(false);
    }
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM organization_search_table_fanout)",
        [],
        |r| r.get(0),
    )?)
}
pub fn search_fanout_step(c: &Connection, limit: u16) -> Result<usize> {
    objects::transaction(c)?;
    if limit == 0 || !enabled(c)? {
        return Ok(0);
    }
    let job=c.query_row("SELECT table_number,after_number,revision FROM organization_search_table_fanout ORDER BY table_number LIMIT 1",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?))).optional()?;
    let Some((table, after, revision)) = job else {
        return Ok(0);
    };
    let mut q=c.prepare("SELECT number FROM managed_rows WHERE table_number=?1 AND number>?2 ORDER BY number LIMIT ?3")?;
    let rows = q
        .query_map(params![table, after, limit.min(64)], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for n in &rows {
        crate::organization_search::mark(c, &crate::organization_search::source_object(*n))?;
    }
    if let Some(last) = rows.last() {
        c.execute("UPDATE organization_search_table_fanout SET after_number=?3 WHERE table_number=?1 AND revision=?2",params![table,revision,last])?;
    } else {
        c.execute(
            "DELETE FROM organization_search_table_fanout WHERE table_number=?1 AND revision=?2",
            params![table, revision],
        )?;
    }
    Ok(rows.len().max(1))
}
fn empty_lifecycle() -> Lifecycle {
    Lifecycle {
        archived_at: None,
        archived_by: None,
        reason: None,
    }
}
pub fn lifecycle(c: &Connection, n: i64, row: bool) -> Result<Lifecycle> {
    let sql = if row {
        "SELECT archived_at,archived_by,archive_reason FROM managed_row_v3_meta WHERE number=?1"
    } else {
        "SELECT archived_at,archived_by,archive_reason FROM managed_table_v3_meta WHERE number=?1"
    };
    let v = c
        .query_row(sql, [n], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?
        .ok_or_else(bad)?;
    match v {
        (None, None, None) => Ok(empty_lifecycle()),
        (Some(at), Some(by), Some(reason))
            if !at.is_empty() && v2::text(&reason, REASON_SCALARS) =>
        {
            Ok(Lifecycle {
                archived_at: Some(at),
                archived_by: Some(serde_json::from_str(&by)?),
                reason: Some(reason),
            })
        }
        _ => Err(bad()),
    }
}
pub fn set_lifecycle(c: &Connection, n: i64, row: bool, value: &Lifecycle) -> Result<()> {
    objects::transaction(c)?;
    let sql = if row {
        "UPDATE managed_row_v3_meta SET archived_at=?2,archived_by=?3,archive_reason=?4 WHERE number=?1"
    } else {
        "UPDATE managed_table_v3_meta SET archived_at=?2,archived_by=?3,archive_reason=?4 WHERE number=?1"
    };
    let by = value
        .archived_by
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    if c.execute(sql, params![n, value.archived_at, by, value.reason])? != 1 {
        return Err(bad());
    }
    Ok(())
}
pub fn definition_revision(c: &Connection, crew: &str) -> Result<i64> {
    let n = c
        .query_row(
            "SELECT revision FROM managed_table_definition_clock WHERE crew_id=?1",
            [crew],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    if n < 0 {
        return Err(bad());
    }
    Ok(n)
}
/// The actual stored codec, not a reconstructed V3 DTO. fieldsJson also retains
/// the exact original bytes of a legitimate legacy definition.
pub fn schema_snapshot(c: &Connection, n: i64) -> Result<String> {
    let value=c.query_row("SELECT t.title,t.fields_json,a.schema_revision,a.next_field_number,a.purpose,a.title_field_id,a.row_policy,a.author_json,m.next_option_number,m.archived_at,m.archived_by,m.archive_reason,r.crew_id FROM managed_tables t JOIN managed_table_attributes a ON a.number=t.number JOIN managed_table_v3_meta m ON m.number=t.number JOIN object_refs r ON r.number=t.number WHERE t.number=?1",[n],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,i64>(8)?,r.get::<_,Option<String>>(9)?,r.get::<_,Option<String>>(10)?,r.get::<_,Option<String>>(11)?,r.get::<_,String>(12)?)))?;
    Ok(serde_json::to_string(
        &serde_json::json!({"id":format!("T{n}"),"crewId":value.12,"title":value.0,"fieldsJson":value.1,"fields":serde_json::from_str::<serde_json::Value>(&value.1)?,"schemaRevision":value.2.to_string(),"nextFieldNumber":value.3.to_string(),"purpose":value.4,"titleFieldId":value.5,"rowWritePolicy":value.6,"author":serde_json::from_str::<serde_json::Value>(&value.7)?,"nextOptionNumber":value.8.to_string(),"lifecycle":{"archivedAt":value.9,"archivedBy":value.10.map(|s|serde_json::from_str::<serde_json::Value>(&s)).transpose()?,"reason":value.11}}),
    )?)
}
pub fn table(c: &Connection, n: i64) -> Result<Option<Table>> {
    let raw=c.query_row("SELECT r.crew_id,r.source_id,t.title,t.fields_json,a.schema_revision,a.next_field_number,a.purpose,a.title_field_id,a.row_policy,a.author_json,a.v1_layout,c.archived_at IS NOT NULL,m.next_option_number FROM managed_tables t JOIN managed_table_attributes a ON a.number=t.number JOIN object_refs r ON r.number=t.number AND r.kind='table' JOIN crews c ON c.id=r.crew_id JOIN managed_table_v3_meta m ON m.number=t.number WHERE t.number=?1",[n],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,bool>(10)?,r.get::<_,bool>(11)?,r.get::<_,i64>(12)?))).optional()?;
    raw.map(
        |(
            crew,
            source,
            title,
            fields,
            rev,
            next,
            purpose,
            title_field,
            policy,
            author,
            layout,
            crew_frozen,
            next_option,
        )| {
            let fields = values::decode_fields(&fields)?;
            if !objects::source(crate::object_refs::ObjectKind::Table, &source)
                || !v2::text(&title, TITLE_SCALARS)
                || rev <= 0
                || purpose
                    .as_ref()
                    .is_some_and(|p| !v2::text(p, PURPOSE_SCALARS))
                || !values::fields_valid(&fields, &title_field, next, next_option)
            {
                return Err(bad());
            }
            let life = lifecycle(c, n, false)?;
            let mut frozen = Vec::new();
            if crew_frozen {
                frozen.push(FrozenReason::CrewArchived {})
            }
            if life.archived_at.is_some() {
                frozen.push(FrozenReason::TableArchived {})
            }
            let v2_readable = fields.iter().all(|f| values::to_v2_field(f).is_some());
            let v1_readable =
                v2_readable && fields.iter().all(|f| !f.hidden && !values::graph_field(f));
            Ok(Table {
                id: format!("T{n}"),
                crew_id: crew,
                schema_revision: rev.to_string(),
                title,
                purpose,
                title_field_id: title_field,
                row_write_policy: serde_json::from_value(serde_json::Value::String(policy))?,
                author: serde_json::from_str(&author)?,
                fields,
                next_field_number: next.to_string(),
                next_option_number: next_option.to_string(),
                lifecycle: life,
                frozen_reasons: frozen,
                v1_readable,
                v1_writable: v1_readable && layout && rev == 1,
                v2_readable,
                v2_writable: v2_readable,
            })
        },
    )
    .transpose()
}
pub fn legacy_table(t: &Table) -> Result<old::Table> {
    if !t.v2_readable {
        return Err(bad());
    }
    Ok(old::Table {
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
            .map(|f| values::to_v2_field(f).ok_or_else(bad))
            .collect::<Result<_>>()?,
        next_field_number: t.next_field_number.clone(),
        frozen: !t.frozen_reasons.is_empty(),
        v1_readable: t.v1_readable,
        v1_writable: t.v1_writable,
    })
}
/// One current logical projection for search while it also serves pre32 Store
/// fixtures. This does not insert metadata, migrate or change old authorities.
pub fn logical_legacy(t: old::Table) -> Table {
    Table {
        id: t.id,
        crew_id: t.crew_id,
        schema_revision: t.schema_revision,
        title: t.title,
        purpose: t.purpose,
        title_field_id: t.title_field_id,
        row_write_policy: t.row_write_policy,
        author: t.author,
        fields: t.fields.into_iter().map(values::from_v2_field).collect(),
        next_field_number: t.next_field_number,
        next_option_number: "1".into(),
        lifecycle: empty_lifecycle(),
        frozen_reasons: if t.frozen {
            vec![FrozenReason::CrewArchived {}]
        } else {
            Vec::new()
        },
        v1_readable: t.v1_readable,
        v1_writable: t.v1_writable,
        v2_readable: true,
        v2_writable: true,
    }
}
pub struct NewTable<'a> {
    pub crew: &'a str,
    pub title: &'a str,
    pub purpose: &'a str,
    pub fields: &'a [Field],
    pub title_field: &'a str,
    pub next_option: i64,
    pub policy: old::RowWritePolicy,
    pub author: &'a old::Author,
    pub now: &'a str,
}
pub fn create(c: &Connection, r: NewTable<'_>) -> Result<Table> {
    objects::transaction(c)?;
    let next_field = i64::try_from(r.fields.len())
        .map_err(|_| bad())?
        .checked_add(1)
        .ok_or_else(bad)?;
    if !values::fields_valid(r.fields, r.title_field, next_field, r.next_option) {
        return Err(bad());
    }
    let object = objects::allocate(c, crate::object_refs::ObjectKind::Table, r.crew, r.now)?;
    c.execute(
        "INSERT INTO managed_tables VALUES(?1,?2,?3)",
        params![object.number, r.title, serde_json::to_string(r.fields)?],
    )?;
    c.execute(
        "INSERT INTO managed_table_attributes VALUES(?1,1,?2,?3,?4,?5,?6,0)",
        params![
            object.number,
            next_field,
            r.purpose,
            r.title_field,
            serde_json::to_value(r.policy)?.as_str().ok_or_else(bad)?,
            serde_json::to_string(r.author)?
        ],
    )?;
    c.execute(
        "UPDATE managed_table_v3_meta SET next_option_number=?2 WHERE number=?1",
        params![object.number, r.next_option],
    )?;
    crate::managed_field_allocator::remember_floor(c, next_field)?;
    remember_option_floor(c, r.next_option)?;
    table(c, object.number)?.ok_or_else(bad)
}
pub fn save_table(c: &Connection, t: &Table, expected: i64) -> Result<()> {
    objects::transaction(c)?;
    let n = v2::number(&t.id, 'T')?;
    let next = v2::revision(&t.next_field_number)?;
    let option = v2::revision(&t.next_option_number)?;
    if !values::fields_valid(&t.fields, &t.title_field_id, next, option) {
        return Err(bad());
    }
    if c.execute("UPDATE managed_table_attributes SET schema_revision=?3,next_field_number=?4,purpose=?5,title_field_id=?6,v1_layout=0 WHERE number=?1 AND schema_revision=?2",params![n,expected,v2::revision(&t.schema_revision)?,next,t.purpose,t.title_field_id])?!=1{return Err(bad())}
    c.execute(
        "UPDATE managed_tables SET title=?2,fields_json=?3 WHERE number=?1",
        params![n, t.title, serde_json::to_string(&t.fields)?],
    )?;
    c.execute(
        "UPDATE managed_table_v3_meta SET next_option_number=?2 WHERE number=?1",
        params![n, option],
    )?;
    crate::managed_field_allocator::remember_floor(c, next)?;
    remember_option_floor(c, option)?;
    Ok(())
}
/// The only recorded exception to current-schema storage tags: actual numeric
/// widening, sealed to its immutable schema event. This is not inferred on read.
pub fn record_numeric_codec(
    c: &Connection,
    table: i64,
    field: &str,
    kind: &str,
    event: &str,
) -> Result<()> {
    objects::transaction(c)?;
    c.execute(
        "INSERT INTO managed_table_scalar_codecs VALUES(?1,?2,?3,?4)",
        params![table, field, kind, event],
    )?;
    Ok(())
}
pub fn stored_scalars(c: &Connection, n: i64) -> Result<Vec<ScalarValue>> {
    let raw: String = c.query_row(
        "SELECT values_json FROM managed_rows WHERE number=?1",
        [n],
        |r| r.get(0),
    )?;
    if raw.len() > ROW_SCALARS_ENCODED_BYTES {
        return Err(bad());
    }
    // Selection storage contains only its stable issued eN; expand with the
    // actual owning row/table/field, never a caller-provided OptionRef scope.
    let mut cells: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
    let table: i64 = c.query_row(
        "SELECT table_number FROM managed_rows WHERE number=?1",
        [n],
        |r| r.get(0),
    )?;
    for cell in &mut cells {
        let field = cell
            .get("fieldId")
            .and_then(|v| v.as_str())
            .ok_or_else(bad)?
            .to_owned();
        if cell.pointer("/value/type").and_then(|v| v.as_str()) == Some("singleSelect") {
            let id = cell
                .pointer("/value/value")
                .and_then(|v| v.as_str())
                .ok_or_else(bad)?
                .to_owned();
            cell["value"]["value"] = serde_json::to_value(OptionRef {
                table_id: format!("T{table}"),
                field_id: field,
                option_id: id,
            })?;
        }
    }
    let cells: Vec<ScalarValue> = serde_json::from_value(serde_json::Value::Array(cells))?;
    if cells
        .iter()
        .map(|v| &v.field_id)
        .collect::<HashSet<_>>()
        .len()
        != cells.len()
    {
        return Err(bad());
    }
    Ok(cells)
}
pub fn logical_scalars(c: &Connection, t: &Table, n: i64) -> Result<Vec<ScalarValue>> {
    let codecs = {
        let mut q = c.prepare(
            "SELECT field_id,original_kind FROM managed_table_scalar_codecs WHERE table_number=?1",
        )?;
        q.query_map([v2::number(&t.id, 'T')?], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()?
    };
    let result = stored_scalars(c, n)?
        .into_iter()
        .map(|mut v| {
            let f = t
                .fields
                .iter()
                .find(|f| f.id == v.field_id)
                .ok_or_else(bad)?;
            let codec = codecs.get(&v.field_id).map(String::as_str);
            if codec.is_some()
                && !matches!(f.kind, FieldKind::Text { .. } | FieldKind::LongText { .. })
            {
                return Err(bad());
            }
            v.value = values::logical_cell(v.value, codec)?;
            if !values::scalar_valid(&t.id, f, &v.value, false) {
                return Err(bad());
            }
            Ok(v)
        })
        .collect::<Result<Vec<_>>>()?;
    if t.fields
        .iter()
        .filter(|f| !f.nullable && !values::graph_field(f))
        .any(|f| !result.iter().any(|v| v.field_id == f.id))
    {
        return Err(bad());
    }
    Ok(result)
}
pub fn scalar_display(t: &Table, field: &str, value: &Cell) -> Result<Option<String>> {
    Ok(match value {
        Cell::Null => None,
        // Keep existing target/search labels; the typed value and immutable
        // history codec separately preserve the exact IEEE representation.
        Cell::Real(value) => Some(value.to_string()),
        Cell::SingleSelect(v) => {
            let f = t.fields.iter().find(|f| f.id == field).ok_or_else(bad)?;
            let FieldKind::SingleSelect { options } = &f.kind else {
                return Err(bad());
            };
            Some(
                options
                    .iter()
                    .find(|o| o.id == v.option_id)
                    .ok_or_else(bad)?
                    .label
                    .clone(),
            )
        }
        _ => Some(values::scalar_text(value)?.1),
    })
}
pub fn row_title(r: &RowBase, fallback: &str) -> Result<String> {
    r.logical
        .iter()
        .find(|v| v.field_id == r.table.title_field_id)
        .map(|v| scalar_display(&r.table, &v.field_id, &v.value))
        .transpose()
        .map(|v| v.flatten().unwrap_or_else(|| fallback.into()))
}
pub fn scalar_storage(cells: &[ScalarValue]) -> Result<String> {
    let mut raw = serde_json::to_value(cells)?;
    for (typed, item) in cells.iter().zip(raw.as_array_mut().ok_or_else(bad)?) {
        if let Cell::SingleSelect(v) = &typed.value {
            item["value"]["value"] = serde_json::Value::String(v.option_id.clone())
        }
    }
    let s = serde_json::to_string(&raw)?;
    if s.len() > ROW_SCALARS_ENCODED_BYTES {
        return Err(bad());
    }
    Ok(s)
}
pub struct RowBase {
    pub table: Table,
    pub revision: String,
    pub author: old::Author,
    pub stored: Vec<ScalarValue>,
    pub logical: Vec<ScalarValue>,
    pub lifecycle: Lifecycle,
}
pub fn row_base(c: &Connection, n: i64) -> Result<Option<RowBase>> {
    let raw=c.query_row("SELECT r.crew_id,r.source_id,v.table_number,v.revision,a.author_json FROM managed_rows v JOIN object_refs r ON r.number=v.number AND r.kind='custom_row' JOIN managed_row_authors a ON a.number=v.number WHERE v.number=?1",[n],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?))).optional()?;
    raw.map(|(crew, source, tn, rev, author)| {
        let t = table(c, tn)?.ok_or_else(bad)?;
        if crew != t.crew_id
            || !objects::source(crate::object_refs::ObjectKind::CustomRow, &source)
            || rev <= 0
        {
            return Err(bad());
        }
        Ok(RowBase {
            stored: stored_scalars(c, n)?,
            logical: logical_scalars(c, &t, n)?,
            table: t,
            revision: rev.to_string(),
            author: serde_json::from_str(&author)?,
            lifecycle: lifecycle(c, n, true)?,
        })
    })
    .transpose()
}
pub fn add(
    c: &Connection,
    t: &Table,
    author: &old::Author,
    cells: &[ScalarValue],
    now: &str,
) -> Result<i64> {
    objects::transaction(c)?;
    let n = objects::allocate(
        c,
        crate::object_refs::ObjectKind::CustomRow,
        &t.crew_id,
        now,
    )?
    .number;
    c.execute(
        "INSERT INTO managed_rows VALUES(?1,?2,1,?3)",
        params![n, v2::number(&t.id, 'T')?, scalar_storage(cells)?],
    )?;
    c.execute(
        "INSERT INTO managed_row_authors VALUES(?1,?2)",
        params![n, serde_json::to_string(author)?],
    )?;
    Ok(n)
}
pub fn save_row(c: &Connection, n: i64, expected: i64, cells: &[ScalarValue]) -> Result<()> {
    objects::transaction(c)?;
    let next = expected.checked_add(1).ok_or_else(bad)?;
    if c.execute(
        "UPDATE managed_rows SET revision=?3,values_json=?4 WHERE number=?1 AND revision=?2",
        params![n, expected, next, scalar_storage(cells)?],
    )? != 1
    {
        return Err(bad());
    }
    Ok(())
}
/// Filtering happens before LIMIT, so archived or unsupported rows cannot eat
/// a public page or create a hidden-count continuation oracle.
pub fn numbers(
    c: &Connection,
    table: Option<i64>,
    after: i64,
    limit: u16,
    include_archived: bool,
) -> Result<Vec<i64>> {
    let sql = if table.is_some() {
        "SELECT r.number FROM managed_rows r JOIN managed_row_v3_meta m ON m.number=r.number WHERE r.table_number=?1 AND r.number>?2 AND (?4 OR m.archived_at IS NULL) ORDER BY r.number LIMIT ?3"
    } else {
        "SELECT t.number FROM managed_tables t JOIN managed_table_v3_meta m ON m.number=t.number WHERE t.number>?2 AND (?4 OR m.archived_at IS NULL) ORDER BY t.number LIMIT ?3"
    };
    let mut q = c.prepare(sql)?;
    Ok(
        q.query_map(params![table, after, limit, include_archived], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?,
    )
}

const OPTION_FLOOR: &str = "managedOptions.restoreFloor.v1";
const OPTION_WITNESS: &str = ".managed-options-restore-floor.v1";
pub fn option_allocator_floor(c: &Connection) -> Result<i64> {
    let carry = crate::meta_get(c, OPTION_FLOOR)?
        .map(|s| {
            s.parse::<i64>()
                .ok()
                .filter(|n| *n >= 0 && n.to_string() == s)
                .ok_or_else(bad)
        })
        .transpose()?
        .unwrap_or(0);
    if !enabled(c)? {
        return Ok(carry);
    }
    if c.query_row("SELECT EXISTS(SELECT 1 FROM managed_table_v3_meta WHERE typeof(next_option_number)!='integer' OR next_option_number<=0)",[],|r|r.get::<_,bool>(0))?{return Err(bad())}
    let live: i64 = c.query_row(
        "SELECT COALESCE(MAX(next_option_number),0) FROM managed_table_v3_meta",
        [],
        |r| r.get(0),
    )?;
    Ok(carry.max(live))
}
pub fn remember_option_floor(c: &Connection, next: i64) -> Result<()> {
    objects::transaction(c)?;
    if next <= 0 {
        return Err(bad());
    }
    let carry = crate::meta_get(c, OPTION_FLOOR)?
        .map(|s| {
            s.parse::<i64>()
                .ok()
                .filter(|n| *n >= 0 && n.to_string() == s)
                .ok_or_else(bad)
        })
        .transpose()?
        .unwrap_or(0);
    if next > carry {
        crate::meta_set(c, OPTION_FLOOR, &next.to_string())?
    }
    Ok(())
}
/// Backup integration calls this only on its private restore stage. Existing
/// eN remain unchanged; only the next unissued value is raised, never reused.
pub fn preserve_option_restore_floor(c: &Connection, floor: i64) -> Result<()> {
    objects::transaction(c)?;
    if floor < 0 {
        return Err(bad());
    }
    let floor = option_allocator_floor(c)?.max(floor);
    if floor > 0 {
        remember_option_floor(c, floor)?;
        if enabled(c)? {
            c.execute("UPDATE managed_table_v3_meta SET next_option_number=?1 WHERE next_option_number<?1",[floor])?;
        }
    }
    Ok(())
}
pub fn option_witness_floor(dir: &std::path::Path) -> Result<i64> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let f = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(OPTION_WITNESS))
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let m = f.metadata()?;
    if !m.is_file()
        || m.nlink() != 1
        || m.mode() & 0o7777 != 0o600
        || m.uid() != unsafe { libc::geteuid() }
        || m.len() > 20
    {
        return Err(bad());
    }
    let mut raw = String::new();
    f.take(21).read_to_string(&mut raw)?;
    let raw = raw.strip_suffix('\n').unwrap_or(&raw);
    raw.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0 && n.to_string() == raw)
        .ok_or_else(bad)
}
pub fn persist_option_witness(dir: &std::path::Path, floor: i64) -> Result<()> {
    if floor < 0 {
        return Err(bad());
    }
    if floor > option_witness_floor(dir)? {
        crate::fsutil::atomic_write(
            &dir.join(OPTION_WITNESS),
            format!("{floor}\n").as_bytes(),
            0o600,
        )?
    }
    Ok(())
}

pub fn definition_candidates(
    c: &Connection,
    crew: &str,
    after: i64,
    limit: u16,
) -> Result<Vec<NearCandidate>> {
    let mut q=c.prepare("SELECT t.number,a.schema_revision,t.title,a.purpose,m.archived_at IS NOT NULL FROM managed_tables t JOIN object_refs r ON r.number=t.number AND r.kind='table' JOIN managed_table_attributes a ON a.number=t.number JOIN managed_table_v3_meta m ON m.number=t.number WHERE r.crew_id=?1 AND t.number>?2 ORDER BY t.number LIMIT ?3")?;
    Ok(q.query_map(params![crew, after, limit], |r| {
        Ok(NearCandidate {
            table_id: format!("T{}", r.get::<_, i64>(0)?),
            schema_revision: r.get::<_, i64>(1)?.to_string(),
            title: r.get(2)?,
            purpose: r.get(3)?,
            archived: r.get(4)?,
        })
    })?
    .collect::<rusqlite::Result<_>>()?)
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionProof {
    pub scope: crewd_proto::managed_objects::Scope,
    pub principal: String,
    pub crew_id: String,
    pub namespace_revision: i64,
    pub excluding_table_id: Option<String>,
    pub query_digest: String,
    pub after: i64,
    pub exact: bool,
    pub candidates: Vec<DefinitionScore>,
    pub more: bool,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionScore {
    pub table_id: String,
    pub title_intersection: usize,
    pub title_sum: usize,
    pub purpose_intersection: usize,
    pub purpose_sum: usize,
}
/// A deterministic, documented near-name predicate. No Unicode normalization,
/// provider or user-supplied summaries participate in namespace decisions.
pub fn similarity(a: &str, b: &str) -> (usize, usize) {
    fn grams(s: &str) -> HashSet<(char, char)> {
        let chars=s.to_ascii_lowercase().chars().filter(|c| !" \t\r\n-_/ :,. ;!?()[]{}\u{ff1a}\u{ff0c}\u{3002}\u{ff01}\u{ff1f}\u{ff08}\u{ff09}\u{3010}\u{3011}".contains(*c)).collect::<Vec<_>>();
        chars.windows(2).map(|v| (v[0], v[1])).collect()
    }
    let a = grams(a);
    let b = grams(b);
    if a.is_empty() || b.is_empty() {
        (0, 1)
    } else {
        (a.intersection(&b).count(), a.len() + b.len())
    }
}
pub fn near(a: &str, b: &str, purpose: bool) -> bool {
    let (intersection, sum) = similarity(a, b);
    intersection > 0 && (if purpose { 3 } else { 6 }) * intersection >= sum
}
pub struct AuditInput<'a> {
    pub event: &'a str,
    pub source: i64,
    pub action: HistoryAction,
    pub actor: &'a crewd_proto::ResourceActor,
    pub at: &'a str,
    pub reason: Option<&'a str>,
    pub fields: &'a [String],
    pub original: Option<&'a str>,
    pub result: Option<&'a str>,
    pub legacy: bool,
}
pub fn record_audit(c: &Connection, r: AuditInput<'_>) -> Result<()> {
    objects::transaction(c)?;
    if !crewd_proto::valid_remote_uuid(r.event)
        || r.at.is_empty()
        || r.reason.is_some_and(|s| !v2::text(s, REASON_SCALARS))
    {
        return Err(bad());
    }
    c.execute("INSERT INTO managed_table_audit(event_id,source_number,action,actor_json,recorded_at,reason,changed_fields_json,original_revision,result_revision,legacy_details) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![r.event,r.source,serde_json::to_value(r.action)?.as_str().ok_or_else(bad)?,serde_json::to_string(r.actor)?,r.at,r.reason,serde_json::to_string(r.fields)?,r.original,r.result,r.legacy])?;
    if let Some(reason) = r.reason {
        record_audit_value(
            c,
            r.event,
            &HistoryPath::Reason {},
            HistoryValueType::Reason,
            reason,
        )?;
    }
    Ok(())
}
pub fn record_audit_value(
    c: &Connection,
    event: &str,
    path: &HistoryPath,
    kind: HistoryValueType,
    body: &str,
) -> Result<()> {
    objects::transaction(c)?;
    c.execute(
        "INSERT INTO managed_table_audit_values VALUES(?1,?2,?3,?4)",
        params![
            event,
            serde_json::to_string(path)?,
            body,
            serde_json::to_value(kind)?.as_str().ok_or_else(bad)?
        ],
    )?;
    Ok(())
}
pub fn audit_value(
    c: &Connection,
    source: i64,
    event: &str,
    path: &HistoryPath,
) -> Result<Option<(HistoryValueType, String)>> {
    c.query_row("SELECT v.value_type,v.body FROM managed_table_audit_values v JOIN managed_table_audit a ON a.event_id=v.event_id WHERE a.source_number=?1 AND a.event_id=?2 AND v.path=?3",params![source,event,serde_json::to_string(path)?],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?.map(|(kind,body)|Ok((serde_json::from_value(serde_json::Value::String(kind))?,body))).transpose()
}
/// Headers alone: no complete scalar values or historical private targets are
/// parsed while selecting a bounded history page.
pub fn audit_page(
    c: &Connection,
    source: i64,
    before: Option<i64>,
    limit: u16,
) -> Result<Vec<(i64, HistorySummary)>> {
    let mut q=c.prepare("SELECT ordinal,event_id,action,actor_json,recorded_at,substr(reason,1,280),reason IS NOT NULL,changed_fields_json,original_revision,result_revision,legacy_details FROM managed_table_audit WHERE source_number=?1 AND (?2 IS NULL OR ordinal<?2) ORDER BY ordinal DESC LIMIT ?3")?;
    let rows = q
        .query_map(params![source, before, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, bool>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
                r.get::<_, bool>(10)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(
            |(
                ordinal,
                event,
                action,
                actor,
                at,
                reason,
                reason_available,
                fields,
                original,
                result,
                legacy,
            )| {
                Ok((
                    ordinal,
                    HistorySummary {
                        key: HistoryKey::Event { event_id: event },
                        action: serde_json::from_value(serde_json::Value::String(action))?,
                        actor: serde_json::from_str(&actor)?,
                        recorded_at: at,
                        reason_preview: reason,
                        reason_available,
                        changed_field_ids: serde_json::from_str(&fields)?,
                        original_revision: original,
                        result_revision: result,
                        legacy_details_unavailable: legacy,
                        restricted_details: false,
                    },
                ))
            },
        )
        .collect()
}
#[cfg(test)]
#[path = "managed_tables_v3_store_tests.rs"]
mod tests;

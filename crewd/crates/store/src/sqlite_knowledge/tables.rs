//! Business table metadata and explicit primary-key pagination for SQLite.
use super::{Knowledge, bound, invalid, protected};
use crate::Result;
use crewd_proto::team_data::{DataCell, DataColumn, DataSql, DataTablePage, DataTableParams};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

struct Metadata {
    strict: bool,
    columns: Vec<DataColumn>,
    key: Option<Key>,
}

#[derive(Clone, Copy)]
enum KeyKind {
    Integer,
    Text,
}

struct Key {
    column: usize,
    kind: KeyKind,
}

fn metadata(c: &Connection, name: &str) -> Result<Option<Metadata>> {
    if name.is_empty() || protected(name) {
        return Ok(None);
    }
    let row: Option<(String, bool)> = c
        .query_row(
            "SELECT type, strict FROM pragma_table_list(?1) WHERE schema='main'",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((kind, strict)) = row else {
        return Ok(None);
    };
    if kind != "table" && kind != "view" {
        return Ok(None);
    }

    let mut statement = c.prepare(
        "SELECT name,type,\"notnull\",pk,dflt_value,hidden
         FROM pragma_table_xinfo(?1,'main') ORDER BY cid",
    )?;
    let mut columns = statement
        .query_map([name], |r| {
            let hidden: i64 = r.get(5)?;
            Ok(DataColumn {
                name: r.get(0)?,
                declared_type: r.get(1)?,
                not_null: r.get(2)?,
                primary_key_position: r.get(3)?,
                default_sql: r.get(4)?,
                attributes: if hidden == 0 {
                    Value::Null
                } else {
                    json!({"hidden": hidden})
                },
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let key = if kind == "table" {
        stable_key(c, name, strict, &mut columns)?
    } else {
        None
    };
    Ok(Some(Metadata {
        strict,
        columns,
        key,
    }))
}

fn stable_key(
    c: &Connection,
    table: &str,
    strict: bool,
    columns: &mut [DataColumn],
) -> Result<Option<Key>> {
    let mut keys = columns
        .iter_mut()
        .enumerate()
        .filter(|(_, column)| column.primary_key_position > 0);
    let (Some((position, column)), None) = (keys.next(), keys.next()) else {
        return Ok(None);
    };
    if column.primary_key_position != 1 || !column.attributes.is_null() {
        return Ok(None);
    }
    let index: Option<String> = c
        .query_row(
            "SELECT name FROM pragma_index_list(?1,'main') WHERE origin='pk'",
            [table],
            |r| r.get(0),
        )
        .optional()?;
    if index.is_none() && column.declared_type.eq_ignore_ascii_case("INTEGER") {
        // The true INTEGER PRIMARY KEY aliases SQLite's never-null rowid.
        // INTEGER PRIMARY KEY DESC is indexed, so it cannot take this branch.
        column.not_null = true;
        return Ok(Some(Key {
            column: position,
            kind: KeyKind::Integer,
        }));
    }
    let Some(index) = index.filter(|_| strict && column.not_null) else {
        return Ok(None);
    };
    let kind = match column.declared_type.to_ascii_uppercase().as_str() {
        "INTEGER" | "INT" => KeyKind::Integer,
        "TEXT" => KeyKind::Text,
        _ => return Ok(None),
    };
    let indexed: Option<(String, String)> = c
        .query_row(
            "SELECT name,coll FROM pragma_index_xinfo(?1,'main') WHERE \"key\"=1",
            [index],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(indexed
        .filter(|(name, collation)| {
            name == &column.name && collation.eq_ignore_ascii_case("BINARY")
        })
        .map(|_| Key {
            column: position,
            kind,
        }))
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

impl KeyKind {
    fn accepts(self, cell: &DataCell) -> bool {
        match (self, cell) {
            (Self::Integer, DataCell::Integer(value)) => value
                .parse::<i64>()
                .is_ok_and(|parsed| parsed.to_string() == *value),
            (Self::Text, DataCell::Text(value)) => value.len() <= 1024 * 1024,
            _ => false,
        }
    }
}

impl Knowledge {
    pub fn table_schema(&self, name: &str) -> Result<(Vec<DataColumn>, Value)> {
        let Some(data) = metadata(&self.conn, name)? else {
            return Ok((vec![], Value::Null));
        };
        // Triggers can redirect an attempted write. Deferred foreign keys can
        // pass the rolled-back preview and fail only when confirmation commits.
        let native_row_writes = data.strict
            && data.key.is_some()
            && self.conn.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM main.sqlite_schema
                        WHERE type='trigger' AND tbl_name=?1 COLLATE NOCASE)
                    AND NOT EXISTS(SELECT 1 FROM pragma_foreign_key_list(?1,'main'))",
                [name],
                |row| row.get::<_, bool>(0),
            )?;
        Ok((
            data.columns,
            json!({
                "strict": data.strict,
                "stablePrimaryKey": data.key.is_some(),
                "nativeRowWrites": native_row_writes,
            }),
        ))
    }

    pub fn table_page(&self, p: DataTableParams) -> Result<DataTablePage> {
        self.check(&p.context)?;
        if p.at_revision.is_some() {
            return Err(invalid("historical SQL table reads are not supported"));
        }
        bound(p.limit, 1000)?;
        if p.table.contains('\0') {
            return Err(invalid("invalid business table identifier"));
        }
        let data = metadata(&self.conn, &p.table)?
            .ok_or_else(|| invalid("business table or view is unavailable"))?;
        let table = quote(&p.table);
        let (selection, params) = match (&data.key, p.after_key) {
            (Some(key), after) => {
                let column = quote(&data.columns[key.column].name);
                let (filter, params) = match after {
                    Some(cells) if cells.len() == 1 && key.kind.accepts(&cells[0]) => {
                        (format!(" WHERE {column} COLLATE BINARY > ?1"), cells)
                    }
                    Some(_) => return Err(invalid("invalid primary key cursor")),
                    None => (String::new(), Vec::new()),
                };
                (
                    format!(
                        "SELECT * FROM main.{table}{filter} ORDER BY {column} COLLATE BINARY ASC"
                    ),
                    params,
                )
            }
            (None, None) => (format!("SELECT * FROM main.{table}"), Vec::new()),
            (None, Some(_)) => return Err(invalid("table has no supported stable primary key")),
        };
        let sql = format!("{selection} LIMIT {}", p.limit + 1);
        let (columns, rows, truncated) = self.query(&DataSql { sql, params }, p.limit)?;
        let mut next_key = None;
        if let Some(key) = &data.key {
            let name = &data.columns[key.column].name;
            let position = columns
                .iter()
                .position(|column| column == name)
                .ok_or_else(|| invalid("primary key column missing from table read"))?;
            if rows
                .iter()
                .any(|row| !row.get(position).is_some_and(|cell| key.kind.accepts(cell)))
            {
                return Err(invalid(
                    "table primary key values do not match the declared schema",
                ));
            }
            if truncated {
                let last = rows
                    .last()
                    .ok_or_else(|| invalid("invalid empty table page"))?;
                next_key = Some(vec![last[position].clone()]);
            }
        }
        Ok(DataTablePage {
            generation: p.context.generation.clone(),
            context: p.context,
            columns,
            rows,
            next_key,
            truncated,
            stable_order: data.key.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_proto::team_data::DataContext;

    fn seeded(sql: &str) -> (tempfile::TempDir, Knowledge) {
        let temp = tempfile::tempdir().unwrap();
        let context = DataContext {
            db_id: "test-core".into(),
            authority_id: "test-core".into(),
            restore_epoch: "initial".into(),
            crew_id: "test-crew".into(),
            namespace: super::super::namespace("test-core", "test-crew"),
            generation: "0".into(),
        };
        let path = std::fs::canonicalize(temp.path())
            .unwrap()
            .join("private/knowledge.sqlite");
        let database = Knowledge::open(&path, context, true).unwrap();
        database.conn.execute_batch(sql).unwrap();
        (temp, database)
    }

    fn page(
        k: &Knowledge,
        table: &str,
        limit: u32,
        after_key: Option<Vec<DataCell>>,
    ) -> DataTableParams {
        DataTableParams {
            context: k.context().unwrap(),
            table: table.into(),
            limit,
            after_key,
            at_revision: None,
        }
    }

    #[test]
    fn exact_integer_keyset_preserves_signed_bounds_and_nullable_values() {
        let (_temp, k) = seeded(
            "CREATE TABLE ledger(amount TEXT, row_key INTEGER PRIMARY KEY, memo TEXT DEFAULT 'default');
             INSERT INTO ledger VALUES ('zero',0,NULL),('maximum',9223372036854775807,'hi'),
                ('minimum',-9223372036854775808,NULL),('negative',-1,NULL);",
        );
        let (columns, attrs) = k.table_schema("ledger").unwrap();
        assert_eq!(
            attrs,
            json!({"strict":false,"stablePrimaryKey":true,"nativeRowWrites":false})
        );
        assert_eq!(columns[1].name, "row_key");
        assert_eq!(columns[1].declared_type, "INTEGER");
        assert!(columns[1].not_null);
        assert_eq!(columns[1].primary_key_position, 1);
        assert_eq!(columns[2].default_sql.as_deref(), Some("'default'"));
        assert!(!columns[2].not_null);
        let first = k.table_page(page(&k, "ledger", 2, None)).unwrap();
        assert!(first.stable_order && first.truncated);
        assert_eq!(first.columns, ["amount", "row_key", "memo"]);
        assert_eq!(
            serde_json::to_value(&first.rows).unwrap(),
            json!([
                [{"type":"text","value":"minimum"},{"type":"integer","value":"-9223372036854775808"},{"type":"null"}],
                [{"type":"text","value":"negative"},{"type":"integer","value":"-1"},{"type":"null"}],
            ])
        );
        let second = k.table_page(page(&k, "ledger", 2, first.next_key)).unwrap();
        assert!(second.stable_order && !second.truncated && second.next_key.is_none());
        assert_eq!(
            serde_json::to_value(&second.rows).unwrap(),
            json!([
                [{"type":"text","value":"zero"},{"type":"integer","value":"0"},{"type":"null"}],
                [{"type":"text","value":"maximum"},{"type":"integer","value":"9223372036854775807"},{"type":"text","value":"hi"}],
            ])
        );
        for cursor in [
            vec![],
            vec![DataCell::Integer("-0".into())],
            vec![DataCell::Integer("+1".into())],
            vec![DataCell::Integer("9223372036854775808".into())],
            vec![DataCell::Text("-1".into())],
            vec![DataCell::Null],
            vec![DataCell::Integer("0".into()), DataCell::Integer("1".into())],
        ] {
            assert!(k.table_page(page(&k, "ledger", 1, Some(cursor))).is_err());
        }
        let mut stale = page(&k, "ledger", 1, None);
        stale.context.generation = "1".into();
        assert!(k.table_page(stale).is_err());
    }

    #[test]
    fn exact_text_keyset_quotes_real_names_and_does_not_guess_legacy_order() {
        let (_temp, k) = seeded(
            r#"CREATE TABLE "账""簿"("主""键" TEXT NOT NULL PRIMARY KEY, memo TEXT,
                                     generated TEXT GENERATED ALWAYS AS (memo) VIRTUAL) STRICT;
              INSERT INTO "账""簿" VALUES('界',NULL),('A','x'),('a','y');
              CREATE TABLE nullable(id TEXT PRIMARY KEY, memo TEXT);
              INSERT INTO nullable VALUES(NULL,'one'),(NULL,'two');
              CREATE TABLE composite(a INTEGER,b TEXT,PRIMARY KEY(a,b)) STRICT;
              CREATE TABLE descending(id INTEGER PRIMARY KEY DESC);
              CREATE TABLE folded(id TEXT NOT NULL PRIMARY KEY COLLATE NOCASE) STRICT;
              CREATE VIEW legacy_view AS SELECT id,memo FROM nullable;
              CREATE TABLE guarded(id INTEGER PRIMARY KEY, note TEXT) STRICT;
              CREATE TRIGGER guarded_add BEFORE INSERT ON guarded
              BEGIN INSERT INTO nullable VALUES(NULL, 'unexpected'); SELECT RAISE(IGNORE); END;
              CREATE TABLE related(id INTEGER PRIMARY KEY, parent INTEGER REFERENCES guarded(id)
                DEFERRABLE INITIALLY DEFERRED) STRICT;"#,
        );
        let (columns, attrs) = k.table_schema("账\"簿").unwrap();
        assert_eq!(
            attrs,
            json!({"strict":true,"stablePrimaryKey":true,"nativeRowWrites":true})
        );
        for table in ["guarded", "related"] {
            let (_, attributes) = k.table_schema(table).unwrap();
            assert_eq!(attributes["nativeRowWrites"], false);
            assert!(k.table_page(page(&k, table, 1, None)).unwrap().stable_order);
        }
        assert_eq!(columns[0].name, "主\"键");
        assert_eq!(columns[2].attributes, json!({"hidden":2}));
        let mut next = None;
        for expected in ["A", "a", "界"] {
            let result = k.table_page(page(&k, "账\"簿", 1, next)).unwrap();
            assert!(result.stable_order && result.rows.len() == 1);
            assert_eq!(
                serde_json::to_value(&result.rows[0][0]).unwrap(),
                json!({"type":"text","value":expected})
            );
            next = result.next_key;
            assert_eq!(result.truncated, expected != "界");
        }
        assert!(next.is_none());
        for table in [
            "nullable",
            "composite",
            "descending",
            "folded",
            "legacy_view",
        ] {
            let result = k.table_page(page(&k, table, 1, None)).unwrap();
            assert!(!result.stable_order && result.next_key.is_none(), "{table}");
            assert!(
                k.table_page(page(&k, table, 1, Some(vec![DataCell::Text("a".into())])))
                    .is_err()
            );
        }
        for forbidden in [
            "_PC_META",
            "sqlite_schema",
            "pragma_table_info",
            "账\"簿\"; SELECT 1",
        ] {
            assert!(
                k.table_page(page(&k, forbidden, 1, None)).is_err(),
                "{forbidden}"
            );
        }
        let mut historical = page(&k, "账\"簿", 1, None);
        historical.at_revision = Some("0".into());
        assert!(k.table_page(historical).is_err());
    }
}

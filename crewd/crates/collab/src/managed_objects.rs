//! Organization-readable T/O, with current owner crew authorization for writes.
//! The caller must first authenticate the Human principal or exact attached
//! session, and revalidate the connection and grants in this transaction.
use crate::{
    CollabError, Result, Sender, Tx, crews,
    refusal::{Code, Refusal},
    resource_acl, sessions,
};
use crewd_proto::managed_objects::{
    Change, Lookup, Object, Receipt, RowPage, Scope, Submit, TablePage,
};
use crewd_store::{StoreError, managed_objects as db};
use rusqlite::Connection;

const PAGE_LIMIT: u16 = 16;

#[derive(Clone, Copy)]
pub enum Actor<'a> {
    /// Trusted scope key, derived from the current qualified local or paired
    /// Human connection. Never accept this value from RPC parameters.
    Human(&'a str),
    Session(&'a str),
}
impl Actor<'_> {
    fn principal(self, c: &Connection) -> Result<String> {
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

    fn owner(self, c: &Connection, crew: &str, write: bool) -> Result<String> {
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
    actor.principal(c)?;
    if id.starts_with('T') {
        let number = object_number(id, 'T', "id")?;
        Ok(Object::Table {
            table: db::table(c, number)?.ok_or_else(|| missing(id))?,
        })
    } else {
        let number = object_number(id, 'O', "id")?;
        Ok(Object::Row {
            row: db::row(c, number)?.ok_or_else(|| missing(id))?,
        })
    }
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
    let principal = actor.owner(c, crew, false)?;
    let current = scope(c)?;
    if origin.db_id != current.db_id {
        return Err(forbidden());
    }
    let recorded = db::receipt(c, crew, &principal, request)?.map(|(_, r)| r);
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
    let principal = actor.owner(c, &request.crew_id, true)?;
    if scope(c)? != request.scope {
        return Err(conflict(&request.request_id));
    }
    let digest: [u8; 32] = resource_acl::digest(&request)?
        .try_into()
        .map_err(|_| StoreError::Invalid("managed object digest length invalid".into()))?;
    if let Some((saved, receipt)) =
        db::receipt(c, &request.crew_id, &principal, &request.request_id)?
    {
        return if saved == digest && receipt.scope == request.scope {
            Ok(receipt)
        } else {
            Err(conflict(&request.request_id))
        };
    }
    let now = tx.now_ts();
    let object = match request.change {
        Change::CreateTable { title, fields } => {
            if !db::definition_valid(&title, &fields) {
                return Err(invalid("fields"));
            }
            Object::Table {
                table: db::create_table(c, &request.crew_id, &title, &fields, &now)?,
            }
        }
        Change::AddRow { table_id, values } => {
            let table_number = object_number(&table_id, 'T', "tableId")?;
            let table = db::table(c, table_number)?.ok_or_else(|| missing(&table_id))?;
            if table.crew_id != request.crew_id {
                return Err(forbidden());
            }
            if !db::values_valid(&table.fields, &values)? {
                return Err(invalid("values"));
            }
            Object::Row {
                row: db::add_row(c, &request.crew_id, table_number, &values, &now)?,
            }
        }
        Change::EditRow {
            row_id,
            expected_revision,
            values,
        } => {
            let row_number = object_number(&row_id, 'O', "rowId")?;
            let expected = number(&expected_revision, "expectedRevision")?;
            let row = db::row(c, row_number)?.ok_or_else(|| missing(&row_id))?;
            if row.crew_id != request.crew_id {
                return Err(forbidden());
            }
            if row.revision != expected_revision {
                return Err(CollabError::Conflict(Refusal::new(Code::ManagedRowChanged)));
            }
            let table_number = object_number(&row.table_id, 'T', "tableId")?;
            let table = db::table(c, table_number)?.ok_or_else(|| missing(&row.table_id))?;
            if !db::values_valid(&table.fields, &values)? {
                return Err(invalid("values"));
            }
            Object::Row {
                row: db::edit_row(c, row_number, expected, &values)?,
            }
        }
    };
    let receipt = Receipt {
        scope: request.scope,
        crew_id: request.crew_id,
        request_id: request.request_id,
        recorded_at: now,
        object,
    };
    db::record(c, &principal, &digest, &receipt)?;
    Ok(receipt)
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
    fn organization_read_uses_exact_t_o_but_only_owner_writes_and_revisions_cas() {
        let mut s = store();
        let create = request("crew-a", definition());
        let made = ok(&mut s, |tx| submit(tx, AUTHOR, create.clone()));
        let t = table(&made);
        assert_eq!(get(s.conn(), OTHER, &t.id).unwrap(), made.object);
        assert_eq!(tables(s.conn(), OTHER, None).unwrap().items, [t.clone()]);
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
            [first.clone()]
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
        let s = Store::open_data_dir(dir.path()).unwrap();
        s.conn()
            .execute_batch(
                "DROP TABLE managed_object_receipts;
             DROP TABLE managed_rows;
             DROP TABLE managed_tables;
             PRAGMA user_version=22;
             INSERT INTO crews(id,title,created_at,updated_at) VALUES('crew-a','A','t','t');",
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
        drop(s);
        let mut s = Store::open_data_dir(dir.path()).unwrap();
        let version: i64 = s
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 23);
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

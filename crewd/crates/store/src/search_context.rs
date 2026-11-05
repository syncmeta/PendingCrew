//! Connection-local source-change context. Never stored in SQLite or accepted
//! from protocol input. The only SQL surface is two innocuous, read-only getters.
use crate::{Result, StoreError};
use rusqlite::{Connection, functions::FunctionFlags};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
struct Current {
    wall: String,
    span: String,
}
#[derive(Clone)]
pub(crate) struct Context(Arc<Mutex<Option<Current>>>);
fn lock(c: &Mutex<Option<Current>>) -> MutexGuard<'_, Option<Current>> {
    c.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub struct Guard(Arc<Mutex<Option<Current>>>);
impl Drop for Guard {
    fn drop(&mut self) {
        *lock(&self.0) = None;
    }
}
impl Context {
    pub(crate) fn register(c: &Connection) -> Result<Self> {
        let current = Arc::new(Mutex::new(None::<Current>));
        let wall = current.clone();
        let span = current.clone();
        let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS;
        c.create_scalar_function(
            "pcw_search_wall",
            0,
            flags,
            move |_| -> rusqlite::Result<Option<String>> {
                Ok(lock(&wall).as_ref().map(|c| c.wall.clone()))
            },
        )?;
        c.create_scalar_function(
            "pcw_search_span",
            0,
            flags,
            move |_| -> rusqlite::Result<Option<String>> {
                Ok(lock(&span).as_ref().map(|c| c.span.clone()))
            },
        )?;
        Ok(Self(current))
    }
    pub(crate) fn enter(&self, wall_ms: i64) -> Result<Guard> {
        let mut current = lock(&self.0);
        if current.is_some() {
            return Err(StoreError::Invalid(
                "organization search context is already active".into(),
            ));
        }
        *current = Some(Current {
            wall: crate::ts_from_ms(wall_ms),
            span: uuid::Uuid::new_v4().to_string(),
        });
        Ok(Guard(self.0.clone()))
    }
}
/// Plain managed migration/backup connections get NULL getters. A Store installs
/// and retains its own handle before migration; never replace that live handle.
pub(crate) fn ensure_registered(c: &Connection) -> Result<()> {
    let count:i64=c.query_row("SELECT count(*) FROM pragma_function_list WHERE name IN('pcw_search_wall','pcw_search_span') AND narg=0",[],|r|r.get(0))?;
    match count {
        0 => {
            Context::register(c)?;
            Ok(())
        }
        2 => Ok(()),
        _ => Err(StoreError::Invalid(
            "incomplete organization search getters".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn values(c: &Connection) -> (Option<String>, Option<String>) {
        c.query_row("SELECT pcw_search_wall(),pcw_search_span()", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap()
    }
    #[test]
    fn error_rollback_and_panic_clear_without_physical_sql_changes() {
        let c = Connection::open_in_memory().unwrap();
        let context = Context::register(&c).unwrap();
        let before = c.total_changes();
        {
            let _guard = context.enter(1_800_000_000_000).unwrap();
            let tx = c.unchecked_transaction().unwrap();
            let v = values(&tx);
            assert!(v.0.is_some() && v.1.is_some());
            assert!(context.enter(1).is_err());
        }
        assert_eq!(values(&c), (None, None));
        assert_eq!(c.total_changes(), before);
        let result: Result<()> = (|| {
            let _guard = context.enter(1_800_000_000_001)?;
            Err(StoreError::Invalid("synthetic failure".into()))
        })();
        assert!(result.is_err());
        assert_eq!(values(&c), (None, None));
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = context.enter(1_800_000_000_002).unwrap();
            panic!("synthetic source callback failure")
        }));
        assert!(failed.is_err());
        assert_eq!(values(&c), (None, None));
        assert_eq!(c.total_changes(), before);
    }
    #[test]
    fn trigger_getters_cannot_leak_after_unwinding_a_real_transaction() {
        let c = Connection::open_in_memory().unwrap();
        let context = Context::register(&c).unwrap();
        c.execute_batch("PRAGMA trusted_schema=OFF; CREATE TABLE input(id INTEGER); CREATE TABLE observed(wall TEXT,span TEXT); CREATE TRIGGER observe AFTER INSERT ON input BEGIN INSERT INTO observed VALUES(pcw_search_wall(),pcw_search_span()); END;").unwrap();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = context.enter(1_800_000_000_003).unwrap();
            let tx = c.unchecked_transaction().unwrap();
            tx.execute("INSERT INTO input VALUES(1)", []).unwrap();
            let count: i64 = tx
                .query_row(
                    "SELECT count(*) FROM observed WHERE wall IS NOT NULL AND span IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
            panic!("synthetic source callback failure");
        }));
        assert!(failed.is_err());
        let count: i64 = c
            .query_row("SELECT count(*) FROM observed", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(values(&c), (None, None));
        c.execute("INSERT INTO input VALUES(2)", []).unwrap();
        let count: i64 = c
            .query_row(
                "SELECT count(*) FROM observed WHERE wall IS NULL AND span IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert!(
            c.query_row("SELECT pcw_search_wall('forged')", [], |r| r
                .get::<_, String>(0))
                .is_err()
        );
    }
    #[test]
    fn begin_failure_clears_context_and_preserves_the_existing_transaction() {
        let c = Connection::open_in_memory().unwrap();
        let context = Context::register(&c).unwrap();
        let outer = c.unchecked_transaction().unwrap();
        let before = c.total_changes();
        let result: Result<()> = (|| {
            let _guard = context.enter(1_800_000_000_004)?;
            let _second = c.unchecked_transaction()?;
            Ok(())
        })();
        assert!(result.is_err());
        assert_eq!(values(&c), (None, None));
        assert_eq!(c.total_changes(), before);
        assert!(!c.is_autocommit());
        outer.commit().unwrap();
    }
    #[test]
    fn connections_never_share_context_and_functions_are_not_deterministic() {
        let a = Connection::open_in_memory().unwrap();
        let b = Connection::open_in_memory().unwrap();
        let context = Context::register(&a).unwrap();
        ensure_registered(&b).unwrap();
        let _guard = context.enter(1_800_000_000_000).unwrap();
        assert!(values(&a).0.is_some());
        assert_eq!(values(&b), (None, None));
        let flags: i64 = a
            .query_row(
                "SELECT flags FROM pragma_function_list WHERE name='pcw_search_wall'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(flags & FunctionFlags::SQLITE_DETERMINISTIC.bits() as i64, 0);
        assert_ne!(flags & FunctionFlags::SQLITE_INNOCUOUS.bits() as i64, 0);
    }
}

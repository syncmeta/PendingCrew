//! Restore authority for deterministic message facts, independent of model configuration.
use crate::{Result, StoreError, judgment_state, personal_drafts};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ALIAS: &str = "message.evidence.legacyAuthority.v1";
const PREFIX: &str = "restore-v1:";
const LIMIT: usize = 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    database: String,
    restore_stamp: String,
    epoch: String,
}

pub struct Authority {
    database: Option<String>,
    epoch: Option<String>,
    legacy: Option<String>,
}

impl Authority {
    pub fn load(c: &Connection) -> Result<Self> {
        let (database, stamp) = identity(c)?;
        let legacy = if let (Some(db), Some(stamp)) = (&database, &stamp) {
            bounded_text(c, ALIAS, LIMIT)?
                .and_then(|text| serde_json::from_str::<Legacy>(&text).ok())
                .filter(|alias| {
                    alias.database == *db
                        && alias.restore_stamp == *stamp
                        && legacy_epoch(&alias.epoch)
                })
                .map(|alias| alias.epoch)
        } else {
            None
        };
        Ok(Self {
            database,
            epoch: stamp.map(|stamp| format!("{PREFIX}{stamp}")),
            legacy,
        })
    }

    pub fn current(&self) -> Option<(&str, &str)> {
        Some((self.database.as_deref()?, self.epoch.as_deref()?))
    }

    pub fn matches(&self, database: &str, epoch: &str) -> bool {
        self.current().is_some_and(|(db, current)| {
            db == database && (current == epoch || self.legacy.as_deref() == Some(epoch))
        })
    }

    pub fn fingerprint(&self) -> (Option<&str>, Option<&str>, Option<&str>) {
        (
            self.database.as_deref(),
            self.epoch.as_deref(),
            self.legacy.as_deref(),
        )
    }
}

// meta is not STRICT. Fetch bounded bytes from actual TEXT and validate UTF-8
// ourselves so a malformed value is not confused with a SQLite read failure.
fn bounded_text(c: &Connection, key: &str, limit: usize) -> Result<Option<String>> {
    let bytes = c
        .query_row(
            "SELECT substr(CAST(value AS BLOB),1,?2) FROM meta WHERE key=?1 AND typeof(value)='text'",
            params![key, limit as i64 + 1],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    Ok(bytes
        .filter(|bytes| bytes.len() <= limit)
        .and_then(|bytes| String::from_utf8(bytes).ok()))
}

fn identity(c: &Connection) -> Result<(Option<String>, Option<String>)> {
    let db = bounded_text(c, "dbId", 256)?
        .filter(|db| !db.is_empty() && !db.chars().any(char::is_control));
    let stamp = bounded_text(c, personal_drafts::RESTORE_KEY, 36)?
        .filter(|stamp| crewd_proto::valid_remote_uuid(stamp));
    Ok((db, stamp))
}

fn uuid(raw: &str) -> bool {
    Uuid::parse_str(raw).is_ok_and(|id| id.to_string() == raw)
}

fn revision(raw: &str) -> bool {
    raw.parse::<i64>()
        .is_ok_and(|n| n > 0 && n.to_string() == raw)
}

fn legacy_epoch(raw: &str) -> bool {
    if uuid(raw) {
        return true;
    }
    let mut fields = raw.split(':');
    matches!(
        (fields.next(), fields.next(), fields.next(), fields.next(), fields.next()),
        (Some("unwitnessed"), Some(epoch), Some(observed), Some(current), None)
            if (epoch.is_empty() || uuid(epoch))
                && (observed.is_empty() || revision(observed))
                && revision(current)
    )
}

/// Called once from startup, before serving settings RPCs. Never replace an
/// old/malformed alias. A restored database cannot reuse an alias captured
/// under the stamp of the original database.
pub fn capture_legacy(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "message evidence startup must be transactional".into(),
        ));
    }
    let exists = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)",
        [ALIAS],
        |r| r.get::<_, bool>(0),
    )?;
    if exists {
        return Ok(());
    }
    let (Some(database), Some(restore_stamp)) = identity(c)? else {
        return Ok(());
    };
    let epoch = judgment_state::epoch(c)?;
    if !legacy_epoch(&epoch) {
        return Ok(());
    }
    let value = serde_json::to_string(&Legacy {
        database,
        restore_stamp,
        epoch,
    })?;
    c.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2)",
        params![ALIAS, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Store, backup, meta_get, meta_set, remote};

    #[test]
    fn exact_legacy_alias_survives_settings_but_not_official_restore() {
        let root = tempfile::tempdir().unwrap();
        let snapshot = root.path().join("before-restore.db");
        let without_alias = root.path().join("before-upgrade.db");
        let mut s = Store::open_data_dir(root.path()).unwrap();
        let initial = judgment_state::epoch(s.conn()).unwrap();
        backup::vacuum_into(s.conn(), &without_alias).unwrap();
        s.with_tx(|tx| capture_legacy(tx)).unwrap();
        let authority = Authority::load(s.conn()).unwrap();
        let (db, tagged) = authority.current().unwrap();
        assert!(authority.matches(db, &initial));
        assert!(authority.matches(db, tagged));
        assert!(!authority.matches("another-db", &initial));
        let captured = meta_get(s.conn(), ALIAS).unwrap();
        s.with_tx(|c| {
            assert!(remote::configure(c, 1, false, "127.0.0.1", 7423)?);
            Ok(())
        })
        .unwrap();
        let changed = judgment_state::epoch(s.conn()).unwrap();
        assert_ne!(changed, initial);
        s.with_tx(|tx| judgment_state::prepare_configuration(tx))
            .unwrap();
        assert_ne!(judgment_state::epoch(s.conn()).unwrap(), changed);
        s.with_tx(|tx| capture_legacy(tx)).unwrap();
        let after_settings = Authority::load(s.conn()).unwrap();
        assert_eq!(after_settings.fingerprint(), authority.fingerprint());
        assert!(!after_settings.matches(db, &changed));
        assert_eq!(meta_get(s.conn(), ALIAS).unwrap(), captured);
        backup::vacuum_into(s.conn(), &snapshot).unwrap();
        drop(s);

        backup::restore(root.path(), &snapshot).unwrap();
        let mut reopened = Store::open_data_dir(root.path()).unwrap();
        reopened.with_tx(|tx| capture_legacy(tx)).unwrap();
        let after_restore = Authority::load(reopened.conn()).unwrap();
        assert_ne!(after_restore.current().unwrap().1, tagged);
        assert!(!after_restore.matches(db, tagged));
        assert!(!after_restore.matches(db, &initial));
        assert_eq!(meta_get(reopened.conn(), ALIAS).unwrap(), captured);
        drop(reopened);

        // A restored older snapshot has no alias; startup may capture only the
        // new, officially rotated judgment epoch, never the snapshot's old cut.
        backup::restore(root.path(), &without_alias).unwrap();
        let mut old_backup = Store::open_data_dir(root.path()).unwrap();
        assert!(meta_get(old_backup.conn(), ALIAS).unwrap().is_none());
        old_backup.with_tx(|tx| capture_legacy(tx)).unwrap();
        let after_older_restore = Authority::load(old_backup.conn()).unwrap();
        assert!(!after_older_restore.matches(db, tagged));
        assert!(!after_older_restore.matches(db, &initial));
    }

    #[test]
    fn malformed_or_missing_witness_never_qualifies_legacy_evidence() {
        let mut s = Store::open_in_memory().unwrap();
        let epoch = judgment_state::epoch(s.conn()).unwrap();
        let scope = Authority::load(s.conn()).unwrap();
        let (db, tagged) = scope.current().unwrap();
        assert!(!scope.matches(db, &epoch));
        meta_set(s.conn(), ALIAS, "malformed").unwrap();
        s.with_tx(|tx| capture_legacy(tx)).unwrap();
        let malformed = Authority::load(s.conn()).unwrap();
        assert_eq!(
            meta_get(s.conn(), ALIAS).unwrap().as_deref(),
            Some("malformed")
        );
        assert!(!malformed.matches(db, &epoch));
        assert!(malformed.matches(db, tagged));
        s.conn()
            .execute(
                "DELETE FROM meta WHERE key='personalDraft.restoreStamp.v1'",
                [],
            )
            .unwrap();
        let missing = Authority::load(s.conn()).unwrap();
        assert!(missing.current().is_none());
        assert!(!missing.matches(db, tagged));
        assert!(!missing.matches(db, &epoch));
    }

    #[test]
    fn wrong_storage_class_bad_utf8_and_oversize_are_not_authority() {
        let mut s = Store::open_in_memory().unwrap();
        let legacy = judgment_state::epoch(s.conn()).unwrap();
        s.with_tx(|tx| capture_legacy(tx)).unwrap();
        let current = Authority::load(s.conn()).unwrap();
        let (db, tagged) = current.current().unwrap();
        let stamp = personal_drafts::epoch(s.conn()).unwrap();
        let alias = meta_get(s.conn(), ALIAS).unwrap().unwrap();

        for raw in ["X'FF'", "CAST(X'FF' AS TEXT)", "CAST(value AS BLOB)"] {
            meta_set(s.conn(), ALIAS, &alias).unwrap();
            s.conn()
                .execute(
                    &format!("UPDATE meta SET value={raw} WHERE key=?1"),
                    [ALIAS],
                )
                .unwrap();
            let malformed = Authority::load(s.conn()).unwrap();
            assert!(malformed.matches(db, tagged));
            assert!(!malformed.matches(db, &legacy));

            meta_set(s.conn(), ALIAS, &alias).unwrap();
            meta_set(s.conn(), personal_drafts::RESTORE_KEY, &stamp).unwrap();
            s.conn()
                .execute(
                    &format!("UPDATE meta SET value={raw} WHERE key=?1"),
                    [personal_drafts::RESTORE_KEY],
                )
                .unwrap();
            let malformed = Authority::load(s.conn()).unwrap();
            assert!(malformed.current().is_none());
            assert!(!malformed.matches(db, tagged));
            assert!(!malformed.matches(db, &legacy));
            meta_set(s.conn(), personal_drafts::RESTORE_KEY, &stamp).unwrap();
        }

        meta_set(s.conn(), ALIAS, &format!("{alias}{}", " ".repeat(LIMIT))).unwrap();
        assert!(!Authority::load(s.conn()).unwrap().matches(db, &legacy));
        meta_set(s.conn(), personal_drafts::RESTORE_KEY, &(stamp + " ")).unwrap();
        assert!(Authority::load(s.conn()).unwrap().current().is_none());

        let missing_table = Connection::open_in_memory().unwrap();
        assert!(matches!(
            Authority::load(&missing_table),
            Err(StoreError::Sql(_))
        ));
    }
}

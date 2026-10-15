//! Optional judgment context, including a witness understood by older Core14.
//! Official old restore/import increments remote_configuration.revision. A normal
//! new configure can advance that witness only while the old cut still matches.
use crate::{Result, meta_get, meta_set};
use rusqlite::Connection;
const KEY: &str = "judgment.restoreEpoch.v1";
const OBSERVED: &str = "judgment.remoteRevision.v1";
pub fn format_available(c: &Connection) -> Result<bool> {
    Ok(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? >= 15
            && meta_get(c, "judgment.format.v1")?.as_deref() == Some("1"),
    )
}
fn current(c: &Connection) -> Result<String> {
    Ok(crate::remote::configuration(c)?.revision)
}
fn qualified(c: &Connection, revision: &str) -> Result<Option<String>> {
    let epoch = meta_get(c, KEY)?;
    let observed = meta_get(c, OBSERVED)?;
    Ok(epoch.filter(|e| uuid::Uuid::parse_str(e).is_ok() && observed.as_deref() == Some(revision)))
}
pub fn epoch(c: &Connection) -> Result<String> {
    let revision = current(c)?;
    if let Some(epoch) = qualified(c, &revision)? {
        return Ok(epoch);
    }
    // An unmatched older restore is observable, not an unobserved raw rollback.
    // New facts can retain attribution under this cut, but old bindings mismatch.
    Ok(format!(
        "unwitnessed:{}:{}:{}",
        meta_get(c, KEY)?.unwrap_or_default(),
        meta_get(c, OBSERVED)?.unwrap_or_default(),
        revision
    ))
}
pub fn restored(c: &Connection) -> Result<()> {
    meta_set(c, KEY, &uuid::Uuid::new_v4().to_string())?;
    meta_set(c, OBSERVED, &current(c)?)
}
/// Called only by a new explicit Human settings transaction, after keep checks.
pub fn prepare_configuration(c: &Connection) -> Result<()> {
    if qualified(c, &current(c)?)?.is_none() {
        restored(c)?;
    }
    Ok(())
}
/// The remote CAS and this witness update must share their original transaction.
/// Missing/corrupt optional state does not reject an otherwise legal remote CAS.
pub fn remote_reconfigured(c: &Connection, previous: i64) -> Result<()> {
    if c.is_autocommit() {
        return Ok(());
    }
    if qualified(c, &previous.to_string())?.is_none() {
        return Ok(());
    }
    let config = meta_get(c, "judgment.config.v1")?;
    if !config.as_deref().is_some_and(|raw| {
        serde_json::from_str::<crewd_proto::judgments::JudgmentSettings>(raw).is_ok()
    }) {
        return Ok(());
    }
    meta_set(c, OBSERVED, &current(c)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn config(c: &Connection) {
        meta_set(
            c,
            "judgment.config.v1",
            r#"{"revision":"fake","config":null}"#,
        )
        .unwrap();
    }
    #[test]
    fn normal_remote_cas_preserves_epoch_but_old_restore_cannot_be_laundered() {
        let mut s = crate::Store::open_in_memory().unwrap();
        s.with_tx(|c| {
            config(c);
            prepare_configuration(c)?;
            Ok(())
        })
        .unwrap();
        let original = epoch(s.conn()).unwrap();
        s.with_tx(|c| {
            assert!(crate::remote::configure(c, 1, false, "127.0.0.1", 7423)?);
            Ok(())
        })
        .unwrap();
        assert_eq!(original, epoch(s.conn()).unwrap());
        // This is the actual pre-Jev old Core14 hook; no new epoch hook runs.
        s.with_tx(|c| crate::remote::invalidate_for_work_replacement(c, "work_restore", "fake"))
            .unwrap();
        assert_ne!(original, epoch(s.conn()).unwrap());
        s.with_tx(|c| {
            assert!(crate::remote::configure(c, 3, false, "127.0.0.1", 7424)?);
            Ok(())
        })
        .unwrap();
        assert_ne!(original, epoch(s.conn()).unwrap());
        assert_eq!(meta_get(s.conn(), OBSERVED).unwrap().as_deref(), Some("2"));
        s.with_tx(|c| prepare_configuration(c)).unwrap();
        assert!(!epoch(s.conn()).unwrap().starts_with("unwitnessed:"));
        assert_ne!(original, epoch(s.conn()).unwrap());
    }
    #[test]
    fn corrupt_or_absent_optional_state_does_not_block_remote_or_gain_a_witness() {
        let mut s = crate::Store::open_in_memory().unwrap();
        s.with_tx(|c| {
            assert!(crate::remote::configure(c, 1, false, "127.0.0.1", 7423)?);
            Ok(())
        })
        .unwrap();
        assert!(meta_get(s.conn(), OBSERVED).unwrap().is_none());
        s.with_tx(|c| {
            prepare_configuration(c)?;
            meta_set(c, "judgment.config.v1", "malformed")?;
            assert!(crate::remote::configure(c, 2, false, "127.0.0.1", 7424)?);
            Ok(())
        })
        .unwrap();
        assert!(epoch(s.conn()).unwrap().starts_with("unwitnessed:"));
    }
}

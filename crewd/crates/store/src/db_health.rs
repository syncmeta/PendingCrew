//! S4 independent DB fault history. Typed owning adapters, no source-chain/string guessing.
use crate::{StorageKind, StoreError};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbFaultStage {
    Enqueue,
    Receive,
    Job,
    Finish,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbFaultKind {
    Closed,
    JobPanicked,
    Sql,
    Storage(StorageKind),
    Integrity,
    Unclassified,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbFault {
    pub stage: DbFaultStage,
    pub kind: DbFaultKind,
    pub at_ms: i64,
    pub sequence: u64,
}

/// Implement at the error-owning crate. Transparent wrappers must delegate explicitly.
/// A business refusal is None. Never inspect formatted strings or arbitrary Error::source chains.
pub trait DbFaultSource {
    fn db_fault_kind(&self) -> Option<DbFaultKind>;
}
impl DbFaultSource for StoreError {
    fn db_fault_kind(&self) -> Option<DbFaultKind> {
        match self {
            StoreError::DbClosed => Some(DbFaultKind::Closed),
            StoreError::DbJobPanicked => Some(DbFaultKind::JobPanicked),
            StoreError::Integrity { .. } => Some(DbFaultKind::Integrity),
            StoreError::Sql(e) => e.db_fault_kind(),
            StoreError::Snapshot { source, .. } | StoreError::MigrationRefused { source, .. } => {
                source.db_fault_kind()
            }
            StoreError::Storage { .. } | StoreError::Io(_) | StoreError::NoSpace { .. } => {
                self.storage_kind().map(DbFaultKind::Storage)
            }
            _ => None,
        }
    }
}
impl DbFaultSource for rusqlite::Error {
    fn db_fault_kind(&self) -> Option<DbFaultKind> {
        Some(StorageKind::of(self).map_or(DbFaultKind::Sql, DbFaultKind::Storage))
    }
}
impl DbFaultSource for std::io::Error {
    fn db_fault_kind(&self) -> Option<DbFaultKind> {
        let kind = match self.raw_os_error() {
            Some(libc::ENOSPC) | Some(libc::EDQUOT) => StorageKind::DiskFull,
            Some(libc::EROFS) => StorageKind::ReadOnly,
            Some(libc::EACCES) | Some(libc::EPERM) => StorageKind::CantOpen,
            _ => StorageKind::Io,
        };
        Some(DbFaultKind::Storage(kind))
    }
}

#[derive(Clone, Debug, Default)]
pub struct DbHealth(Arc<Mutex<Option<DbFault>>>);
impl DbHealth {
    pub fn snapshot(&self) -> Option<DbFault> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub(crate) fn record(&self, stage: DbFaultStage, kind: DbFaultKind) {
        let mut last = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let sequence = last.as_ref().map_or(1, |v| v.sequence.saturating_add(1));
        *last = Some(DbFault {
            stage,
            kind,
            at_ms: chrono::Utc::now().timestamp_millis(),
            sequence,
        });
    }
    pub(crate) fn observe<E: DbFaultSource>(&self, stage: DbFaultStage, error: &E) {
        if let Some(kind) = error.db_fault_kind() {
            self.record(stage, kind);
        }
    }
    pub(crate) fn finish_error(&self, error: &StoreError) {
        // Actual finish failed even if its typed cause is a non-storage Invalid (e.g. busy checkpoint).
        // Exactly one record preserves Finish and does not mistake an ordinary request refusal for outage.
        self.record(
            DbFaultStage::Finish,
            error.db_fault_kind().unwrap_or(DbFaultKind::Unclassified),
        );
    }
}

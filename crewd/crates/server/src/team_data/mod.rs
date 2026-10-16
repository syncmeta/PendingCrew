//! Authenticated SQLite knowledge controller in the crewd process.
// Historical worker.rs is intentionally not linked.
pub mod compile;
// Projection is implemented by the SQLite runtime.
// ACL cut is implemented by the SQLite runtime.
// Historical import is not part of the fresh-format candidate.

#[path = "sqlite_runtime.rs"]
pub(crate) mod runtime;

/// Only implemented SQLite methods are advertised.
pub use crewd_proto::team_data_api::METHODS;

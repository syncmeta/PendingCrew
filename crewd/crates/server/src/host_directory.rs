//! Owner-only, read-only browsing of this service user's filesystem.
use crewd_proto::RpcError;
pub(crate) use crewd_proto::host::DirectoryParams as Params;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};
fn error(e: std::io::Error) -> RpcError {
    let (code, refusal) = match e.kind() {
        std::io::ErrorKind::NotFound => (RpcError::NOT_FOUND, "host.directory_not_found"),
        std::io::ErrorKind::PermissionDenied => {
            (RpcError::FORBIDDEN, "host.directory_permission_denied")
        }
        std::io::ErrorKind::NotADirectory => {
            (RpcError::INVALID_PARAMS, "host.directory_not_directory")
        }
        _ => (RpcError::INTERNAL, "host.directory_unavailable"),
    };
    RpcError::new(code, refusal).with_refusal(refusal)
}
fn invalid() -> RpcError {
    RpcError::new(RpcError::INVALID_PARAMS, "Invalid directory parameters")
}
pub(crate) fn list(p: Params) -> Result<Value, RpcError> {
    let limit = p.limit.unwrap_or(100);
    if limit == 0
        || limit > 500
        || p.after
            .as_ref()
            .is_some_and(|v| v.len() > 4096 || v.contains('/'))
    {
        return Err(invalid());
    }
    let path = PathBuf::from(match p.path {
        Some(s) => s,
        None => std::env::var("HOME").map_err(|_| invalid())?,
    });
    if !path.is_absolute() {
        return Err(invalid());
    }
    let path = path.canonicalize().map_err(error)?;
    let canonical = path.to_str().ok_or_else(invalid)?;
    // Retain only one page plus lookahead even for very large directories.
    let mut rows = BTreeMap::new();
    for entry in std::fs::read_dir(&path).map_err(error)? {
        let entry = entry.map_err(error)?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if p.after.as_ref().is_some_and(|after| name <= *after) {
            continue;
        }
        let target = match entry.path().canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !target.is_dir() {
            continue;
        }
        let Some(target) = target.to_str() else {
            continue;
        };
        rows.insert(name, target.to_owned());
        if rows.len() > limit + 1 {
            rows.pop_last();
        }
    }
    let more = rows.len() > limit;
    if more {
        rows.pop_last();
    }
    let next = if more {
        rows.last_key_value().map(|(n, _)| n.clone())
    } else {
        None
    };
    Ok(
        json!({"path":canonical,"parentPath":path.parent().and_then(|p|p.to_str()),"entries":rows.into_iter().map(|(name,path)|json!({"name":name,"path":path})).collect::<Vec<_>>(),"nextAfter":next}),
    )
}

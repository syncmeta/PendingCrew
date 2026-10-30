//! Versioned, bounded per-session control metadata in the existing Core store.
//! This is neither provider history nor execution permission. Unknown receipts
//! remain in the original command ledger, never scheduled from this value.
use crate::{Result, StoreError};
use crewd_proto::codex::*;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    pub version: u8,
    #[serde(default)]
    pub fork_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_history: Option<CodexNativeHistoryState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_previous: Option<CodexNativeHistoryState>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restore_unconfirmed: bool,

    #[serde(default)]
    pub title_attempted: bool,
    #[serde(default)]
    pub settings_attempted: bool,
    #[serde(default)]
    pub name_revision: u64,
    #[serde(default)]
    pub title_intent_observation: u64,
    pub desired_settings: Option<CodexSettingsPatch>,
    pub settings_status: Option<String>,
    pub settings: Option<CodexObservedSettings>,
    pub settings_observed_at: Option<String>,
    pub observed_binding: Option<String>,
    pub observed_thread: Option<String>,
    pub title: CodexTitleState,
    pub intent_request: Option<String>,
    pub title_request: Option<String>,
}
pub fn get(c: &Connection, session: &str) -> Result<State> {
    let key = format!("codex.controls.v1/{session}");
    let Some(raw) = crate::meta_get(c, &key)? else {
        return Ok(State {
            version: 1,
            ..Default::default()
        });
    };
    if raw.len() > 65536 {
        return Err(StoreError::Invalid("Codex metadata exceeds bound".into()));
    }
    let state: State = serde_json::from_str(&raw)
        .map_err(|_| StoreError::Invalid("invalid Codex control metadata".into()))?;
    if !matches!(state.version, 1 | 2) {
        return Err(StoreError::Invalid("unknown Codex metadata version".into()));
    }
    Ok(state)
}
pub fn put(c: &Connection, session: &str, state: &State) -> Result<()> {
    if crate::sessions::executor(c, session)? != "codex" {
        return Err(StoreError::Invalid(
            "Codex control metadata requires Codex source session".into(),
        ));
    }
    let raw = serde_json::to_string(state)
        .map_err(|_| StoreError::Invalid("Codex metadata serialization".into()))?;
    if !matches!(state.version, 1 | 2)
        || (state.native_history.is_some() && state.version != 2)
        || raw.len() > 65536
    {
        return Err(StoreError::Invalid(
            "invalid Codex metadata size/version".into(),
        ));
    }
    crate::meta_set(c, &format!("codex.controls.v1/{session}"), &raw)
}
pub fn initial(
    c: &Connection,
    session: &str,
    patch: Option<CodexSettingsPatch>,
    human_title: bool,
) -> Result<()> {
    let row =
        crate::sessions::get(c, session)?.ok_or_else(|| StoreError::NotFound(session.into()))?;
    if row.executor != "codex" {
        if patch.is_some() {
            return Err(StoreError::Invalid(
                "codexOptions require Codex executor".into(),
            ));
        }
        return Ok(());
    }
    let title = row
        .title
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            row.brief
                .as_deref()
                .and_then(|b| b.lines().find(|l| !l.trim().is_empty()))
                .map(|l| l.chars().take(120).collect())
        });
    if let Some(p) = &patch {
        p.validate().map_err(|s| StoreError::Invalid(s.into()))?;
    }
    let state = State {
        version: 1,
        desired_settings: patch,
        settings_status: None,
        title: CodexTitleState {
            desired: title,
            source: if human_title {
                CodexTitleSource::Human
            } else {
                CodexTitleSource::Task
            },
            ..Default::default()
        },
        ..Default::default()
    };
    put(c, session, &state)
}
pub fn observe(
    c: &Connection,
    session: &str,
    settings: Option<serde_json::Value>,
    title: Option<CodexTitleState>,
    accepted: Option<CodexSettingsPatch>,
    binding: &str,
) -> Result<()> {
    let mut state = get(c, session)?;
    let row =
        crate::sessions::get(c, session)?.ok_or_else(|| StoreError::NotFound(session.into()))?;
    if accepted
        .as_ref()
        .is_some_and(|p| state.desired_settings.as_ref() == Some(p))
    {
        state.settings_status = Some("accepted".into());
    }
    if let Some(s) = settings {
        // Internal producer already normalized the native spelling.
        state.settings = Some(
            serde_json::from_value(s)
                .map_err(|_| StoreError::Invalid("invalid observed Codex settings".into()))?,
        );
        state.settings_observed_at = Some(crate::now_ts());
    }
    if let Some(t) = title {
        if let Some(native) = t.native {
            state.title.native = Some(native);
            state.name_revision = state
                .name_revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Invalid("Codex name observation exhausted".into()))?;
        }
        state.title.status = if state.title.native == state.title.desired
            && matches!(t.status, CodexTitleStatus::Synced)
        {
            CodexTitleStatus::Synced
        } else {
            t.status
        };
        if matches!(state.title.status, CodexTitleStatus::Synced)
            && state.title.native != state.title.desired
        {
            state.title.status = CodexTitleStatus::Unknown;
        }
        state.title.detail = t.detail;
    }
    state.observed_binding = Some(binding.into());
    state.observed_thread = row.agent_session_id;
    put(c, session, &state)
}
pub fn configure_intent(
    c: &Connection,
    session: &str,
    patch: &CodexSettingsPatch,
    request: &str,
) -> Result<()> {
    patch
        .validate()
        .map_err(|s| StoreError::Invalid(s.into()))?;
    let mut state = get(c, session)?;
    let mut merged = serde_json::to_value(state.desired_settings.clone().unwrap_or_default())
        .map_err(|_| StoreError::Invalid("settings intent serialization".into()))?;
    let next = serde_json::to_value(patch)
        .map_err(|_| StoreError::Invalid("settings patch serialization".into()))?;
    if patch.permissions.is_some() {
        merged.as_object_mut().unwrap().remove("sandbox");
    }
    if patch.sandbox.is_some() {
        merged.as_object_mut().unwrap().remove("permissions");
    }
    merged
        .as_object_mut()
        .unwrap()
        .extend(next.as_object().unwrap().clone());
    state.desired_settings = Some(
        serde_json::from_value(merged).map_err(|_| StoreError::Invalid("settings merge".into()))?,
    );
    state.settings_status = Some("pending".into());
    state.settings_attempted = true;
    state.intent_request = Some(request.into());
    put(c, session, &state)
}

/// A v2 row is a per-session old-writer barrier, not new execution permission.
pub fn native_ready(
    c: &Connection,
    session: &str,
    action: Option<CodexHistoryAction>,
) -> Result<bool> {
    let state = get(c, session)?;
    if state.fork_pending || state.restore_unconfirmed {
        return Ok(false);
    }
    Ok(match state.native_history.as_ref().map(|h| h.status) {
        Some(CodexNativeHistoryStatus::Unknown | CodexNativeHistoryStatus::Deleted) => false,
        Some(CodexNativeHistoryStatus::Archived) => matches!(
            action,
            Some(CodexHistoryAction::Unarchive | CodexHistoryAction::Delete)
        ),
        _ => true,
    })
}
pub fn history_intent(
    c: &Connection,
    session: &str,
    request: &str,
    action: CodexHistoryAction,
) -> Result<()> {
    if c.is_autocommit()
        || !crewd_proto::valid_remote_uuid(request)
        || !native_ready(c, session, Some(action))?
    {
        return Err(StoreError::Invalid(
            "native history requires original transaction/current session authority".into(),
        ));
    }
    let mut state = get(c, session)?;
    state.version = 2;
    state.history_previous = state.native_history.take();
    state.native_history = Some(CodexNativeHistoryState {
        status: CodexNativeHistoryStatus::Unknown,
        request_id: Some(request.into()),
        operation: Some(action),
    });
    put(c, session, &state)
}
pub fn settle_history(
    c: &Connection,
    session: &str,
    request: &str,
    phase: crewd_proto::commands::CommandPhase,
) -> Result<()> {
    use crewd_proto::commands::CommandPhase;
    let mut state = get(c, session)?;
    let Some(current) = state
        .native_history
        .as_ref()
        .filter(|h| h.request_id.as_deref() == Some(request))
    else {
        return Ok(());
    };
    let action = current.operation;
    match phase {
        CommandPhase::Confirmed => {
            let status = match action {
                Some(CodexHistoryAction::Archive) => CodexNativeHistoryStatus::Archived,
                Some(CodexHistoryAction::Delete) => CodexNativeHistoryStatus::Deleted,
                _ => CodexNativeHistoryStatus::Active,
            };
            state.native_history.as_mut().unwrap().status = status;
            state.history_previous = None;
        }
        CommandPhase::Rejected | CommandPhase::Suppressed => {
            state.native_history = state.history_previous.take();
        }
        _ => {}
    }
    put(c, session, &state)
}

/// A restore cannot establish the current native timeline from an old snapshot.
/// Keep original bindings and lifecycle facts, but never synthesize an operation UUID.
pub fn restored(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(StoreError::Invalid(
            "Codex restore requires original transaction".into(),
        ));
    }
    // An old snapshot predating the new lifecycle cannot prove that its native
    // thread was not subsequently reverted/deleted/submitted. Every restored
    // Codex row is historical; fresh explicit rows remain independent.
    let mut stmt = c.prepare("SELECT id FROM sessions WHERE executor='codex'")?;
    let sessions = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    for session in sessions {
        let mut state = get(c, &session)?;
        state.version = 2;
        state.restore_unconfirmed = true;
        put(c, &session, &state)?;
    }
    Ok(())
}

//! Work tools delegate state and permissions to the same collab transactions as the client.
use super::{Ctx, crew_arg, parse_args};
use crate::agent::port;
use crewd_proto::agent::CallResult;
use crewd_proto::{
    WorkCreateParams, WorkEvidenceParam, WorkLevel, WorkUpdateParams, WorkUpdateStatus,
};
use serde::Deserialize;
use serde_json::Value;
fn receipt<T: serde::Serialize>(cx: &Ctx<'_>, v: port::PortResult<T>) -> CallResult {
    match v {
        Ok(v) => match serde_json::to_string(&v) {
            Ok(result) => cx.ok("r.work_result", &[("result", &result)]),
            Err(e) => cx.fail(port::PortError::Internal(e.to_string())),
        },
        Err(e) => cx.fail(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mine {
    after: Option<String>,
    limit: Option<u16>,
    #[serde(default)]
    include_finished: bool,
    #[serde(default)]
    include_removed: bool,
}
pub async fn mine(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Mine = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::work_mine(
            cx.st,
            &cx.me.session_id,
            a.after,
            a.limit,
            a.include_finished,
            a.include_removed,
        )
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    id: String,
}
pub async fn get(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Get = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    receipt(cx, port::work_get(cx.st, &a.id).await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    title: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    level: WorkLevel,
    #[serde(default)]
    acceptance: String,
    owner: Option<String>,
    crew: Option<String>,
    parent: Option<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    sources: Vec<String>,
    next_check_at: Option<String>,
}
pub async fn create(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Create = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let crew = match a.crew {
        Some(arg) => match crew_arg(cx, &arg).await {
            Ok(c) => c.id,
            Err(e) => return e,
        },
        None => cx.me.crew_id.clone(),
    };
    receipt(
        cx,
        port::work_create(
            cx.st,
            &cx.me.session_id,
            WorkCreateParams {
                crew_id: crew,
                title: a.title,
                goal: a.goal,
                level: a.level,
                acceptance: a.acceptance,
                owner: a.owner,
                parent: a.parent,
                depends_on: a.depends_on,
                sources: a.sources,
                next_check_at: a.next_check_at,
            },
        )
        .await
        .map(crewd_proto::WorkSummary::from),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    id: String,
    expected_revision: String,
    status: Option<WorkUpdateStatus>,
    next_check_at: Option<String>,
    evidence: Option<WorkEvidenceParam>,
    note: Option<String>,
}
pub async fn update(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Update = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::work_update(
            cx.st,
            &cx.me.session_id,
            WorkUpdateParams {
                id: a.id,
                expected_revision: a.expected_revision,
                status: a.status,
                next_check_at: a.next_check_at,
                evidence: a.evidence,
                note: a.note,
            },
        )
        .await
        .map(crewd_proto::WorkSummary::from),
    )
}

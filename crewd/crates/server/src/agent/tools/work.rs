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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assign {
    id: String,
    expected_revision: String,
    owner: String,
    note: String,
}
pub async fn assign(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Assign = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::work_assign(
            cx.st,
            &cx.me.session_id,
            crewd_proto::WorkAssignParams {
                id: a.id,
                expected_revision: a.expected_revision,
                owner: a.owner,
                note: a.note,
            },
        )
        .await
        .map(crewd_proto::WorkSummary::from),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    id: String,
    before: Option<String>,
    limit: Option<u16>,
}
pub async fn history(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: History = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::work_history(
            cx.st,
            crewd_proto::WorkHistoryParams {
                id: a.id,
                before: a.before,
                limit: a.limit,
            },
        )
        .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Block {
    id: String,
    expected_revision: String,
    on: Vec<BlockOn>,
    can_continue: Continue,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum BlockOn {
    Work {
        id: String,
    },
    Human {
        id: String,
    },
    NewHuman {
        question: String,
        default: Option<BlockDefault>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockDefault {
    text: String,
    after_minutes: i64,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Continue {
    Nothing,
    Split { items: Vec<Create> },
}
pub async fn block(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Block = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let on =
        a.on.into_iter()
            .map(|on| match on {
                BlockOn::Work { id } => crewd_proto::WorkBlockOn::Work { id },
                BlockOn::Human { id } => crewd_proto::WorkBlockOn::Human { id },
                BlockOn::NewHuman { question, default } => crewd_proto::WorkBlockOn::NewHuman {
                    question,
                    default: default.map(|d| crewd_proto::AskDefault {
                        text: d.text,
                        after_minutes: d.after_minutes,
                    }),
                },
            })
            .collect();
    let can_continue = match a.can_continue {
        Continue::Nothing => crewd_proto::WorkContinuation::Nothing,
        Continue::Split { items } => {
            let mut mapped = Vec::with_capacity(items.len());
            for p in items {
                let crew = match p.crew {
                    Some(arg) => match crew_arg(cx, &arg).await {
                        Ok(c) => c.id,
                        Err(e) => return e,
                    },
                    None => String::new(),
                };
                mapped.push(WorkCreateParams {
                    crew_id: crew,
                    title: p.title,
                    goal: p.goal,
                    level: p.level,
                    acceptance: p.acceptance,
                    owner: p.owner,
                    parent: p.parent,
                    depends_on: p.depends_on,
                    sources: p.sources,
                    next_check_at: p.next_check_at,
                });
            }
            crewd_proto::WorkContinuation::Split { items: mapped }
        }
    };
    receipt(
        cx,
        port::work_block(
            cx.st,
            &cx.me.session_id,
            crewd_proto::WorkBlockParams {
                id: a.id,
                expected_revision: a.expected_revision,
                on,
                can_continue,
            },
        )
        .await
        .map(|reply| {
            let mut item = crewd_proto::WorkSummary::from(reply.item);
            item.note = None;
            serde_json::json!({"item":item,"humanTasks":reply.human_tasks,"split":reply.split})
        }),
    )
}

pub async fn todo_get(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: Get = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    receipt(cx, port::todo_get(cx.st, &a.id).await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferRequest {
    id: String,
    expected_revision: String,
    to: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferResolve {
    id: String,
    expected_revision: String,
}
fn transfer_receipt(cx: &Ctx<'_>, p: port::PortResult<crewd_proto::WorkPlan>) -> CallResult {
    receipt(
        cx,
        p.map(|p| {
            let mut s = crewd_proto::WorkSummary::from(p);
            s.note = None;
            s
        }),
    )
}
pub async fn transfer_request(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TransferRequest = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    transfer_receipt(
        cx,
        port::work_transfer_request(
            cx.st,
            &cx.me.session_id,
            crewd_proto::WorkTransferRequestParams {
                id: a.id,
                expected_revision: a.expected_revision,
                to: a.to,
                note: a.note,
            },
        )
        .await,
    )
}
pub async fn transfer_accept(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TransferResolve = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    transfer_receipt(
        cx,
        port::work_transfer_accept(
            cx.st,
            &cx.me.session_id,
            crewd_proto::WorkTransferResolveParams {
                id: a.id,
                expected_revision: a.expected_revision,
            },
        )
        .await,
    )
}
pub async fn transfer_withdraw(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TransferResolve = match parse_args(cx, v) {
        Ok(a) => a,
        Err(e) => return e,
    };
    transfer_receipt(
        cx,
        port::work_transfer_withdraw(
            cx.st,
            &cx.me.session_id,
            crewd_proto::WorkTransferResolveParams {
                id: a.id,
                expected_revision: a.expected_revision,
            },
        )
        .await,
    )
}

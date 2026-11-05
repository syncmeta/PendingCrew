//! Typed organization-object tools. Their connection and token must be
//! revalidated at the same FIFO cut as every read or write.
use super::{Ctx, parse_args, port};
use crewd_collab::managed_objects::{self as objects, Actor};
use crewd_proto::{
    Empty,
    agent::CallResult,
    managed_objects::{Scope, Submit},
    managed_objects_api::{GetParams, RequestParams, RowsParams, TablesParams},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentScope {
    scope: Scope,
    crew_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    after: Option<String>,
}

fn result(cx: &Ctx<'_>, result: port::PortResult<impl Serialize>) -> CallResult {
    match result {
        Ok(value) => match serde_json::to_string(&value) {
            Ok(value) => CallResult::ok(value),
            Err(error) => cx.fail(port::PortError::Internal(error.to_string())),
        },
        Err(error) => cx.fail(error),
    }
}

pub(super) async fn call(cx: &Ctx<'_>, tool: &str, args: Value) -> CallResult {
    match call_inner(cx, tool, args).await {
        Ok(result) | Err(result) => result,
    }
}

async fn call_inner(cx: &Ctx<'_>, tool: &str, args: Value) -> Result<CallResult, CallResult> {
    let output = match tool {
        "managed_object_scope_get" => {
            let Empty {} = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, |c, by| {
                    Ok(CurrentScope {
                        scope: objects::scope(c)?,
                        crew_id: by.crew_id.clone(),
                    })
                })
                .await,
            )
        }
        "managed_object_get" => {
            let p: GetParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::get(c, Actor::Session(&by.session_id), &p.id)
                })
                .await,
            )
        }
        "managed_tables_list" => {
            let p: TablesParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::tables(c, Actor::Session(&by.session_id), p.after.as_deref())
                })
                .await,
            )
        }
        "managed_rows_list" => {
            let p: RowsParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::rows(
                        c,
                        Actor::Session(&by.session_id),
                        &p.table_id,
                        p.after.as_deref(),
                    )
                })
                .await,
            )
        }
        "managed_decisions_list" => {
            let p: ListParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::decisions(c, Actor::Session(&by.session_id), p.after.as_deref())
                })
                .await,
            )
        }
        "managed_risks_list" => {
            let p: ListParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::risks(c, Actor::Session(&by.session_id), p.after.as_deref())
                })
                .await,
            )
        }
        "managed_lessons_list" => {
            let p: ListParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::lessons(c, Actor::Session(&by.session_id), p.after.as_deref())
                })
                .await,
            )
        }
        "managed_object_submit" => {
            let p: Submit = parse_args(cx, args)?;
            result(cx, port::managed_submit(cx.st, cx.me, p).await)
        }
        "managed_object_request_get" => {
            let p: RequestParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::lookup(
                        c,
                        Actor::Session(&by.session_id),
                        &p.scope,
                        &p.crew_id,
                        &p.request_id,
                    )
                })
                .await,
            )
        }
        _ => cx.err("r.no_handler", &[("name", tool)]),
    };
    Ok(output)
}

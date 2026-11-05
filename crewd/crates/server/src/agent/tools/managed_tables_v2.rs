//! V2 table/link tools use typed FIFO ports; no local ACL or Knowledge path fallback.
use super::{Ctx, parse_args, port};
use crewd_proto::{agent::CallResult, managed_tables_v2 as v2};
use serde::Serialize;
use serde_json::Value;
fn result(cx: &Ctx<'_>, r: port::PortResult<impl Serialize>) -> CallResult {
    match r {
        Ok(value) => match serde_json::to_string(&value) {
            Ok(text) => CallResult::ok(text),
            Err(error) => cx.fail(port::PortError::Internal(error.to_string())),
        },
        Err(error) => cx.fail(error),
    }
}
pub(super) async fn call(cx: &Ctx<'_>, tool: &str, args: Value) -> CallResult {
    match inner(cx, tool, args).await {
        Ok(value) | Err(value) => value,
    }
}
async fn inner(cx: &Ctx<'_>, tool: &str, args: Value) -> Result<CallResult, CallResult> {
    Ok(match tool {
        "managed_table_v2_get" => {
            let p: v2::TableGet = parse_args(cx, args)?;
            result(cx, port::managed_table_v2_get(cx.st, cx.me, p).await)
        }
        "managed_tables_v2_list" => {
            let p: v2::TableList = parse_args(cx, args)?;
            result(cx, port::managed_tables_v2_list(cx.st, cx.me, p).await)
        }
        "managed_row_v2_get" => {
            let p: v2::TableGet = parse_args(cx, args)?;
            result(cx, port::managed_row_v2_get(cx.st, cx.me, p).await)
        }
        "managed_rows_v2_list" => {
            let p: v2::RowList = parse_args(cx, args)?;
            result(cx, port::managed_rows_v2_list(cx.st, cx.me, p).await)
        }
        "managed_table_v2_submit" => {
            let p: v2::Submit = parse_args(cx, args)?;
            result(cx, port::managed_table_v2_submit(cx.st, cx.me, p).await)
        }
        "managed_table_v2_request_get" => {
            let p: v2::RequestGet = parse_args(cx, args)?;
            result(
                cx,
                port::managed_table_v2_request_get(cx.st, cx.me, p).await,
            )
        }
        "managed_link_resolve_target" => {
            let p: v2::ResolveTarget = parse_args(cx, args)?;
            result(cx, port::managed_link_resolve_target(cx.st, cx.me, p).await)
        }
        "managed_links_list" => {
            let p: v2::ListLinks = parse_args(cx, args)?;
            result(cx, port::managed_links_list(cx.st, cx.me, p).await)
        }
        "managed_link_submit" => {
            let p: v2::ManualSubmit = parse_args(cx, args)?;
            result(cx, port::managed_link_submit(cx.st, cx.me, p).await)
        }
        "managed_link_request_get" => {
            let p: v2::RequestGet = parse_args(cx, args)?;
            result(cx, port::managed_link_request_get(cx.st, cx.me, p).await)
        }
        _ => unreachable!("V2 tools are catalog-routed"),
    })
}

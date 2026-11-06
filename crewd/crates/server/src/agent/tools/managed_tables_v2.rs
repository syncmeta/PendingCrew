//! Existing table/link tools use typed FIFO ports; version choice never grants authority.
use super::{Ctx, parse_args, port};
use crewd_proto::{
    agent::CallResult,
    managed_table_tools::{self, ContractVersion},
    managed_tables_v2 as v2, managed_tables_v3 as v3,
};
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
    let args = if managed_table_tools::legacy_fields(tool).is_some() {
        match managed_table_tools::select_version(args) {
            Ok((ContractVersion::V2, args)) => args,
            Ok((ContractVersion::V3, args)) => {
                return match inner_v3(cx, tool, args).await {
                    Ok(value) | Err(value) => value,
                };
            }
            Err(field) => return cx.err("r.bad_args", &[("detail", field)]),
        }
    } else {
        args
    };
    match inner(cx, tool, args).await {
        Ok(value) | Err(value) => value,
    }
}

async fn inner_v3(cx: &Ctx<'_>, tool: &str, args: Value) -> Result<CallResult, CallResult> {
    Ok(match tool {
        "managed_table_v2_get" => {
            let params: v3::ToolGetV3 = parse_args(cx, args)?;
            match params {
                v3::ToolGetV3::Table { id } => result(
                    cx,
                    port::managed_table_v3_get(cx.st, cx.me, v3::Get { id }).await,
                ),
                v3::ToolGetV3::DefineCheck {
                    scope,
                    crew_id,
                    excluding_table_id,
                    title,
                    purpose,
                    cursor,
                } => result(
                    cx,
                    port::managed_table_v3_define_check(
                        cx.st,
                        cx.me,
                        v3::DefineCheck {
                            scope,
                            crew_id,
                            excluding_table_id,
                            title,
                            purpose,
                            cursor,
                        },
                    )
                    .await,
                ),
                v3::ToolGetV3::History { source, cursor } => result(
                    cx,
                    port::managed_table_v3_history(cx.st, cx.me, v3::History { source, cursor })
                        .await,
                ),
                v3::ToolGetV3::HistoryValue {
                    source,
                    key,
                    path,
                    cursor,
                } => result(
                    cx,
                    port::managed_table_v3_history_value(
                        cx.st,
                        cx.me,
                        v3::HistoryValue {
                            source,
                            key,
                            path,
                            cursor,
                        },
                    )
                    .await,
                ),
            }
        }
        "managed_tables_v2_list" => {
            let params: v3::TableList = parse_args(cx, args)?;
            result(cx, port::managed_tables_v3_list(cx.st, cx.me, params).await)
        }
        "managed_row_v2_get" => {
            let params: v3::ToolRowGetV3 = parse_args(cx, args)?;
            match params {
                v3::ToolRowGetV3::Row { id } => result(
                    cx,
                    port::managed_row_v3_get(cx.st, cx.me, v3::Get { id }).await,
                ),
                v3::ToolRowGetV3::Links {
                    row_id,
                    field_id,
                    expected_schema_revision,
                    expected_row_revision,
                } => result(
                    cx,
                    port::managed_row_v3_links(
                        cx.st,
                        cx.me,
                        v3::RowLinks {
                            row_id,
                            field_id,
                            expected_schema_revision,
                            expected_row_revision,
                        },
                    )
                    .await,
                ),
            }
        }
        "managed_rows_v2_list" => {
            let params: v3::RowList = parse_args(cx, args)?;
            result(cx, port::managed_rows_v3_list(cx.st, cx.me, params).await)
        }
        "managed_table_v2_submit" => {
            let params: v3::Submit = parse_args(cx, args)?;
            result(
                cx,
                port::managed_table_v3_submit(cx.st, cx.me, params).await,
            )
        }
        "managed_table_v2_request_get" => {
            let params: v3::RequestGet = parse_args(cx, args)?;
            result(
                cx,
                port::managed_table_v3_request_get(cx.st, cx.me, params).await,
            )
        }
        _ => unreachable!("versioned table tools are catalog-routed"),
    })
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

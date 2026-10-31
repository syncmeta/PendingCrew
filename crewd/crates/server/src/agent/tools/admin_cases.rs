//! Authenticated bounded case coordination; no caller-selected recipient or crew.
use super::{Ctx, parse_args};
use crate::agent::port;
use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    request_id: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    after: Option<String>,
    limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Accept {
    request_id: String,
    expected_revision: String,
    note: String,
}
fn receipt<T: serde::Serialize>(cx: &Ctx<'_>, result: port::PortResult<T>) -> CallResult {
    match result {
        Ok(view) => match serde_json::to_string(&view) {
            Ok(text) => CallResult::ok(text),
            Err(error) => cx.fail(port::PortError::Internal(error.to_string())),
        },
        Err(error) => cx.fail(error),
    }
}
pub async fn request(cx: &Ctx<'_>, args: Value) -> CallResult {
    let p: Request = match parse_args(cx, args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::admin_handoff_request(cx.st, &cx.me.session_id, p.request_id, p.reason).await,
    )
}
pub async fn get(cx: &Ctx<'_>, args: Value) -> CallResult {
    let p: Get = match parse_args(cx, args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::admin_handoff_get(cx.st, &cx.me.session_id, p.request_id).await,
    )
}
pub async fn accept(cx: &Ctx<'_>, args: Value) -> CallResult {
    let p: Accept = match parse_args(cx, args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::admin_handoff_accept(
            cx.st,
            &cx.me.session_id,
            p.request_id,
            p.expected_revision,
            p.note,
        )
        .await,
    )
}
pub async fn list(cx: &Ctx<'_>, args: Value) -> CallResult {
    let p: List = match parse_args(cx, args) {
        Ok(p) => p,
        Err(e) => return e,
    };
    receipt(
        cx,
        port::admin_handoff_list(cx.st, &cx.me.session_id, p.after, p.limit.unwrap_or(50)).await,
    )
}

//! Three tools available only through a bound Secretary envelope.
use super::{Ctx, parse_args};
use crate::agent::{admin_case_scope, port};
use crewd_proto::{AdminSecretaryPublish, AdminSecretaryReference, agent::CallResult};
use serde::Deserialize;
use serde_json::Value;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    request_id: String,
    source_revision: String,
    generation: String,
    body: String,
    references: Vec<AdminSecretaryReference>,
}
pub(crate) fn publish_params(args: Value) -> Result<AdminSecretaryPublish, serde_json::Error> {
    let p: Publish = serde_json::from_value(args)?;
    Ok(AdminSecretaryPublish {
        request_id: p.request_id,
        source_revision: p.source_revision,
        generation: p.generation,
        body: p.body,
        references: p.references,
    })
}
fn value<T: serde::Serialize>(cx: &Ctx<'_>, v: port::PortResult<T>) -> CallResult {
    match v {
        Ok(v) => match serde_json::to_string(&v) {
            Ok(v) => CallResult::ok(v),
            Err(e) => cx.fail(port::PortError::Internal(e.to_string())),
        },
        Err(e) => cx.fail(e),
    }
}
pub async fn call(cx: &Ctx<'_>, tool: &str, args: Value) -> CallResult {
    let Some(authority) = admin_case_scope::capture() else {
        return CallResult::err("Secretary case authority unavailable".into());
    };
    let witness = match authority.secretary_witness() {
        Ok(w) => w,
        Err(_) => return CallResult::err("Secretary case authority unavailable".into()),
    };
    match tool {
        crewd_proto::admin_secretary::SNAPSHOT_TOOL => {
            if let Err(e) = parse_args::<Empty>(cx, args) {
                return e;
            }
            value(cx, port::secretary_snapshot(cx.st, witness).await)
        }
        crewd_proto::admin_secretary::RECEIPT_TOOL => {
            let p: Receipt = match parse_args(cx, args) {
                Ok(p) => p,
                Err(e) => return e,
            };
            value(
                cx,
                port::secretary_receipt(cx.st, witness, p.request_id).await,
            )
        }
        crewd_proto::admin_secretary::PUBLISH_TOOL => {
            let p = match publish_params(args) {
                Ok(p) => p,
                Err(_) => {
                    return CallResult::err("Invalid Secretary publication parameters".into());
                }
            };
            value(cx, port::secretary_publish(cx.st, witness, p).await)
        }
        _ => CallResult::err("Secretary tool unavailable".into()),
    }
}

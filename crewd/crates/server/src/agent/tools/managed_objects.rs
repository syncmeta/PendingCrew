//! Typed organization-object and decision-request tools. Their connection and token must be
//! revalidated at the same FIFO cut as every read or write.
use super::{Ctx, parse_args, port};
use crewd_collab::managed_objects::{self as objects, Actor};
use crewd_proto::{
    Empty,
    agent::CallResult,
    decision_asks::{LookupParams, Open},
    managed_objects::{Scope, Submit},
    managed_objects_api::{DecisionsParams, GetParams, RequestParams, RowsParams, TablesParams},
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
        "ask_decision" => {
            let p: Open = parse_args(cx, args)?;
            let (receipt, fresh) = port::open_decision_ask(cx.st, cx.me, p)
                .await
                .map_err(|error| cx.fail(error))?;
            if fresh {
                crate::agent::turn::note_post(
                    cx.st,
                    cx.me,
                    &[crewd_proto::Mention {
                        kind: crewd_proto::MentionKind::Human,
                        target_id: None,
                    }],
                )
                .await;
            }
            result(cx, Ok(receipt))
        }
        "ask_decision_request_get" => {
            let p: LookupParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    crewd_collab::decision_asks::lookup(c, &by.session_id, &p.scope, &p.request_id)
                })
                .await,
            )
        }
        "artifact_preview" => {
            let p: crewd_proto::managed_artifacts::PreviewParams=parse_args(cx,args)?;
            result(cx,port::artifact_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_artifacts::preview(c,Actor::Session(&by.session_id),&p.crew_id,&p.input)
            }).await)
        }
        "artifacts_list" => {
            let p: crewd_proto::managed_artifacts::ListParams=parse_args(cx,args)?;
            result(cx,port::artifact_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_artifacts::list(c,Actor::Session(&by.session_id),p.after.as_ref())
            }).await)
        }
        "artifact_get" => {
            let p: crewd_proto::managed_artifacts::GetParams=parse_args(cx,args)?;
            result(cx,port::artifact_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_artifacts::get(c,Actor::Session(&by.session_id),&p.reference)
            }).await)
        }
        "artifact_backlinks" => {
            let p: crewd_proto::managed_artifacts::BacklinksParams=parse_args(cx,args)?;
            result(cx,port::artifact_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_artifacts::backlinks(c,Actor::Session(&by.session_id),&p.reference,p.after.as_deref())
            }).await)
        }
        "artifact_subject" => {
            let p: crewd_proto::managed_artifacts::SubjectParams=parse_args(cx,args)?;
            result(cx,port::artifact_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_artifacts::subject(c,Actor::Session(&by.session_id),&p.subject,p.after.as_deref())
            }).await)
        }
        "artifact_submit" => {
            let p: crewd_proto::managed_artifacts::Submit=parse_args(cx,args)?;
            result(cx,port::artifact_submit(cx.st,cx.me,p).await)
        }
        "artifact_request_get" => {
            let p: RequestParams=parse_args(cx,args)?;
            result(cx,port::artifact_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_artifacts::lookup(c,Actor::Session(&by.session_id),&p.scope,&p.crew_id,&p.request_id)
            }).await)
        }
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
                    objects::get_with_defaults(
                        c,
                        Actor::Session(&by.session_id),
                        &p.id,
                        p.include_system_defaults,
                    )
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
            let p: DecisionsParams = parse_args(cx, args)?;
            result(
                cx,
                port::managed_read(cx.st, cx.me, move |c, by| {
                    objects::decisions_with_defaults(
                        c,
                        Actor::Session(&by.session_id),
                        p.after.as_deref(),
                        p.include_system_defaults,
                    )
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
        "managed_object_from_message" => {
            let p: crewd_proto::managed_message_sources::RecordParams = parse_args(cx,args)?;
            result(cx,port::managed_record_from_message(cx.st,cx.me,p).await)
        }
        "managed_message_request_get" => {
            let p: RequestParams = parse_args(cx,args)?;
            result(cx,port::managed_message_request_get(cx.st,cx.me,p).await)
        }
        "managed_object_source_get" => {
            let p: crewd_proto::managed_message_sources::GetParams = parse_args(cx,args)?;
            result(cx,port::managed_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_message_sources::source(c,Actor::Session(&by.session_id),&p.id)
            }).await)
        }
        "managed_objects_from_message" => {
            let p: crewd_proto::managed_message_sources::MessageObjectsParams = parse_args(cx,args)?;
            result(cx,port::managed_read(cx.st,cx.me,move |c,by| {
                crewd_collab::managed_message_sources::objects_from_message(c,Actor::Session(&by.session_id),&p.message_id,p.after.as_deref())
            }).await)
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

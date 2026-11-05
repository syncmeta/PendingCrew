//! The existing search tool gains organization scope; one generic read expands
//! real links. All facts and authorization remain in the typed FIFO ports.
use super::{Ctx, crew_arg, parse_args, port, resolve_channel};
use crate::agent::Role;
use crewd_proto::agent::CallResult;
use crewd_proto::managed_tables_v2::TargetRef;
use crewd_proto::organization_search as wire;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchArgs {
    pub query: String,
    pub channel: Option<String>,
    pub crew: Option<String>,
    pub after: Option<String>,
    pub before: Option<String>,
    pub limit: Option<u32>,
    pub r#in: Option<wire::Domain>,
    pub r#type: Option<String>,
    pub cursor: Option<String>,
}

fn refused(cx: &Ctx<'_>, code: crewd_collab::refusal::Code) -> CallResult {
    cx.fail(port::PortError::from(crewd_collab::CollabError::Conflict(
        crewd_collab::refusal::Refusal::new(code),
    )))
}

fn invalid(cx: &Ctx<'_>, field: &str) -> CallResult {
    cx.fail(port::PortError::from(crewd_collab::CollabError::Invalid(
        crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::SearchInvalid)
            .arg("field", field),
    )))
}

fn kind_filter(raw: Option<&str>) -> Result<(Option<Vec<wire::Kind>>, Option<String>), ()> {
    use wire::Kind;
    let Some(raw) = raw else {
        return Ok((None, None));
    };
    let kind = match raw {
        "decision" => Kind::Decision,
        "risk" => Kind::Risk,
        "lesson" => Kind::Lesson,
        "work" => Kind::Work,
        "requirement" => Kind::Requirement,
        "ask" => Kind::Ask,
        "table" => Kind::Table,
        "row" => Kind::Row,
        "artifact" => Kind::Artifact,
        "message" => Kind::Message,
        _ => {
            let n = raw.strip_prefix('T').ok_or(())?;
            if n.starts_with('0')
                || !n.bytes().all(|b| b.is_ascii_digit())
                || n.parse::<i64>().ok().is_none_or(|n| n <= 0)
            {
                return Err(());
            }
            return Ok((Some(vec![Kind::Row]), Some(raw.into())));
        }
    };
    Ok((Some(vec![kind]), None))
}

/// Only the old message search is available without this runtime. This keeps
/// pre-search hosts and legacy administrative readers within their old scope.
pub(super) fn use_unified(cx: &Ctx<'_>, args: &SearchArgs) -> bool {
    cx.st.organization_search_runtime.get().is_some()
        && (matches!(cx.me.role, Role::Worker | Role::Captain)
            || args.r#in == Some(wire::Domain::Messages)
            || args.r#type.as_deref() == Some("message"))
}

pub(super) fn validate_legacy(cx: &Ctx<'_>, args: &SearchArgs) -> Result<(), CallResult> {
    if args.cursor.is_some()
        || matches!(args.r#in, Some(wire::Domain::Objects | wire::Domain::All))
        || args.r#type.as_deref().is_some_and(|kind| kind != "message")
    {
        return Err(refused(cx, crewd_collab::refusal::Code::SearchUnavailable));
    }
    Ok(())
}

pub(super) async fn search(cx: &Ctx<'_>, args: SearchArgs) -> CallResult {
    match search_inner(cx, args).await {
        Ok(result) | Err(result) => result,
    }
}

async fn search_inner(cx: &Ctx<'_>, args: SearchArgs) -> Result<CallResult, CallResult> {
    let (kinds, table_id) = kind_filter(args.r#type.as_deref()).map_err(|_| invalid(cx, "type"))?;
    let domain = args.r#in.unwrap_or(wire::Domain::All);
    if !matches!(cx.me.role, Role::Worker | Role::Captain)
        && domain != wire::Domain::Messages
        && args.r#type.as_deref() != Some("message")
    {
        return Err(
            cx.fail(port::PortError::from(crewd_collab::CollabError::Forbidden(
                crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::ResourceForbidden),
            ))),
        );
    }
    let channel_id = match args.channel.as_deref() {
        Some(arg) => Some(resolve_channel(cx, arg).await?.id),
        None => None,
    };
    let crew_id = match args.crew.as_deref() {
        Some(arg) => Some(crew_arg(cx, arg).await?.id),
        None => None,
    };
    let query = wire::Search {
        scope: None,
        query: args.query,
        r#in: domain,
        kinds,
        table_id,
        crew_id,
        channel_id,
        after_time: args.after,
        before_time: args.before,
        cursor: args.cursor,
        limit: args.limit.unwrap_or(50).clamp(1, 200) as u16,
        include_system_defaults: false,
    };
    let page = port::organization_search(cx.st, cx.me, query)
        .await
        .map_err(|error| cx.fail(error))?;
    grouped(cx, page)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    kind: wire::Kind,
    table_id: Option<String>,
    table_title: Option<String>,
    items: Vec<wire::Hit>,
}

fn grouped(cx: &Ctx<'_>, page: wire::Page) -> Result<CallResult, CallResult> {
    let mut groups: Vec<Group> = Vec::new();
    for hit in page.items {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.kind == hit.kind && group.table_id == hit.table_id)
        {
            group.items.push(hit);
        } else {
            groups.push(Group {
                kind: hit.kind,
                table_id: hit.table_id.clone(),
                table_title: hit.table_title.clone(),
                items: vec![hit],
            });
        }
    }
    let value = serde_json::json!({
        "scope": page.scope,
        "indexState": page.index_state,
        "snapshot": page.snapshot,
        "groups": groups,
        "nextCursor": page.next_cursor,
    });
    encode(cx, value)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpandArgs {
    target: TargetRef,
    cursor: Option<String>,
    limit: Option<u16>,
    #[serde(default)]
    include_system_defaults: bool,
}

pub(super) async fn expand(cx: &Ctx<'_>, args: Value) -> CallResult {
    let args: ExpandArgs = match parse_args(cx, args) {
        Ok(args) => args,
        Err(error) => return error,
    };
    if cx.st.organization_search_runtime.get().is_none() {
        return refused(cx, crewd_collab::refusal::Code::SearchUnavailable);
    }
    let query = wire::Expand {
        scope: None,
        target: args.target,
        cursor: args.cursor,
        limit: args.limit.unwrap_or(wire::DEFAULT_LIMIT),
        include_system_defaults: args.include_system_defaults,
    };
    match port::organization_search_expand(cx.st, cx.me, query).await {
        Ok(page) => match encode(cx, page) {
            Ok(result) | Err(result) => result,
        },
        Err(error) => cx.fail(error),
    }
}

fn encode(cx: &Ctx<'_>, value: impl Serialize) -> Result<CallResult, CallResult> {
    serde_json::to_string(&value)
        .map(CallResult::ok)
        .map_err(|error| cx.fail(port::PortError::Internal(error.to_string())))
}

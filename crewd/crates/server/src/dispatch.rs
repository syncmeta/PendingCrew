//! 客户端 RPC（spec §22.2）。客户端都是人：写操作的发送者一律 `Sender::Human`。
//! 写操作经 `AppState::collab`（同一事务 + 提交后发事件）；纯读直接在库线程上读。
use crate::AppState;
use crewd_collab::{
    CollabError, Sender, approvals, asks, channels, crews, cursors, directory, messages, restart,
    sessions,
};
use crewd_proto::*;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

pub async fn handle(state: &AppState, req: Request) -> Response {
    let id = req.id;
    match route(state, req).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(RpcError::INTERNAL, e.to_string())
}

fn to_value<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

/// collab 错误 → RPC 错误码（M3b 的 agent RPC 复用同一张表）。
pub fn collab_err(e: CollabError) -> RpcError {
    match e {
        CollabError::NotFound(m) => RpcError::new(RpcError::NOT_FOUND, m),
        CollabError::Invalid(m) => RpcError::new(RpcError::INVALID_PARAMS, m),
        CollabError::Forbidden(m) => RpcError::new(RpcError::FORBIDDEN, m),
        CollabError::Conflict(m) => RpcError::new(RpcError::CONFLICT, m),
        CollabError::Work(crewd_work::Rejected::Forbidden(a)) => {
            RpcError::new(RpcError::FORBIDDEN, format!("not allowed: {a}"))
        }
        CollabError::Work(r) => RpcError::new(RpcError::INVALID_PARAMS, r.to_string()),
        CollabError::Store(e) => internal(e),
    }
}

async fn write<T: Serialize + Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&mut crewd_collab::Tx<'_>) -> crewd_collab::Result<T> + Send + 'static,
) -> Result<Value, RpcError> {
    to_value(state.collab(f).await.map_err(collab_err)?)
}

async fn read<T: Serialize + Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Connection) -> crewd_collab::Result<T> + Send + 'static,
) -> Result<Value, RpcError> {
    let v = state
        .db
        .call(move |s| f(s.conn()))
        .await
        .map_err(internal)?
        .map_err(collab_err)?;
    to_value(v)
}

async fn route(state: &AppState, req: Request) -> Result<Value, RpcError> {
    let p = req.params;
    match req.method.as_str() {
        methods::HELLO => {
            parse_no_params(p)?;
            to_value(Hello {
                server: "crewd".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                api_version: API_VERSION,
            })
        }
        methods::CREW_LIST => {
            parse_no_params(p)?;
            read(state, crews::list).await
        }
        methods::CREW_GET => {
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| crews::detail(tx, &p.crew_id)).await
        }
        methods::CREW_CREATE => {
            let p: CrewCreateParams = parse_params(p)?;
            write(state, move |tx| {
                crews::create(
                    tx,
                    crews::NewCrew {
                        title: p.title,
                        working_directory: p.working_directory,
                        parent_id: p.parent_id,
                        title_source: Some("human".into()),
                    },
                )
            })
            .await
        }
        methods::CREW_UPDATE => {
            let p: CrewUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                let title_source = p.title.as_ref().map(|_| "human".to_string());
                crews::update(
                    tx,
                    &p.crew_id,
                    crews::CrewPatch {
                        title: p.title,
                        title_source,
                        working_directory: p.working_directory,
                        hidden: p.hidden,
                        status_line: p.status_line,
                        policy: None,
                    },
                )
            })
            .await
        }
        methods::CREW_ADOPT => {
            let p: CrewAdoptParams = parse_params(p)?;
            write(state, move |tx| crews::adopt(tx, &p.crew_id, &p.parent_id)).await
        }
        methods::CREW_REORDER => {
            // C1：一次排一层。parentId 给了 = 这个父名下（边上的次序 → 父的 childIds）；缺省 = 顶层（Crew.position）。
            let p: CrewReorderParams = parse_params(p)?;
            write(state, move |tx| {
                crews::reorder(tx, p.parent_id.as_deref(), &p.crew_ids)
            })
            .await
        }
        methods::CREW_RELEASE => {
            let p: CrewReleaseParams = parse_params(p)?;
            write(state, move |tx| {
                crews::release(tx, &p.crew_id, &p.from_parent_id, p.to_parent_id.as_deref())
            })
            .await
        }
        methods::CREW_DELETE => {
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| {
                crews::delete(tx, &p.crew_id).map(|()| Empty {})
            })
            .await
        }
        methods::CHANNEL_LIST => {
            // 写路径：根机组的人类群懒建（R1）。
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| channels::list_for_crew(tx, &p.crew_id)).await
        }
        methods::CHANNEL_CREATE => {
            let p: ChannelCreateParams = parse_params(p)?;
            write(state, move |tx| {
                channels::create(
                    tx,
                    &Sender::Human,
                    channels::NewChannel {
                        crew_id: p.crew_id,
                        kind: p.kind,
                        name: p.name,
                        purpose: p.purpose,
                        members: p.members,
                    },
                )
            })
            .await
        }
        methods::CHANNEL_UPDATE => {
            let p: ChannelUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                channels::update(
                    tx,
                    &Sender::Human,
                    &p.channel_id,
                    channels::ChannelPatch {
                        name: p.name,
                        purpose: p.purpose,
                        archived: p.archived,
                    },
                )
            })
            .await
        }
        methods::CHANNEL_ADD_MEMBERS => {
            let p: ChannelMembersParams = parse_params(p)?;
            write(state, move |tx| {
                channels::add_members(tx, &Sender::Human, &p.channel_id, &p.members)
            })
            .await
        }
        methods::CHANNEL_REMOVE_MEMBER => {
            let p: ChannelMemberParams = parse_params(p)?;
            write(state, move |tx| {
                channels::remove_member(tx, &Sender::Human, &p.channel_id, &p.member)
            })
            .await
        }
        methods::CHANNEL_WATCH => {
            let p: ChannelWatchParams = parse_params(p)?;
            write(state, move |tx| channels::watch(tx, &p.channel_id, p.on)).await
        }
        methods::CHANNEL_SET_OWNER => {
            // 人 = 强制转让（§22.7 Q2）。
            let p: ChannelSetOwnerParams = parse_params(p)?;
            write(state, move |tx| {
                channels::force_owner(tx, &Sender::Human, &p.channel_id, &p.owner)
            })
            .await
        }
        methods::MESSAGE_LIST => {
            let p: MessageListParams = parse_params(p)?;
            read(state, move |c| {
                messages::list(
                    c,
                    &p.channel_id,
                    p.after_seq,
                    p.before_seq,
                    p.limit.unwrap_or(200),
                )
            })
            .await
        }
        methods::MESSAGE_POST => {
            let p: MessagePostParams = parse_params(p)?;
            write(state, move |tx| {
                messages::post(
                    tx,
                    Sender::Human,
                    messages::NewPost {
                        channel_id: p.channel_id,
                        text: p.text,
                        in_reply_to: p.in_reply_to,
                        mentions: p.mentions,
                        attachments: p.attachments,
                        category: None,
                    },
                )
                .map(|posted| posted.message)
            })
            .await
        }
        methods::MESSAGE_SEARCH => {
            let p: MessageSearchParams = parse_params(p)?;
            read(state, move |c| {
                messages::search(
                    c,
                    &messages::Search {
                        query: p.query,
                        crew_id: p.crew_id,
                        channel_ids: p.channel_ids,
                        after: p.after,
                        before: p.before,
                        limit: p.limit.unwrap_or(50),
                    },
                )
            })
            .await
        }
        methods::READ_MARK => {
            let p: ReadMarkParams = parse_params(p)?;
            write(state, move |tx| {
                cursors::mark_read(tx, &p.channel_id, p.seq)?;
                channels::summary(tx.conn(), &p.channel_id)
            })
            .await
        }
        methods::SESSION_LIST => {
            let p: SessionListParams = parse_params_or_default(p)?;
            read(state, move |c| sessions::list(c, p.crew_id.as_deref())).await
        }
        methods::SESSION_GET => {
            let p: SessionGetParams = parse_params(p)?;
            read(state, move |c| sessions::get(c, &p.session_id)).await
        }
        methods::ASK_LIST => {
            let p: AskListParams = parse_params_or_default(p)?;
            let now = crewd_store::now_ms();
            read(state, move |c| {
                asks::waits(c, now, p.root_crew_id.as_deref())
            })
            .await
        }
        methods::ASK_ANSWER => {
            let p: AskAnswerParams = parse_params(p)?;
            write(state, move |tx| {
                asks::answer(tx, &p.crew_id, &p.ask, &p.text)
            })
            .await
        }
        methods::ASK_WITHDRAW => {
            let p: AskWithdrawParams = parse_params(p)?;
            write(state, move |tx| {
                asks::withdraw(tx, &Sender::Human, &p.crew_id, &p.ask, &p.reason)
            })
            .await
        }
        methods::APPROVAL_LIST => {
            parse_no_params(p)?;
            read(state, approvals::list_pending).await
        }
        methods::APPROVAL_DECIDE => {
            let p: ApprovalDecideParams = parse_params(p)?;
            write(state, move |tx| {
                approvals::decide(tx, &p.id, p.allow, "human")
            })
            .await
        }
        methods::RESTART_PENDING => {
            parse_no_params(p)?;
            read(state, restart::pending).await
        }
        methods::RESTART_ANSWER => {
            let p: RestartAnswerParams = parse_params(p)?;
            write(state, move |tx| restart::answer(tx, p.id, p.answer)).await
        }
        methods::DIRECTORY_LIST => {
            let p: DirectoryListParams = parse_params_or_default(p)?;
            read(state, move |c| directory::list(c, p.query.as_deref())).await
        }
        methods::TODO_LIST => {
            let p: TodoListParams = parse_params(p)?;
            read(state, move |c| {
                Ok(crewd_store::todos::list(c, &p.crew_id, p.ledger)?)
            })
            .await
        }
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("未知方法: {other}"),
        )),
    }
}

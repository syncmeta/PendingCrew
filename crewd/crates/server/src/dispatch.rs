//! 客户端 RPC（spec §22.2）。客户端都是人：写操作的发送者一律 `Sender::Human`。
//! 写操作经 `AppState::collab`（同一事务 + 提交后发事件）；纯读直接在库线程上读。
use crate::AppState;
use crewd_collab::{
    CollabError, Sender, asks, channels, crews, cursors, directory, messages, restart, sessions,
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

fn to_value<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

/// 内部错误（库 / SQL / 序列化失败、账本不变式坏了）：细节只进日志（stderr），客户端拿到一句通用话 + 一个编号
/// （对日志用）。错误原文可能带表名、SQL、数据片段，不上线（Task 14 复审裁定 5）。
fn internal(detail: impl std::fmt::Display) -> RpcError {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    eprintln!("crewd: 内部错误 #{n}: {detail}");
    RpcError::new(
        RpcError::INTERNAL,
        format!("内部错误 #{n}（详情见 crewd 日志）"),
    )
}

/// collab 错误 → RPC 错误码（M3b 的 agent RPC 复用同一张表）。
pub fn collab_err(e: CollabError) -> RpcError {
    use crewd_work::Rejected as R;
    match e {
        CollabError::NotFound(m) => RpcError::new(RpcError::NOT_FOUND, m),
        CollabError::Invalid(m) => RpcError::new(RpcError::INVALID_PARAMS, m),
        CollabError::Forbidden(m) => RpcError::new(RpcError::FORBIDDEN, m),
        CollabError::Conflict(m) => RpcError::new(RpcError::CONFLICT, m),
        CollabError::Work(R::Forbidden(a)) => {
            RpcError::new(RpcError::FORBIDDEN, format!("not allowed: {a}"))
        }
        // 号码格式对、账本里没有（R11；`asks::with_book` 已先映射成 NotFound，这里兜底同一个码）。
        CollabError::Work(r @ (R::UnknownAsk(_) | R::UnknownItem(_) | R::UnknownEntry(_))) => {
            RpcError::new(RpcError::NOT_FOUND, r.to_string())
        }
        // 与当前状态冲突（控制者裁定 a）：换个时机就不一样，不是参数写错。
        CollabError::Work(
            r @ (R::AskNotOpen(_)
            | R::TransferPending(_)
            | R::NoPendingTransfer(_)
            | R::EntryClosed(_)
            | R::Terminal(_)
            | R::AskInUse(_)),
        ) => RpcError::new(RpcError::CONFLICT, r.to_string()),
        // 账本自己的不变式坏了 = 服务端的错（控制者裁定 d），细节不上线。
        CollabError::Work(R::Invariant(v)) => {
            internal(format!("work book invariant violated: {v}"))
        }
        CollabError::Work(r) => RpcError::new(RpcError::INVALID_PARAMS, r.to_string()),
        CollabError::Store(crewd_store::StoreError::NotFound(m)) => {
            RpcError::new(RpcError::NOT_FOUND, m)
        }
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
                crews::reorder(tx, &p.parent_id, &p.crew_ids)
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
                        members: p.members.into_iter().map(Into::into).collect(),
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
                let members: Vec<MemberRef> = p.members.into_iter().map(Into::into).collect();
                channels::add_members(tx, &Sender::Human, &p.channel_id, &members)
            })
            .await
        }
        methods::CHANNEL_REMOVE_MEMBER => {
            let p: ChannelMemberParams = parse_params(p)?;
            write(state, move |tx| {
                channels::remove_member(tx, &Sender::Human, &p.channel_id, &p.member.into())
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
                channels::force_owner(tx, &Sender::Human, &p.channel_id, &p.owner.into())
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
                        mentions: p.mentions.into_iter().map(Into::into).collect(),
                        attachments: p.attachments.into_iter().map(Into::into).collect(),
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
                asks::waits(c, now, p.home_crew_id.as_deref())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_store::StoreError;
    use crewd_work::{AskId, EntryNo, Rejected, Violation, WorkId};

    fn code(e: CollabError) -> i32 {
        collab_err(e).code
    }

    /// 控制者裁定 a：与当前状态冲突（问已经答过 / 撤过、转交已在途 / 没有在途、入口已关、项已结、问还卡着活项）
    /// = -32009（Mac 与 iOS 同时答同一条问，后到的要看到「已经答了」，不是「参数错」）。
    #[test]
    fn state_conflicts_are_conflict_not_invalid_params() {
        for r in [
            Rejected::AskNotOpen(AskId(1)),
            Rejected::TransferPending(WorkId(1)),
            Rejected::NoPendingTransfer(WorkId(1)),
            Rejected::EntryClosed(EntryNo(1)),
            Rejected::Terminal(WorkId(1)),
            Rejected::AskInUse(AskId(1)),
        ] {
            let what = r.to_string();
            assert_eq!(code(CollabError::Work(r)), RpcError::CONFLICT, "{what}");
        }
        assert_eq!(
            code(CollabError::Work(Rejected::Blank("text"))),
            RpcError::INVALID_PARAMS
        );
    }

    /// 复审裁定 5：库 / SQL 的错误原文不上线，只给通用话 + 编号（细节进 stderr）。
    #[test]
    fn store_errors_do_not_leak_their_text() {
        for e in [
            StoreError::Invalid("表 secret_table 坏了".into()),
            StoreError::Sql(rusqlite::Error::InvalidColumnName("secret_col".into())),
            StoreError::DbClosed,
        ] {
            let detail = e.to_string();
            let r = collab_err(CollabError::Store(e));
            assert_eq!(r.code, RpcError::INTERNAL);
            assert!(
                !r.message.contains(&detail) && !r.message.contains("secret"),
                "库错误原文不上线：{}",
                r.message
            );
            assert!(r.message.contains('#'), "带编号：{}", r.message);
        }
    }

    /// 控制者裁定 d：store 层的「没有这一行」= -32004；账本不变式被破坏 = -32603，细节不给客户端（只给编号，对日志）。
    #[test]
    fn store_not_found_and_invariants_map_to_their_codes() {
        assert_eq!(
            code(CollabError::Store(StoreError::NotFound("没有 x".into()))),
            RpcError::NOT_FOUND
        );
        for r in [
            Rejected::UnknownAsk(AskId(9)),
            Rejected::UnknownItem(WorkId(9)),
            Rejected::UnknownEntry(EntryNo(9)),
        ] {
            let what = r.to_string();
            assert_eq!(code(CollabError::Work(r)), RpcError::NOT_FOUND, "{what}");
        }
        let v = Violation::BlankTitle(WorkId(77));
        let detail = v.to_string();
        let e = collab_err(CollabError::Work(Rejected::Invariant(v)));
        assert_eq!(e.code, RpcError::INTERNAL);
        assert!(
            !e.message.contains("W77") && !e.message.contains(&detail),
            "不变式的细节不上线：{}",
            e.message
        );
        assert!(
            e.message.contains('#'),
            "带一个对日志用的编号：{}",
            e.message
        );
    }
}

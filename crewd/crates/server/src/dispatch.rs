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

/// 把内部错误细节写进日志，返回对日志用的编号（agent 工具回执也用它：只给编号，不给细节）。
pub(crate) fn log_internal(detail: impl std::fmt::Display) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    eprintln!("crewd: 内部错误 #{n}: {detail}");
    n
}

/// 内部错误（库 / SQL / 序列化失败、账本不变式坏了）：细节只进日志（stderr），客户端拿到一句通用话 + 一个编号
/// （对日志用）。错误原文可能带表名、SQL、数据片段，不上线（Task 14 复审裁定 5）。
pub(crate) fn internal(detail: impl std::fmt::Display) -> RpcError {
    let n = log_internal(detail);
    RpcError::new(
        RpcError::INTERNAL,
        format!("内部错误 #{n}（详情见 crewd 日志）"),
    )
}

/// 规则拒绝 → RPC 错误：message 是中文缺省，`data.refusal` 是稳定码，客户端按码本地化。
pub(crate) fn refused(code: i32, r: crewd_collab::refusal::Refusal) -> RpcError {
    let mut error = RpcError::new(code, r.to_string()).with_refusal(r.code.as_str());
    error
        .data
        .as_mut()
        .expect("with_refusal has data")
        .current_work = r.current_work;
    let data = error.data.as_mut().expect("refusal data");
    data.current_document = r.current_document;
    data.current_tree_revision = r.current_tree_revision;
    data.current_link_revision = r.current_link_revision;
    data.current_source_revision = r.current_source_revision;
    error
}

/// collab 错误 → RPC 错误码（M3b 的 agent RPC 复用同一张表）。
pub fn collab_err(e: CollabError) -> RpcError {
    use crewd_work::Rejected as R;
    match e {
        CollabError::NotFound(r) => refused(RpcError::NOT_FOUND, r),
        CollabError::Invalid(r) => refused(RpcError::INVALID_PARAMS, r),
        CollabError::Forbidden(r) => refused(RpcError::FORBIDDEN, r),
        CollabError::Conflict(r) => refused(RpcError::CONFLICT, r),
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
        CollabError::Store(e) => match e.storage_kind() {
            Some(kind) => storage(kind, e),
            None => internal(e),
        },
    }
}

/// 存储故障（S26）：给人一句原因 + 编号，原文（路径、SQL）只进日志。
pub(crate) fn storage(kind: crewd_store::StorageKind, detail: impl std::fmt::Display) -> RpcError {
    let e = internal(detail);
    RpcError::new(
        RpcError::STORAGE,
        format!("存储故障：{}。{}", kind.describe(), e.message),
    )
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
        .call_result(move |s| f(s.conn()))
        .await
        .map_err(internal)?
        .map_err(collab_err)?;
    to_value(v)
}

/// Only read the independent in-memory history. Never queue a DB job to report its failure.
fn current_db_health(state: &AppState) -> Option<DaemonHealth> {
    use crewd_store::db_health::{DbFaultKind, DbFaultStage};
    state.db.health().snapshot().map(|fault| {
        let stage = match fault.stage {
            DbFaultStage::Enqueue => DbFaultStageView::Enqueue,
            DbFaultStage::Receive => DbFaultStageView::Receive,
            DbFaultStage::Job => DbFaultStageView::Job,
            DbFaultStage::Finish => DbFaultStageView::Finish,
        };
        let (kind, storage_kind) = match fault.kind {
            DbFaultKind::Closed => (DbFaultKindView::DbClosed, None),
            DbFaultKind::JobPanicked => (DbFaultKindView::DbJobPanicked, None),
            DbFaultKind::Sql => (DbFaultKindView::Sql, None),
            DbFaultKind::Integrity => (DbFaultKindView::Integrity, None),
            DbFaultKind::Unclassified => (DbFaultKindView::Unclassified, None),
            DbFaultKind::Storage(kind) => {
                let kind = match kind {
                    crewd_store::StorageKind::DiskFull => DbStorageKindView::DiskFull,
                    crewd_store::StorageKind::Io => DbStorageKindView::Io,
                    crewd_store::StorageKind::Corrupt => DbStorageKindView::Corrupt,
                    crewd_store::StorageKind::ReadOnly => DbStorageKindView::ReadOnly,
                    crewd_store::StorageKind::CantOpen => DbStorageKindView::CantOpen,
                };
                (DbFaultKindView::Storage, Some(kind))
            }
        };
        DaemonHealth {
            last_db_error: Some(DbFaultView {
                stage,
                kind,
                storage_kind,
                at_ms: fault.at_ms,
                sequence: fault.sequence,
            }),
        }
    })
}

async fn route(state: &AppState, req: Request) -> Result<Value, RpcError> {
    if req.method.starts_with("remote.") {
        let remote = state
            .remote
            .get()
            .ok_or_else(|| RpcError::new(RpcError::METHOD_NOT_FOUND, "远程组件未初始化"))?;
        return remote.local_route(&req.method, req.params).await;
    }

    let p = req.params;
    match req.method.as_str() {
        methods::HELLO => {
            // U1：客户端报自己是谁（记一行）；太老的拒掉，比 crewd 新的照收（客户端自己决定怎么降级）。
            let p: HelloParams = parse_params_or_default(p)?;
            let client = p.client.as_ref();
            eprintln!(
                "crewd: hello：客户端 {:?} {:?}（构建 {:?}），API {:?}，能力 {:?}",
                client.map(|c| c.name.as_str()),
                client.map(|c| c.version.as_str()),
                client.and_then(|c| c.build.as_deref()),
                p.api_version,
                p.capabilities
            );
            if let Some(v) = p.api_version
                && v < MIN_API_VERSION
            {
                return Err(refused(
                    RpcError::CONFLICT,
                    crewd_collab::refusal::Refusal::new(
                        crewd_collab::refusal::Code::SystemApiIncompatible,
                    )
                    .arg("client", v.to_string())
                    .arg("min", MIN_API_VERSION.to_string())
                    .arg("current", API_VERSION.to_string()),
                ));
            }
            let mut capabilities: Vec<String> = match state.orchestra.get() {
                Some(_) => orchestra::CAPABILITIES
                    .iter()
                    .map(|c| c.to_string())
                    .collect(),
                None => Vec::new(),
            };
            if state.data_runtime.get().is_some() {
                capabilities.push("data".into());
            }
            if state.management.get().is_some() {
                capabilities.push("lifecycle.management".into());
            }
            if state.judgment_runtime.get().is_some() {
                capabilities.push("judgment".into());
            }
            if state.runtime_identity.get().is_some() {
                capabilities.push("command.receipts".into());
            }
            if state.orchestra.get().is_some()
                && state.native_helpers.get().is_some()
                && state
                    .terminal_supported
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                capabilities.push("session.terminal".into());
            }
            capabilities.extend(
                ["work.reassignLost", "viewFolders", "channelWork"]
                    .into_iter()
                    .map(str::to_owned),
            );
            if state.data_runtime.get().is_none() {
                capabilities.push("documents".into());
            }
            to_value(Hello {
                server: "crewd".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                api_version: API_VERSION,
                min_api_version: MIN_API_VERSION,
                build: crate::build_info::build_info(),
                capabilities,
                daemon_health: current_db_health(state),
                remote_binding: None,
                runtime_identity: state.runtime_identity.get().cloned(),
            })
        }
        methods::CREW_LIST => {
            let p: CrewListParams = parse_params_or_default(p)?;
            let all = p.include_removed.unwrap_or(false);
            read(state, move |c| crews::list(c, all)).await
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
            // C1：一次排一层：parentId 给了 = 这个父名下（边上的次序 → 父的 childIds）；缺省 = 顶层机组（crews.position）。
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
        methods::CREW_REMOVE => {
            // 移除机组 = 归档（N1 裁定 13；PM 2026-10-07 定名「移除」）：可逆、什么都不删，不拍快照。
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| crews::archive(tx, &p.crew_id)).await
        }
        methods::CREW_RESTORE => {
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| crews::unarchive(tx, &p.crew_id)).await
        }
        methods::CREW_PURGE => {
            let p: CrewIdParams = parse_params(p)?;
            // N4 / S22：永久删除之前先拍快照（安全网，30 天后随定时备份轮转删掉），前置条件 → 快照 → 删除在同一个库线程任务里；
            // 前置条件不成立（没有 / 总机组 / 没归档 / 有在跑的 session）不拍；快照拍不成就不删。
            let what = p.crew_id.clone();
            let id = p.crew_id.clone();
            to_value(
                state
                    .collab_after_snapshot(
                        what,
                        move |c| crews::check_purgeable(c, &id),
                        move |tx| crews::purge(tx, &p.crew_id).map(|()| Empty {}),
                    )
                    .await
                    .map_err(collab_err)?,
            )
        }
        methods::CHANNEL_LIST => {
            // 写路径：home（总机组 / 顶层机组）的人类群懒建（R1）。
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
            let all = p.include_removed.unwrap_or(false);
            read(state, move |c| {
                sessions::list_for_client(c, p.crew_id.as_deref(), all)
            })
            .await
        }
        methods::SESSION_GET => {
            let p: SessionGetParams = parse_params(p)?;
            read(state, move |c| sessions::get(c, &p.session_id)).await
        }
        methods::CHANNEL_WORK_LIST => {
            let p: ChannelWorkListParams = parse_params(p)?;
            read(state, move |c| {
                crewd_collab::channel_work::list(c, &Sender::Human, p)
            })
            .await
        }
        methods::CHANNEL_WORK_LINK | methods::CHANNEL_WORK_UNLINK => {
            let linked = req.method == methods::CHANNEL_WORK_LINK;
            let p: ChannelWorkLinkParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::channel_work::link(tx, &Sender::Human, p, linked)
            })
            .await
        }
        methods::DOCUMENT_CREATE => {
            let p: DocumentCreateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::documents::create(tx, &Sender::Human, p)
            })
            .await
        }
        methods::DOCUMENT_GET => {
            let p: DocumentGetParams = parse_params(p)?;
            read(state, move |c| {
                crewd_collab::documents::get(c, &Sender::Human, p)
            })
            .await
        }
        methods::DOCUMENT_LIST => {
            let p: DocumentListParams = parse_params(p)?;
            read(state, move |c| {
                crewd_collab::documents::list(c, &Sender::Human, p)
            })
            .await
        }
        methods::DOCUMENT_UPDATE => {
            let p: DocumentUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::documents::update(tx, &Sender::Human, p)
            })
            .await
        }
        methods::DOCUMENT_DELETE => {
            let p: DocumentDeleteParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::documents::delete(tx, &Sender::Human, p)
            })
            .await
        }
        methods::FOLDER_LIST => {
            let p: FolderListParams = parse_params(p)?;
            read(state, move |c| {
                crewd_collab::view_folders::list(c, &Sender::Human, p)
            })
            .await
        }
        methods::FOLDER_CREATE => {
            let p: FolderCreateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::view_folders::create(tx, &Sender::Human, p)
            })
            .await
        }
        methods::FOLDER_UPDATE => {
            let p: FolderUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::view_folders::update(tx, &Sender::Human, p)
            })
            .await
        }
        methods::FOLDER_DELETE => {
            let p: FolderDeleteParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::view_folders::delete(tx, &Sender::Human, p)
            })
            .await
        }
        methods::FOLDER_ASSIGN => {
            let p: FolderAssignParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::view_folders::assign(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_CREATE => {
            let p: WorkCreateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work::create(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_UPDATE => {
            let p: WorkUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work::update(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_BLOCK => {
            let p: WorkBlockParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work_block::block(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_TRANSFER_REQUEST => {
            let p: WorkTransferRequestParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work_transfer::request(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_TRANSFER_ACCEPT => {
            let p: WorkTransferResolveParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work_transfer::accept(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_TRANSFER_WITHDRAW => {
            let p: WorkTransferResolveParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work_transfer::withdraw(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_REASSIGN_LOST => {
            let p: WorkAssignParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work_lost::reassign(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_ASSIGN => {
            let p: WorkAssignParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::work::assign(tx, &Sender::Human, p)
            })
            .await
        }
        methods::WORK_HISTORY => {
            let p: WorkHistoryParams = parse_params(p)?;
            read(state, move |c| crewd_collab::work::history(c, p)).await
        }
        methods::SESSION_USAGE => {
            let p: RunnerUsageParams = parse_params(p)?;
            let limit = p.limit.unwrap_or(50);
            if !(1..=200).contains(&limit) {
                return Err(RpcError::new(
                    RpcError::INVALID_PARAMS,
                    "usage page limit must be 1..200",
                )
                .with_refusal("session.usage_bad_limit"));
            }
            let after = match p.after.as_deref() {
                None => 0,
                Some(v) => match v.parse::<i64>() {
                    Ok(n) if n > 0 && n.to_string() == v => n,
                    _ => {
                        return Err(RpcError::new(
                            RpcError::INVALID_PARAMS,
                            "invalid usage record cursor",
                        )
                        .with_refusal("session.usage_bad_cursor"));
                    }
                },
            };
            read(state, move |c| {
                sessions::get(c, &p.session_id)?;
                Ok(crewd_store::runner_usage::list(
                    c,
                    &p.session_id,
                    after,
                    limit,
                )?)
            })
            .await
        }
        methods::WORK_PATROL => {
            let p: WorkPatrolParams = parse_params_or_default(p)?;
            let now = crewd_store::now_ms();
            read(state, move |c| crewd_collab::work_patrol::read(c, p, now)).await
        }
        methods::WORK_LIST => {
            let p: WorkListParams = parse_params(p)?;
            read(state, move |c| crewd_collab::work::list(c, p)).await
        }
        methods::WORK_GET => {
            let p: WorkGetParams = parse_params(p)?;
            read(state, move |c| crewd_collab::work::get(c, &p.id)).await
        }
        methods::ASK_GET => {
            let p: AskGetParams = parse_params(p)?;
            if p.ask
                .strip_prefix('H')
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|n| *n > 0 && p.ask == format!("H{n}"))
                .is_none()
            {
                return Err(refused(
                    RpcError::INVALID_PARAMS,
                    crewd_collab::refusal::Refusal::new(
                        crewd_collab::refusal::Code::ResourceInvalid,
                    )
                    .arg("field", "ask"),
                ));
            }
            read(state, move |c| asks::get(c, &p.crew_id, &p.ask)).await
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
        methods::TODO_CREATE => {
            let p: TodoCreateParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::requirements::create(tx, &Sender::Human, p)
            })
            .await
        }
        methods::TODO_GET => {
            let p: TodoGetParams = parse_params(p)?;
            read(state, move |c| crewd_collab::requirements::get(c, &p.id)).await
        }
        methods::TODO_CLOSE => {
            let p: TodoCloseParams = parse_params(p)?;
            write(state, move |tx| {
                crewd_collab::requirements::close(tx, &Sender::Human, p)
            })
            .await
        }
        methods::TODO_LIST => {
            let p: TodoListParams = parse_params(p)?;
            read(state, move |c| {
                Ok(crewd_store::todos::list(c, &p.crew_id, p.ledger)?)
            })
            .await
        }
        // M3c：编排的方法（T10b 补充 §2.7）。编排没起来（库里的 `serve()` 不带执行器，C1）= -32009 `orchestra.not_running`，
        // 不是 -32601。
        m if crate::orchestra::rpc::handles(m) => match state.orchestra.get() {
            Some(h) => crate::orchestra::rpc::call(h, state, m, p).await,
            None => Err(refused(
                RpcError::CONFLICT,
                crewd_collab::refusal::Refusal::new(
                    crewd_collab::refusal::Code::OrchestraNotRunning,
                ),
            )),
        },
        other => Err(RpcError::new(
            RpcError::METHOD_NOT_FOUND,
            format!("未知方法: {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn s4_read_owner_dispatch_real_sql_records_once_before_mapping() {
        let fixture = crate::agent::testkit::Fixture::new().await;
        let db = fixture.state.db.clone();
        let health = db.health();
        let control = db
            .call(|s| {
                s.conn()
                    .query_row("SELECT v FROM s4_read_owner_missing_table", [], |row| {
                        row.get::<_, i64>(0)
                    })
            })
            .await;
        let before = health.snapshot();
        let failed = read(&fixture.state, |conn| {
            conn.query_row("SELECT v FROM s4_read_owner_missing_table", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(Into::into)
        })
        .await;
        let observed = health.snapshot();
        let recovered = read(&fixture.state, |conn| {
            conn.query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                .map_err(Into::into)
        })
        .await;
        let later = health.snapshot();
        let closed = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(fixture);
        assert!(
            closed.is_ok(),
            "original owned DB shutdown task completed before semantics"
        );
        assert!(
            matches!(&control, Ok(Err(rusqlite::Error::SqliteFailure(f, _))) if f.extended_code == rusqlite::ffi::SQLITE_ERROR),
            "real raw SQLite missing-table control"
        );
        assert!(
            before.is_none(),
            "plain value-return Db call does not observe nested SQL result"
        );
        assert!(
            matches!(&failed, Err(e) if e.code == RpcError::INTERNAL && e.message.contains('#') && !e.message.contains("s4_read_owner_missing_table")),
            "original numbered generic RPC error stays unchanged"
        );
        assert_eq!(recovered.unwrap(), serde_json::json!(42));
        assert_eq!(
            observed, later,
            "successful FIFO read retains history without another incident"
        );
        // First intended functional target; kind/stage are NOTREACHED if this fails.
        assert_eq!(
            observed.as_ref().map(|f| f.sequence),
            Some(1),
            "one actual owning read SQL failure observed before error mapping"
        );
        let fault = observed.unwrap();
        assert_eq!(fault.stage, crewd_store::db_health::DbFaultStage::Job);
        assert_eq!(fault.kind, crewd_store::db_health::DbFaultKind::Sql);
    }

    #[tokio::test]
    async fn s4_read_owner_dispatch_business_not_found_adds_no_fault() {
        let fixture = crate::agent::testkit::Fixture::new().await;
        let db = fixture.state.db.clone();
        let health = db.health();
        let before = health.snapshot();
        let refused = read(&fixture.state, |conn| {
            crewd_collab::crews::get(conn, "s4-read-owner-no-such-crew")
        })
        .await;
        let after = health.snapshot();
        let recovered = read(&fixture.state, |conn| {
            conn.query_row("SELECT 42", [], |row| row.get::<_, i64>(0))
                .map_err(Into::into)
        })
        .await;
        let later = health.snapshot();
        let closed = tokio::task::spawn_blocking(move || db.shutdown()).await;
        drop(fixture);
        assert!(
            closed.is_ok(),
            "original owned DB shutdown task completed before semantics"
        );
        assert!(
            matches!(&refused, Err(e) if e.code == RpcError::NOT_FOUND),
            "real business NotFound mapping unchanged"
        );
        assert_eq!(recovered.unwrap(), serde_json::json!(42));
        assert!(
            before.is_none(),
            "fresh owned fixture starts without fault history"
        );
        assert_eq!(
            before, after,
            "business refusal does not record a database incident"
        );
        assert_eq!(
            after, later,
            "successful FIFO read does not add a database incident"
        );
    }
    use super::*;
    use crewd_store::StoreError;
    use crewd_work::{AskId, EntryNo, Rejected, Violation, WorkId};

    fn code(e: CollabError) -> i32 {
        collab_err(e).code
    }

    #[test]
    fn work_revision_conflict_exposes_typed_current_facts_in_error_data() {
        let mut r =
            crewd_collab::refusal::Refusal::new(crewd_collab::refusal::Code::WorkRevisionConflict)
                .arg("id", "W9");
        r.current_work = Some(Box::new(crewd_proto::WorkConflictFacts {
            id: "W9".into(),
            revision: "9007199254740993".into(),
            owner: None,
            status: "done".into(),
            next_check_at: None,
            blocker_count: 0,
            note: None,
            note_truncated: false,
            pending_transfer: None,
        }));
        let error = collab_err(CollabError::Conflict(r));
        assert_eq!(error.code, RpcError::CONFLICT);
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(json["data"]["refusal"], "work.revision_conflict");
        assert_eq!(json["data"]["currentWork"]["revision"], "9007199254740993");
        assert_eq!(json["data"]["currentWork"]["status"], "done");
        let old:RpcError=serde_json::from_value(serde_json::json!({"code":RpcError::CONFLICT,"message":"old refusal","data":{"refusal":"work.revision_conflict"}})).unwrap();
        assert!(old.data.unwrap().current_work.is_none());
    }

    #[test]
    fn a_refusal_carries_its_code_to_the_client() {
        use crewd_collab::refusal::{Code, Refusal};
        let e = collab_err(CollabError::Invalid(Refusal::new(
            Code::ChannelDmCannotGrow,
        )));
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        assert_eq!(
            e.data.as_ref().map(|d| d.refusal.as_str()),
            Some("channel.dm_cannot_grow")
        );
        // 其余错误（账本 / 存储 / 内部）不带码：data 缺席，线上不出现这个键。
        let e = collab_err(CollabError::Work(Rejected::Blank("text")));
        assert!(e.data.is_none());
        let wire = serde_json::to_value(&e).unwrap();
        assert!(wire.get("data").is_none(), "{wire}");
        // 每个规则变体都带码，状态码对得上。
        for (e, want) in [
            (
                CollabError::NotFound(Refusal::new(Code::MessageEmpty)),
                RpcError::NOT_FOUND,
            ),
            (
                CollabError::Forbidden(Refusal::new(Code::MessageEmpty)),
                RpcError::FORBIDDEN,
            ),
            (
                CollabError::Conflict(Refusal::new(Code::MessageEmpty)),
                RpcError::CONFLICT,
            ),
        ] {
            let r = collab_err(e);
            assert_eq!(r.code, want);
            assert_eq!(r.data.unwrap().refusal, "message.empty");
        }
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

    /// S26：存储故障（磁盘满、I/O、损坏、只读、打不开）有自己的码和一句人话，不混进 -32603；
    /// 原文（路径、SQL）照样只进日志。
    #[test]
    fn storage_failures_get_their_own_code_and_a_plain_reason() {
        use crewd_store::StorageKind;
        for (rc, kind) in [
            (rusqlite::ffi::SQLITE_FULL, StorageKind::DiskFull),
            (rusqlite::ffi::SQLITE_IOERR_WRITE, StorageKind::Io),
            (rusqlite::ffi::SQLITE_CORRUPT, StorageKind::Corrupt),
            (rusqlite::ffi::SQLITE_READONLY, StorageKind::ReadOnly),
            (rusqlite::ffi::SQLITE_CANTOPEN, StorageKind::CantOpen),
        ] {
            let e = StoreError::from(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rc),
                Some("/secret/path/crewd.db".into()),
            ));
            let r = collab_err(CollabError::Store(e));
            assert_eq!(r.code, RpcError::STORAGE, "{kind:?}");
            assert!(r.message.contains(kind.describe()), "{}", r.message);
            assert!(!r.message.contains("secret"), "{}", r.message);
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
    mod s4_health;
}

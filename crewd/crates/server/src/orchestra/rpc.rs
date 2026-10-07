//! M3c 的客户端方法（spec §22.4「客户端 RPC（M3c 新增）」，T10b 补充 §2.1、§2.4、§2.7）。
//!
//! 错误（C14 / S26）：编排的 `ControlError` 一律带拒绝码（`data.refusal`，客户端按码本地化）；内部错误 = -32603 带编号、
//! 原文只进日志；存储故障 = -32010。
use crewd_proto::RpcError;

use super::core::{ControlError, RefusedKind};
use crate::dispatch;

/// 编排的错误 → RPC 错误（T10b 补充 §2.4）。
pub fn rpc_err(e: ControlError) -> RpcError {
    let code = match &e {
        ControlError::Internal(detail) => return dispatch::internal(detail),
        ControlError::Storage { kind, detail } => return dispatch::storage(*kind, detail),
        ControlError::NotFound(_) => RpcError::NOT_FOUND,
        ControlError::Invalid(_) => RpcError::INVALID_PARAMS,
        ControlError::Refused { kind, .. } => match kind {
            RefusedKind::NotFound => RpcError::NOT_FOUND,
            RefusedKind::Invalid => RpcError::INVALID_PARAMS,
            RefusedKind::Forbidden => RpcError::FORBIDDEN,
            RefusedKind::Conflict => RpcError::CONFLICT,
        },
        ControlError::AlreadyRunning(_)
        | ControlError::CaptainTaken(_)
        | ControlError::NotRunning(_)
        | ControlError::AtCap { .. }
        | ControlError::ShuttingDown
        | ControlError::NoWorkdir(_)
        | ControlError::Worktree(_)
        | ControlError::CrewRemoved(_)
        | ControlError::Unsupported(_) => RpcError::CONFLICT,
    };
    match e.refusal() {
        Some(r) => dispatch::refused(code, r),
        // 到不了：除了内部 / 存储错误，编排的错误都带拒绝码（`refusal` 的定义）。
        None => dispatch::internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::core::{Cmd, Origin, StartSession};
    use crate::orchestra::testkit::Rig;
    use crewd_collab::refusal::{Code, Refusal};
    use crewd_store::StorageKind;

    /// 每个变体 → (错误号, 拒绝码)；文字缺省是中文、`data.refusal` 是码（T10b 补充 §2.4）。
    #[test]
    fn every_control_error_maps_to_its_code_and_refusal() {
        let s = || "s-1".to_string();
        let refused = |kind| ControlError::Refused {
            kind,
            refusal: Refusal::new(Code::SessionTitleTooLong)
                .arg("max", "18")
                .arg("actual", "30"),
        };
        let table = [
            (ControlError::NotFound(s()), -32004, "session.not_found"),
            (
                ControlError::AlreadyRunning(s()),
                -32009,
                "session.already_running",
            ),
            (ControlError::NotRunning(s()), -32009, "session.not_running"),
            (ControlError::AtCap { max: 3 }, -32009, "session.at_cap"),
            (
                ControlError::Unsupported("codex".into()),
                -32009,
                "session.executor_unavailable",
            ),
            (ControlError::NoWorkdir(s()), -32009, "session.no_workdir"),
            (
                ControlError::Worktree("合成原因".into()),
                -32009,
                "session.worktree_failed",
            ),
            (
                ControlError::CaptainTaken("c-1".into()),
                -32009,
                "session.captain_exists",
            ),
            (
                ControlError::Invalid(Refusal::new(Code::OrchestraNudgeEmpty)),
                -32602,
                "orchestra.nudge_empty",
            ),
            (
                ControlError::ShuttingDown,
                -32009,
                "orchestra.shutting_down",
            ),
            (
                ControlError::CrewRemoved("c-1".into()),
                -32009,
                "crew.removed",
            ),
            (
                refused(RefusedKind::NotFound),
                -32004,
                "session.title_too_long",
            ),
            (
                refused(RefusedKind::Invalid),
                -32602,
                "session.title_too_long",
            ),
            (
                refused(RefusedKind::Forbidden),
                -32003,
                "session.title_too_long",
            ),
            (
                refused(RefusedKind::Conflict),
                -32009,
                "session.title_too_long",
            ),
        ];
        for (e, code, refusal) in table {
            let r = rpc_err(e.clone());
            assert_eq!(r.code, code, "{e:?}");
            assert_eq!(
                r.data.as_ref().map(|d| d.refusal.as_str()),
                Some(refusal),
                "{e:?}"
            );
            assert_eq!(r.message, e.to_string(), "message = 中文缺省文字");
            assert!(!r.message.contains('{'), "占位符都填了：{}", r.message);
        }
    }

    /// C14 / S26：内部错误 = -32603 带编号、原文不上线；存储故障 = -32010 只说类别。两者都不带拒绝码。
    #[test]
    fn internal_and_storage_errors_keep_their_detail_off_the_wire() {
        let r = rpc_err(ControlError::Internal("合成 secret 原文".into()));
        assert_eq!(r.code, RpcError::INTERNAL);
        assert!(
            !r.message.contains("secret") && r.message.contains('#'),
            "{}",
            r.message
        );
        assert!(r.data.is_none());
        let r = rpc_err(ControlError::Storage {
            kind: StorageKind::DiskFull,
            detail: "/secret/crewd.db".into(),
        });
        assert_eq!(r.code, RpcError::STORAGE);
        assert!(
            r.message.contains(StorageKind::DiskFull.describe()),
            "{}",
            r.message
        );
        assert!(!r.message.contains("secret"), "{}", r.message);
        assert!(r.data.is_none());
    }

    /// S26：Port 读出的存储故障（MemPort 注入一个 `StoreError::Storage`）经 `from_port` 成为 `Storage`，RPC 上 -32010。
    #[tokio::test]
    async fn a_storage_failure_from_the_port_reaches_the_client_as_storage() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port.lock().storage_error = Some(StorageKind::ReadOnly);
        let e = rig
            .cmd(|reply| Cmd::Stop {
                session: "cap".into(),
                reply,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(
                e,
                ControlError::Storage {
                    kind: StorageKind::ReadOnly,
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(rpc_err(e).code, RpcError::STORAGE);
    }

    /// T10a 遗留 2：写 session 行时 collab 的规则拒绝（标题太长）原样带码到客户端，不再是一个内部错误。
    #[tokio::test]
    async fn a_refused_session_row_keeps_its_collab_code() {
        use crate::orchestra::clock::ManualClock;
        use crate::orchestra::db::fixture::seeded;
        use crate::orchestra::exec::Executors;
        use crate::orchestra::hooks::Hooks;
        use crate::orchestra::testkit::StandIn;
        use crate::orchestra::{OrchestraConfig, spawn};
        use std::sync::Arc;
        let (_state, port, _d) = seeded().await;
        let stand_in = StandIn::default();
        let (h, _actor) = spawn(
            Arc::new(port),
            Executors::new().with("claude", Arc::new(stand_in.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(crewd_wake::Ms(1_000_000_000))),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        let req = StartSession {
            crew_id: "c-1".into(),
            captain: false,
            executor: "claude".into(),
            brief: "合成任务".into(),
            title: Some("合成".repeat(15)),
            model: None,
            effort: None,
            isolation: false,
        };
        let e = h.start(req, Origin::Human).await.unwrap_err();
        assert!(
            matches!(
                &e,
                ControlError::Refused {
                    kind: RefusedKind::Invalid,
                    refusal,
                } if refusal.code == Code::SessionTitleTooLong
            ),
            "{e:?}"
        );
        let r = rpc_err(e);
        assert_eq!(r.code, RpcError::INVALID_PARAMS);
        assert_eq!(r.data.unwrap().refusal, "session.title_too_long");
        assert!(stand_in.lock().launches.is_empty(), "什么都没起");
    }

    /// 编排自己拒的（同一个 `ControlError` 经 anyhow 回来）原样带出，不变成内部错误。
    #[test]
    fn from_port_keeps_an_orchestra_refusal_and_wraps_the_rest() {
        let e = ControlError::from_port(anyhow::Error::new(ControlError::AlreadyRunning(
            "s-1".into(),
        )));
        assert_eq!(e, ControlError::AlreadyRunning("s-1".into()));
        let e = ControlError::from_port(anyhow::anyhow!("合成：别的错"));
        assert!(matches!(e, ControlError::Internal(_)), "{e:?}");
        let e = ControlError::from_port(
            anyhow::Error::new(crewd_collab::CollabError::Conflict(
                Refusal::new(Code::WakeBadTransition)
                    .arg("wake", "1")
                    .arg("from", "dropped")
                    .arg("to", "inflight"),
            ))
            .context("合成：外面再包一层"),
        );
        assert!(
            matches!(
                &e,
                ControlError::Refused {
                    kind: RefusedKind::Conflict,
                    ..
                }
            ),
            "沿错误链找到 collab 的拒绝：{e:?}"
        );
    }
}

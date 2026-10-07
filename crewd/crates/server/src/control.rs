//! 会话控制的窄接口（spec §22.5）：M3c 的 `server::orchestra::Sessions` 实现它；M3b 的工具、M3a 的 RPC 经它调。
//! 参数一律 owned、返回 `BoxFuture`，可做 `dyn`。`NoSessions` 是 M3c 接上之前的默认实现（一律 Unsupported）。
use crewd_proto::{Role, Session, SessionLive};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartRequest {
    pub crew_id: String,
    pub role: Role,
    /// "claude" | "codex" | "acp:<id>"
    pub executor: String,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// 控制者裁定 C8：true = 在机组工作目录里新建 git worktree（M3c 建）；false = 机组共享目录。
    pub isolation: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inspection {
    pub live: SessionLive,
    /// 屏幕文本（claude）或结构化记录的尾部（codex）。
    pub screen: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaLine {
    pub executor: String,
    pub plan: Option<String>,
    /// 额度用尽时的重置时刻（RFC 3339）。
    pub limited_until: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaReport {
    pub lines: Vec<QuotaLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    #[error("{0}")]
    NotFound(String),
    #[error("session {0} 已经在跑")]
    AlreadyRunning(String),
    #[error("不支持：{0}")]
    Unsupported(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}

pub trait SessionControl: Send + Sync {
    /// 注册 session 行（经 collab）并起进程；返回注册后的行。
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>>;
    /// `--resume` 一个已有 session（重启询问答「恢复」、`session.resume`）。
    fn resume(&self, session_id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn stop(&self, session_id: String, reason: String) -> BoxFuture<'_, Result<(), ControlError>>;
    /// 给在跑的 session 投一段文字（机长的 nudge_session）。
    fn nudge(&self, session_id: String, text: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn inspect(
        &self,
        session_id: String,
        max_lines: usize,
    ) -> BoxFuture<'_, Result<Inspection, ControlError>>;
    fn set_profile(
        &self,
        session_id: String,
        model: Option<String>,
        effort: Option<String>,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    /// 交接机长：数据（`collab::crews::set_captain`）+ 两边的世界观 / 工具表刷新。
    fn handoff_captain(
        &self,
        crew_id: String,
        to_session_id: String,
    ) -> BoxFuture<'_, Result<(), ControlError>>;
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>>;
    fn live(&self, session_id: String) -> BoxFuture<'_, Result<SessionLive, ControlError>>;
}

/// M3c 接上之前的默认实现。
pub struct NoSessions;

fn unsupported<T: Send + 'static>() -> BoxFuture<'static, Result<T, ControlError>> {
    Box::pin(async { Err(ControlError::Unsupported("会话管理还没接上（M3c）".into())) })
}

impl SessionControl for NoSessions {
    fn start(&self, _: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>> {
        unsupported()
    }
    fn resume(&self, _: String) -> BoxFuture<'_, Result<(), ControlError>> {
        unsupported()
    }
    fn stop(&self, _: String, _: String) -> BoxFuture<'_, Result<(), ControlError>> {
        unsupported()
    }
    fn nudge(&self, _: String, _: String) -> BoxFuture<'_, Result<(), ControlError>> {
        unsupported()
    }
    fn inspect(&self, _: String, _: usize) -> BoxFuture<'_, Result<Inspection, ControlError>> {
        unsupported()
    }
    fn set_profile(
        &self,
        _: String,
        _: Option<String>,
        _: Option<String>,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        unsupported()
    }
    fn handoff_captain(&self, _: String, _: String) -> BoxFuture<'_, Result<(), ControlError>> {
        unsupported()
    }
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>> {
        unsupported()
    }
    fn live(&self, _: String) -> BoxFuture<'_, Result<SessionLive, ControlError>> {
        unsupported()
    }
}

/// 测试替身（M3b 的工具测试、M3a 的 RPC 测试用）：记下每次调用，按预设回答。不碰库、不起进程。
#[derive(Debug, Clone, PartialEq)]
pub enum FakeCall {
    Start(StartRequest),
    Resume(String),
    Stop(String, String),
    Nudge(String, String),
    Inspect(String, usize),
    SetProfile(String, Option<String>, Option<String>),
    Handoff(String, String),
    Quota,
    Live(String),
}

/// Which SessionControl method a `FakeCall` is (for `FakeSessionControl::fail_method`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeMethod {
    Start,
    Resume,
    Stop,
    Nudge,
    Inspect,
    SetProfile,
    Handoff,
    Quota,
    Live,
}

impl FakeCall {
    pub fn method(&self) -> FakeMethod {
        match self {
            Self::Start(_) => FakeMethod::Start,
            Self::Resume(_) => FakeMethod::Resume,
            Self::Stop(..) => FakeMethod::Stop,
            Self::Nudge(..) => FakeMethod::Nudge,
            Self::Inspect(..) => FakeMethod::Inspect,
            Self::SetProfile(..) => FakeMethod::SetProfile,
            Self::Handoff(..) => FakeMethod::Handoff,
            Self::Quota => FakeMethod::Quota,
            Self::Live(_) => FakeMethod::Live,
        }
    }
}

#[derive(Default)]
pub struct FakeSessionControl {
    calls: Mutex<Vec<FakeCall>>,
    live: Mutex<std::collections::HashMap<String, SessionLive>>,
    fail_next: Mutex<Option<ControlError>>,
    fail_method: Mutex<Option<(FakeMethod, ControlError)>>,
}

impl FakeSessionControl {
    pub fn calls(&self) -> Vec<FakeCall> {
        self.calls.lock().unwrap().clone()
    }
    pub fn set_live(&self, l: SessionLive) {
        self.live.lock().unwrap().insert(l.session_id.clone(), l);
    }
    /// 下一次调用（任何方法）返回这个错误。
    pub fn fail_next(&self, e: ControlError) {
        *self.fail_next.lock().unwrap() = Some(e);
    }
    /// The next call of `method` (only that method; other calls go through) returns this error.
    pub fn fail_method(&self, method: FakeMethod, e: ControlError) {
        *self.fail_method.lock().unwrap() = Some((method, e));
    }
    fn record<T: Send + 'static>(
        &self,
        c: FakeCall,
        ok: T,
    ) -> BoxFuture<'_, Result<T, ControlError>> {
        let method = c.method();
        self.calls.lock().unwrap().push(c);
        let by_method = {
            let mut slot = self.fail_method.lock().unwrap();
            match slot.take() {
                Some((m, e)) if m == method => Some(e),
                other => {
                    *slot = other;
                    None
                }
            }
        };
        let r = match by_method.or_else(|| self.fail_next.lock().unwrap().take()) {
            Some(e) => Err(e),
            None => Ok(ok),
        };
        Box::pin(async move { r })
    }
}

impl SessionControl for FakeSessionControl {
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>> {
        let n = self.calls.lock().unwrap().len();
        let s = Session {
            id: format!("fake-{n}"),
            crew_id: req.crew_id.clone(),
            role: req.role,
            executor: req.executor.clone(),
            title: req.title.clone(),
            brief: Some(req.brief.clone()),
            desired: crewd_proto::Desired::Running,
            // C8：假实现不建真 worktree，只给个可断言的假路径。
            worktree: req.isolation.then(|| format!("/fake/worktree-{n}")),
            ..Default::default()
        };
        self.record(FakeCall::Start(req), s)
    }
    fn resume(&self, id: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.record(FakeCall::Resume(id), ())
    }
    fn stop(&self, id: String, reason: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.record(FakeCall::Stop(id, reason), ())
    }
    fn nudge(&self, id: String, text: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.record(FakeCall::Nudge(id, text), ())
    }
    fn inspect(
        &self,
        id: String,
        max_lines: usize,
    ) -> BoxFuture<'_, Result<Inspection, ControlError>> {
        let live = self
            .live
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or(SessionLive {
                session_id: id.clone(),
                ..Default::default()
            });
        self.record(
            FakeCall::Inspect(id, max_lines),
            Inspection {
                live,
                screen: String::new(),
            },
        )
    }
    fn set_profile(
        &self,
        id: String,
        model: Option<String>,
        effort: Option<String>,
    ) -> BoxFuture<'_, Result<(), ControlError>> {
        self.record(FakeCall::SetProfile(id, model, effort), ())
    }
    fn handoff_captain(&self, crew: String, to: String) -> BoxFuture<'_, Result<(), ControlError>> {
        self.record(FakeCall::Handoff(crew, to), ())
    }
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>> {
        self.record(FakeCall::Quota, QuotaReport::default())
    }
    fn live(&self, id: String) -> BoxFuture<'_, Result<SessionLive, ControlError>> {
        let l = self
            .live
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or(SessionLive {
                session_id: id.clone(),
                ..Default::default()
            });
        self.record(FakeCall::Live(id), l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn fake_records_calls_and_can_fail_once() {
        let fake = Arc::new(FakeSessionControl::default());
        let dynamic: Arc<dyn SessionControl> = fake.clone();
        let s = dynamic
            .start(StartRequest {
                crew_id: "crew-a".into(),
                executor: "claude".into(),
                brief: "修滚动".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s.crew_id, "crew-a");
        assert!(s.worktree.is_none(), "isolation=false → 共享机组目录");
        let iso = dynamic
            .start(StartRequest {
                crew_id: "crew-a".into(),
                executor: "claude".into(),
                brief: "独立改".into(),
                isolation: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            iso.worktree.is_some(),
            "isolation=true → session 行带 worktree 路径"
        );
        fake.fail_next(ControlError::AlreadyRunning("w-1".into()));
        assert_eq!(
            dynamic.resume("w-1".into()).await,
            Err(ControlError::AlreadyRunning("w-1".into()))
        );
        dynamic.resume("w-1".into()).await.unwrap();
        fake.set_live(SessionLive {
            session_id: "w-1".into(),
            running: true,
            ..Default::default()
        });
        assert!(dynamic.live("w-1".into()).await.unwrap().running);
        assert_eq!(fake.calls().len(), 5);
        assert!(matches!(fake.calls()[2], FakeCall::Resume(ref id) if id == "w-1"));
    }

    #[tokio::test]
    async fn no_sessions_says_unsupported() {
        let n: Arc<dyn SessionControl> = Arc::new(NoSessions);
        assert!(matches!(n.quota().await, Err(ControlError::Unsupported(_))));
    }
}

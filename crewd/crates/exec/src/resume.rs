//! 重启后接回原对话（spec §13 Q12：重启/升级只要求能恢复）。
//!
//! 会话号账本只回答一个问题：这个 crewd session 上次用的 claude 会话号是什么。
//! 记过就 `--resume`（不预判它还在不在——claude 按会话号全盘找，预判只会挡掉能接上的）；
//! 没记过就生成新号，**起进程前先落账**。claude 拒绝续跑由驱动层从屏幕认出，换新号重起一次。
//!
//! 执行器不依赖存储：账本是个 trait，crewd 用 SQLite 实现（`crewd_server::ledger`），
//! 测试与不落库的命令行用 [`MemoryLedger`]。以后执行器拆成独立进程，这里换成一次 RPC。
use std::io;
use std::sync::Mutex;

use crate::launch::{AgentSessionArg, new_agent_session_id};

pub trait AgentIdLedger: Send + Sync {
    /// 上次记下的 claude 会话号。
    fn recorded(&self) -> io::Result<Option<String>>;
    /// 落账。失败 = 不起进程（起了也接不回来）。
    fn record(&self, agent_session_id: &str) -> io::Result<()>;
}

/// 只在内存里的账本。
#[derive(Debug, Default)]
pub struct MemoryLedger(Mutex<Option<String>>);

impl MemoryLedger {
    pub fn with(id: Option<&str>) -> Self {
        Self(Mutex::new(id.map(str::to_string)))
    }

    pub fn current(&self) -> Option<String> {
        self.0.lock().expect("账本锁中毒").clone()
    }
}

impl AgentIdLedger for MemoryLedger {
    fn recorded(&self) -> io::Result<Option<String>> {
        Ok(self.current())
    }

    fn record(&self, agent_session_id: &str) -> io::Result<()> {
        *self.0.lock().expect("账本锁中毒") = Some(agent_session_id.to_string());
        Ok(())
    }
}

/// 记过（去空白后非空）就续跑，否则新号。
pub fn decide_resume(recorded: Option<&str>) -> AgentSessionArg {
    match recorded.map(str::trim).filter(|id| !id.is_empty()) {
        Some(id) => AgentSessionArg::Resume(id.to_string()),
        None => AgentSessionArg::New(new_agent_session_id()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_id_always_resumes_without_predicting() {
        assert_eq!(
            decide_resume(Some(" abc ")),
            AgentSessionArg::Resume("abc".into())
        );
    }

    #[test]
    fn no_or_blank_record_starts_fresh_with_a_new_id() {
        for recorded in [None, Some(""), Some("  ")] {
            match decide_resume(recorded) {
                AgentSessionArg::New(id) => assert_eq!(id.len(), 36),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn memory_ledger_round_trips() {
        let l = MemoryLedger::default();
        assert_eq!(l.recorded().unwrap(), None);
        l.record("x").unwrap();
        assert_eq!(l.recorded().unwrap().as_deref(), Some("x"));
        assert_eq!(
            MemoryLedger::with(Some("y")).current().as_deref(),
            Some("y")
        );
    }
}

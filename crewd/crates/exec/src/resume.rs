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

pub use crate::launch::valid_agent_session_id;
use crate::launch::{AgentSessionArg, new_agent_session_id};

pub trait AgentIdLedger: Send + Sync {
    /// 上次记下的 claude 会话号。
    fn recorded(&self) -> io::Result<Option<String>>;
    /// 落账。失败 = 不起进程（起了也接不回来）。
    /// 实现必须拒绝 [`valid_agent_session_id`] 不认的号（`InvalidInput`）。
    fn record(&self, agent_session_id: &str) -> io::Result<()>;
}

/// 落账前的统一检查，各实现共用。
pub fn check_agent_session_id(id: &str) -> io::Result<()> {
    if valid_agent_session_id(id) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("非法的会话号 {id:?}"),
        ))
    }
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
        check_agent_session_id(agent_session_id)?;
        *self.0.lock().expect("账本锁中毒") = Some(agent_session_id.to_string());
        Ok(())
    }
}

/// [`decide_resume`] 的结论。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeDecision {
    pub arg: AgentSessionArg,
    /// 账里有号但不能用（非法）而被忽略时的原因，调用方应记日志；没记过 / 正常续跑为 `None`。
    pub ignored: Option<String>,
}

/// 记过（去空白后非空且合法）就续跑，否则新号。账里的号非法 = 当作没记过，并说明原因。
pub fn decide_resume(recorded: Option<&str>) -> ResumeDecision {
    let fresh = |ignored| ResumeDecision {
        arg: AgentSessionArg::New(new_agent_session_id()),
        ignored,
    };
    match recorded.map(str::trim).filter(|id| !id.is_empty()) {
        None => fresh(None),
        Some(id) if valid_agent_session_id(id) => ResumeDecision {
            arg: AgentSessionArg::Resume(id.to_string()),
            ignored: None,
        },
        Some(id) => {
            let shown: String = id.chars().take(64).collect();
            fresh(Some(format!(
                "账里记的会话号非法，已忽略并换新号：{shown:?}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_id_always_resumes_without_predicting() {
        let d = decide_resume(Some(" abc "));
        assert_eq!(d.arg, AgentSessionArg::Resume("abc".into()));
        assert_eq!(d.ignored, None);
    }

    #[test]
    fn no_or_blank_record_starts_fresh_with_a_new_id() {
        for recorded in [None, Some(""), Some("  ")] {
            let d = decide_resume(recorded);
            match d.arg {
                AgentSessionArg::New(id) => assert_eq!(id.len(), 36),
                other => panic!("{other:?}"),
            }
            assert_eq!(d.ignored, None, "没记过不是异常");
        }
    }

    #[test]
    fn invalid_recorded_id_is_ignored_with_a_reason() {
        for bad in ["../../etc/x", "a/b", "x\0", "-rf", "has space"] {
            let d = decide_resume(Some(bad));
            match d.arg {
                AgentSessionArg::New(id) => assert!(valid_agent_session_id(&id)),
                other => panic!("{bad:?} -> {other:?}"),
            }
            assert!(d.ignored.is_some(), "{bad:?} 要给出忽略原因");
        }
    }

    #[test]
    fn memory_ledger_rejects_invalid_ids() {
        let l = MemoryLedger::default();
        for bad in ["../x", "a/b", "x\0", "", "-rf"] {
            let e = l.record(bad).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
        assert_eq!(l.current(), None);
        l.record("0199c1a2-7b3e-7c4d-8e5f-0123456789ab").unwrap();
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

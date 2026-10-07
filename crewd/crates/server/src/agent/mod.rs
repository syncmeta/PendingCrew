//! agent 面（spec §22.3）：`crewd mcp` / `crewd hook` 在 serve 里的另一半。
//! 给 agent 看的文字全在 `crates/collab/prompts/{tools,world-model}.*.md`；本模块的非测试代码里
//! 不写中文字面量（`literals` 测试钉住），给开发者看的错误写英文短句。
pub mod approval;
pub mod catalog;
pub mod inject;
pub mod port;
pub mod rpc;
pub mod runtime;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod text;
pub mod tools;
pub mod turn;

mod literals;

pub use runtime::{AgentConfig, AgentRuntime};

pub use crewd_proto::Role;

/// 一个 attach 过的 session 此刻的身份。每次调用现查（角色会变：换机长）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub session_id: String,
    pub crew_id: String,
    pub role: Role,
    /// 群里的显示名（session 的 title / display_name，没有就是 id）。
    pub label: String,
    /// 所在机组是内建总机组（`crews.builtin`）。
    pub builtin_crew: bool,
}

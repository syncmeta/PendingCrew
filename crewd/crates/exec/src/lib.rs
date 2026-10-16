//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod acp;
pub mod codex;
pub mod delivery;
pub mod driver;
pub mod env;
pub mod health;
pub mod launch;
pub mod pty;
pub mod registry;
pub mod resume;
pub mod session;

// owned_stdio.rs is retained as historical native-worker source, not linked.

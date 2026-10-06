//! crewd 的执行器：起 agent 进程（PTY）、读它的屏幕、投递正文、认健康状态、收尸与恢复。
//! 其余部分只经 `driver::SessionHandle` 这一个窄接口与它交互（spec §13 Q12）。
pub mod env;
pub mod launch;

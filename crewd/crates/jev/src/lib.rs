//! Jev（TypeSafe System One）客户端：机长的一部分，替机长做高频小判断（spec §7）。
//! 置信度够 → `Outcome::Act` 直接执行；不够 / 没配置 / 出错 → `Outcome::Defer` 交机长。
//! 只支持 unix：key 文件与诊断日志靠 unix 权限位与属主保密。
#[cfg(not(unix))]
compile_error!("crewd-jev 只支持 unix（key 文件与诊断日志依赖 std::os::unix 的权限与属主）");
pub mod client;
pub mod decisions;
pub mod diag;
pub mod error;
pub mod eval;
pub mod jev;
pub mod key;
pub mod model;
pub mod prompts;
pub mod scrub;
pub mod shuffle;

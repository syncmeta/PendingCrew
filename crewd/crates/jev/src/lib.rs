//! Jev（TypeSafe System One）客户端：机长的一部分，替机长做高频小判断（spec §7）。
//! 置信度够 → `Outcome::Act` 直接执行；不够 / 没配置 / 出错 → `Outcome::Defer` 交机长。
pub mod client;
pub mod decisions;
pub mod error;
pub mod key;
pub mod model;
pub mod prompts;
pub mod scrub;
pub mod shuffle;

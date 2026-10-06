//! crewd 的唤醒判定（spec §22.4、§22.7）：谁醒、能不能醒、什么时候醒、醒来看到的附言。
//! sans-IO：不起进程、不碰 tokio / 文件 / 数据库、不读时钟——时间由调用方传入（同 crewd-work）。
pub mod breaker;
pub mod model;
pub mod plan;
pub mod policy;
pub mod queue;
pub mod text;

pub use model::*;

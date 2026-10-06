//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod perm;
pub mod progress;
#[cfg(test)]
mod testkit;

//! codex 执行器：`codex app-server`（stdio JSON-RPC）客户端。
//!
//! 与 claude 驱动共用窄接口（`driver::SessionHandle` + `session::SessionEvent`）、
//! 环境白名单、孤儿登记表与会话号账本；没有终端，「屏幕文本」由结构化记录代替。
pub mod rpc;

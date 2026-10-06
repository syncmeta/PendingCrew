//! 占位：M3b 换成真正的 `AgentRuntime`（必须保留 `Default`，`AppState::new` 用它）。
/// 花括号空结构而不是单元结构：`AppState::new` 写 `AgentRuntime::default()`（M3b 的真类型照用），
/// 单元结构会被 clippy 的 `default_constructed_unit_structs` 拦下。
#[derive(Debug, Default)]
pub struct AgentRuntime {}

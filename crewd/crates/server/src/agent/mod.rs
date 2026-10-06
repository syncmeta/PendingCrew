//! agent 面（M3b）。M3a 只放占位，让 `AppState.agent` 有类型；M3b 接手整个目录。
pub mod runtime;
pub use runtime::AgentRuntime;

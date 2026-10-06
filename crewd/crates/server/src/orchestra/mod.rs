//! 编排（M3c）。M3a 只放占位，让 `AppState.orchestra` 有类型；M3c 接手整个目录（真 `Handle` 必须是 `Send + Sync`）。
#[derive(Debug, Clone)]
pub struct Handle;

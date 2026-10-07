//! serve 里 agent 面的配置（serve 启动段由 M3c 填；缺省值见 Default）。整文件替换 M3a 的占位。

use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// 一次注入的字符上限（官方 additionalContext 上限 10,000，留余量）。
    pub inject_cap_chars: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            inject_cap_chars: 9_000,
        }
    }
}

#[derive(Debug, Default)]
pub struct AgentRuntime {
    pub config: AgentConfig,
    /// One async mutex per session: two post-tool-use hooks of the same session (parallel tool
    /// calls) must not read the same unread messages. Entries are tiny and live as long as serve.
    locks: std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AgentRuntime {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            locks: Default::default(),
        }
    }

    /// The injection lock of one session (held across `inject::prepare` + `commit`).
    pub fn session_lock(&self, session: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(session.to_string()).or_default().clone()
    }
}

//! 领域模型：客户端看到的机组、session、消息、Todo。字段名即线上 JSON（camelCase）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub server: String,
    pub version: String,
    pub api_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    pub builtin: bool,
    pub parent_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captain_agent_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub crew_id: String,
    /// "captain" | "worker"
    pub role: String,
    /// "claude_code" | "codex" | "unknown"
    pub agent_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Mention {
    /// "session" | "captain" | "human" | "broadcast"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub mime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// 全库单调递增的序号；客户端用它增量拉取。
    pub seq: i64,
    pub id: String,
    pub crew_id: String,
    /// "user" | "session" | "captain" | "pendingcrew" | 旧数据里的 "human"
    pub sender_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Ledger {
    Agent,
    Human,
}

impl Ledger {
    pub fn as_str(self) -> &'static str {
        match self {
            Ledger::Agent => "agent",
            Ledger::Human => "human",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoResponse {
    pub id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Todo {
    pub id: String,
    pub crew_id: String,
    pub ledger: Ledger,
    /// 账内编号 #N；软删除的行也占号。
    pub number: i64,
    pub text: String,
    /// "pending" | "in_progress" | "completed" | "blocked_on_human" | "dropped"
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub responses: Vec<TodoResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardListParams {
    pub crew_id: String,
    /// 只返回 seq 大于它的消息；缺省 0。
    #[serde(default)]
    pub after_seq: Option<i64>,
    /// 缺省 200，上限 1000。
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardPostParams {
    pub crew_id: String,
    pub text: String,
    pub sender_kind: String,
    #[serde(default)]
    pub sender_session_id: Option<String>,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoListParams {
    pub crew_id: String,
    pub ledger: Ledger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    /// 主题 = 事件名第一个点之前的部分，例如 "whiteboard"。
    pub topics: Vec<String>,
}

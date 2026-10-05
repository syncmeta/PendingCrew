//! 旧 Swift Codable 的 JSON 形状（只读）。键名 = Swift 属性名（camelCase），
//! 例外：mention 的 `target_id`。未知键全部落进 `extra`，导入时原样存进对应行的 extra 列；
//! 嵌套对象（mention / attachment / todo response）没有 extra 列，它们带未知键时整份文件原样归档。
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LCrew {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub is_builtin: bool,
    pub crew_number: Option<i64>,
    pub working_directory: Option<String>,
    pub captain_agent_kind: Option<String>,
    #[serde(default)]
    pub parent_crew_ids: Vec<String>,
    #[serde(default)]
    pub session_members: Vec<LMember>,
    pub manually_hidden_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LMember {
    pub session_id: String,
    pub display_name: String,
    pub created_at: String,
    /// 例如 `extensionNumber`。导入时并进对应 session 的 `extra.member`。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LAgentSession {
    pub crew_id: String,
    pub session_id: String,
    pub kind: String,
    pub agent_session_id: String,
    pub updated_at: String,
    pub working_directory: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct LMention {
    pub kind: String,
    pub target_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct LAttachment {
    pub id: String,
    pub mime: String,
    pub size: Option<i64>,
    pub path: String,
    pub filename: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LMessage {
    pub id: String,
    pub sender_kind: String,
    pub text: String,
    pub created_at: String,
    pub sender_session_id: Option<String>,
    pub sender_name: Option<String>,
    pub category: Option<String>,
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub mentions: Vec<LMention>,
    #[serde(default)]
    pub attachments: Vec<LAttachment>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LTodoResponse {
    pub id: String,
    pub session_id: String,
    pub text: String,
    pub created_at: String,
    pub sender_name: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub attachments: Vec<LAttachment>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LTodo {
    pub id: String,
    pub number: i64,
    pub text: String,
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub responses: Vec<LTodoResponse>,
    pub updated_at: Option<String>,
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<LAttachment>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl LMessage {
    /// 嵌套对象里有没建模、也没有 extra 列可放的键。
    pub fn has_nested_extra(&self) -> bool {
        self.mentions.iter().any(|x| !x.extra.is_empty())
            || self.attachments.iter().any(|a| !a.extra.is_empty())
    }
}

impl LTodo {
    /// 嵌套对象里有没建模、也没有 extra 列可放的键。
    pub fn has_nested_extra(&self) -> bool {
        self.attachments.iter().any(|a| !a.extra.is_empty())
            || self
                .responses
                .iter()
                .any(|r| !r.extra.is_empty() || r.attachments.iter().any(|a| !a.extra.is_empty()))
    }
}

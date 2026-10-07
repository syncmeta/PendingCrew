//! RPC 参数。全部 `deny_unknown_fields`（§14 遗留：客户端拼错字段要报错，不能静默忽略）。
//! 嵌套在参数里的值（@、成员、附件）有自己的严格类型（`*Param`），经 `From` 转成模型类型：严格只管客户端输入，
//! 共享的模型类型对库里读出来的行保持宽松（控制者裁定 c，Task 14 复审）。
use crate::model::{
    Attachment, ChannelKind, Ledger, MemberRef, Mention, MentionKind, RestartAnswer,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! params {
    ($(#[$m:meta])* $name:ident { $($(#[$fm:meta])* $f:ident : $t:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
        $(#[$m])*
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $($(#[$fm])* pub $f: $t,)* }
    };
}

// R11（契约与解析器一致）：解析器收 `null` 的参数类型在契约里写成 `object | null`。只给只被
// `parse_no_params` / `parse_params_or_default` 解析的类型加（proto 的测试按方法逐个核）。
/// 客户端给的 @（`message.post`）。线上形状同 `Mention`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MentionParam {
    pub kind: MentionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
}

impl From<MentionParam> for Mention {
    fn from(p: MentionParam) -> Mention {
        Mention {
            kind: p.kind,
            target_id: p.target_id,
        }
    }
}

/// 客户端给的附件（`message.post`；非空一律拒，裁定 14）。线上形状同 `Attachment`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentParam {
    pub id: String,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

impl From<AttachmentParam> for Attachment {
    fn from(p: AttachmentParam) -> Attachment {
        Attachment {
            id: p.id,
            mime: p.mime,
            size: p.size,
            path: p.path,
            filename: p.filename,
        }
    }
}

/// 客户端给的成员 / 群主。线上形状同 `MemberRef`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum MemberParam {
    Session {
        id: String,
    },
    // `Default` 只为参数结构的 `Default`（`parse_params_or_default`）；没有业务含义。
    #[default]
    Human,
}

/// 手写：serde 对内部标签枚举的**单元**变体不执行 `deny_unknown_fields`（`{"kind":"human","id":"x"}` 会被静默收下），
/// 所以经一个线上形状相同、`Human` 写成空结构变体的影子类型来解。契约（JsonSchema）仍由上面的 derive 生成。
impl<'de> Deserialize<'de> for MemberParam {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
        enum Wire {
            Session { id: String },
            Human {},
        }
        Ok(match Wire::deserialize(d)? {
            Wire::Session { id } => MemberParam::Session { id },
            Wire::Human {} => MemberParam::Human,
        })
    }
}

impl From<MemberParam> for MemberRef {
    fn from(p: MemberParam) -> MemberRef {
        match p {
            MemberParam::Session { id } => MemberRef::Session { id },
            MemberParam::Human => MemberRef::Human,
        }
    }
}

params!(/// 无参数方法只收 null 或 {}。
    #[schemars(extend("type" = ["object", "null"]))]
    NoParams {});
params!(CrewIdParams { crew_id: String });
params!(/// 全可选：`null` 当 `{}`。归档的机组默认不列；`includeArchived: true` 连它们一起列。
    /// `Crew.childIds` 照实含归档的子机组（边还在）：列表里没有的 id 跳过即可。
    #[schemars(extend("type" = ["object", "null"]))]
    CrewListParams { #[serde(default)] include_archived: Option<bool> });
params!(CrewCreateParams { title: String, working_directory: String, #[serde(default)] parent_id: Option<String> });
params!(CrewUpdateParams {
    crew_id: String,
    #[serde(default)] title: Option<String>,
    #[serde(default)] working_directory: Option<String>,
    #[serde(default)] hidden: Option<bool>,
    /// "" = 清掉。
    #[serde(default)] status_line: Option<String>,
});
params!(CrewAdoptParams {
    crew_id: String,
    parent_id: String
});
params!(/// 侧栏次序按父边分层（控制者裁定 C1）：一次只排**一层**。
    /// 排 `parentId` 名下的直系子机组（写的是这条父边上的次序，读回来是父的 `Crew.childIds`；
    /// 多父机组在别的父下的次序不受影响）。顶层只剩总机组：一级机组的次序 = `parentId` 给总机组。
    /// `crewIds` 必须恰好是这一层现有的全部机组（不重复、不缺、不多），否则 -32602。
    CrewReorderParams { parent_id: String, crew_ids: Vec<String> });
params!(CrewReleaseParams { crew_id: String, from_parent_id: String, #[serde(default)] to_parent_id: Option<String> });
params!(ChannelCreateParams {
    crew_id: String,
    /// 只能是 topic 或 dm。
    kind: ChannelKind,
    #[serde(default)] name: String,
    #[serde(default)] purpose: Option<String>,
    #[serde(default)] members: Vec<MemberParam>,
});
params!(ChannelUpdateParams {
    channel_id: String,
    #[serde(default)] name: Option<String>,
    #[serde(default)] purpose: Option<String>,
    #[serde(default)] archived: Option<bool>,
});
params!(ChannelMembersParams { channel_id: String, members: Vec<MemberParam> });
params!(ChannelMemberParams {
    channel_id: String,
    member: MemberParam
});
params!(ChannelWatchParams {
    channel_id: String,
    on: bool
});
params!(ChannelSetOwnerParams {
    channel_id: String,
    owner: MemberParam
});
params!(MessageListParams {
    channel_id: String,
    #[serde(default)] after_seq: Option<i64>,
    #[serde(default)] before_seq: Option<i64>,
    /// 缺省 200，上限 1000。
    #[serde(default)] limit: Option<u32>,
});
params!(MessagePostParams {
    channel_id: String,
    text: String,
    #[serde(default)] in_reply_to: Option<String>,
    /// captain 不带 targetId（发送时解析）；session 必带。
    #[serde(default)] mentions: Vec<MentionParam>,
    /// 非空一律拒（上传协议未定，裁定 14）。
    #[serde(default)] attachments: Vec<AttachmentParam>,
});
params!(MessageSearchParams {
    query: String,
    #[serde(default)] crew_id: Option<String>,
    #[serde(default)] channel_ids: Option<Vec<String>>,
    #[serde(default)] after: Option<String>,
    #[serde(default)] before: Option<String>,
    /// 缺省 50，上限 500。
    #[serde(default)] limit: Option<u32>,
});
params!(ReadMarkParams {
    channel_id: String,
    seq: i64
});
params!(/// 全可选：`null` 当 `{}`。给了 `crewId` = 照实列这个机组的（归档的也列）；不给 = 全部，但归档机组的
/// session 只在 `includeArchived: true` 时列（同 `crew.list`）。
#[schemars(extend("type" = ["object", "null"]))]
SessionListParams {
    #[serde(default)] crew_id: Option<String>,
    #[serde(default)] include_archived: Option<bool>,
});
params!(SessionGetParams { session_id: String });
params!(/// 全可选：`null` 当 `{}`。
    #[schemars(extend("type" = ["object", "null"]))]
    AskListParams { #[serde(default)] home_crew_id: Option<String> });
params!(AskAnswerParams {
    crew_id: String,
    ask: String,
    text: String
});
params!(AskWithdrawParams {
    crew_id: String,
    ask: String,
    reason: String
});
params!(RestartAnswerParams {
    id: i64,
    answer: RestartAnswer
});
params!(/// 全可选：`null` 当 `{}`。
    #[schemars(extend("type" = ["object", "null"]))]
    DirectoryListParams { #[serde(default)] query: Option<String> });
params!(TodoListParams {
    crew_id: String,
    ledger: Ledger
});
params!(SubscribeParams {
    /// 主题 = 事件名第一个点之前的部分，例如 "message"。契约里是封闭枚举（`events::TOPICS`）；
    /// 服务端另有检查，认不出的主题 -32602。
    #[schemars(schema_with = "topics_schema")]
    topics: Vec<String>,
});

fn topics_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let topics = crate::events::TOPICS;
    schemars::json_schema!({
        "type": "array",
        "items": { "type": "string", "enum": topics },
    })
}

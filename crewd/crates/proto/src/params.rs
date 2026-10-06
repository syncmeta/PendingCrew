//! RPC 参数。全部 `deny_unknown_fields`（§14 遗留：客户端拼错字段要报错，不能静默忽略）。
use crate::model::{Attachment, ChannelKind, Ledger, MemberRef, Mention, RestartAnswer};
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
params!(/// 无参数方法只收 null 或 {}。
    #[schemars(extend("type" = ["object", "null"]))]
    NoParams {});
params!(CrewIdParams { crew_id: String });
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
    /// `parentId` 给了 = 排这个父名下的直系子机组（写的是这条父边上的次序，读回来是父的 `Crew.childIds`；
    /// 多父机组在别的父下的次序不受影响）；缺省 = 排顶层的根机组（写 `Crew.position`；内建机组不参与顶层排序、不能出现在名单里）。
    /// `crewIds` 必须恰好是这一层现有的全部机组（不重复、不缺、不多），否则 -32602。
    CrewReorderParams { #[serde(default)] parent_id: Option<String>, crew_ids: Vec<String> });
params!(CrewReleaseParams { crew_id: String, from_parent_id: String, #[serde(default)] to_parent_id: Option<String> });
params!(ChannelCreateParams {
    crew_id: String,
    /// 只能是 topic 或 dm。
    kind: ChannelKind,
    #[serde(default)] name: String,
    #[serde(default)] purpose: Option<String>,
    #[serde(default)] members: Vec<MemberRef>,
});
params!(ChannelUpdateParams {
    channel_id: String,
    #[serde(default)] name: Option<String>,
    #[serde(default)] purpose: Option<String>,
    #[serde(default)] archived: Option<bool>,
});
params!(ChannelMembersParams { channel_id: String, members: Vec<MemberRef> });
params!(ChannelMemberParams {
    channel_id: String,
    member: MemberRef
});
params!(ChannelWatchParams {
    channel_id: String,
    on: bool
});
params!(ChannelSetOwnerParams {
    channel_id: String,
    owner: MemberRef
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
    #[serde(default)] mentions: Vec<Mention>,
    /// 非空一律拒（上传协议未定，裁定 14）。
    #[serde(default)] attachments: Vec<Attachment>,
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
params!(/// 全可选：`null` 当 `{}`。
    #[schemars(extend("type" = ["object", "null"]))]
    SessionListParams { #[serde(default)] crew_id: Option<String> });
params!(SessionGetParams { session_id: String });
params!(/// 全可选：`null` 当 `{}`。
    #[schemars(extend("type" = ["object", "null"]))]
    AskListParams { #[serde(default)] root_crew_id: Option<String> });
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
params!(ApprovalDecideParams {
    id: String,
    allow: bool
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
    /// 主题 = 事件名第一个点之前的部分，例如 "message"。
    topics: Vec<String>,
});

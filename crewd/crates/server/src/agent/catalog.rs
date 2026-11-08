//! 工具表（spec §22.3 表 + §22.7 Q2 / Q4）。只有名字、范围、参数形状；文字全在 tools.*.md。
use serde_json::{Map, Value, json};

use crate::agent::text::{self, Locale};
use crate::agent::{Identity, Role};
use crewd_proto::agent::{ToolDef, ToolHints};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    CrewMember,
    Captain,
    CaseAdmin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Num,
    Bool,
    StrList,
    /// 消息引用：序号（数字或 `#123`）或消息 id。
    Ref,
    /// `[{kind, target_id?}]`
    Mentions,
    Evidence,
    WorkBlockers,
    FolderObject,
    ChannelWorkObject,
    DocumentEdits,
    WorkContinuation,
    CoordinationChildren,
    ArtifactInput,
    ArtifactReference,
    ArtifactSubject,
    ArtifactChange,
    ArtifactHistoryCursor,
    ManagedObjectScope,
    ManagedTableV2Operation,
    ManagedTableHistorySource,
    ManagedTableHistoryKey,
    ManagedTableHistoryPath,
    ManagedLinkV2Operation,
    ManagedLinkQuery,
    ManagedLinkTarget,
    ManagedObjectChange,
    ManagedMessageChange,
    DecisionAskDraft,
    SecretaryReferences,
    /// `[{text, category?}]`
    Messages,
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub required: bool,
    pub choices: &'static [&'static str],
    /// 整数参数的闭区间（md 里写的范围；schema 的 minimum / maximum）。
    pub bounds: Option<(i64, i64)>,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub scope: Scope,
    pub params: &'static [Param],
    /// MCP `readOnlyHint`: changes nothing (S76; the index table's RO column).
    pub read_only: bool,
    /// MCP `destructiveHint`: may undo or end something (S76).
    pub destructive: bool,
    /// MCP `idempotentHint`: calling again with the same arguments changes nothing more.
    pub idempotent: bool,
    /// Only talks (`post`, `channel_listen`): not output for the loop breaker (B14).
    pub talk: bool,
}

impl ToolSpec {
    /// Not output for the loop breaker (B14, S76): reads and talk. One rule, derived here.
    pub fn passive(&self) -> bool {
        self.read_only || self.talk
    }

    pub fn hints(&self) -> ToolHints {
        ToolHints {
            read_only_hint: self.read_only,
            destructive_hint: self.destructive,
            idempotent_hint: self.idempotent,
            open_world_hint: false,
        }
    }
}

/// The categories the injection digest folds (S62); see `CATEGORIES`.
pub const CAT_PROGRESS: &str = "progress";
pub const CAT_ACK: &str = "ack";
pub const CATEGORIES: &[&str] = &[
    CAT_PROGRESS,
    "done",
    "blocked",
    "question",
    "finding",
    "handoff",
    CAT_ACK,
    "note",
];
pub const MENTION_KINDS: &[&str] = &["session", "captain", "human"];
const RUNNERS: &[&str] = &["claude", "codex"];
/// `nudge_session.key` (T15 Q1): both interrupt the current turn; free text goes through `post(to:)`.
pub const NUDGE_KEYS: &[&str] = &["esc", "interrupt"];
/// `crew_structure.action` (S69). No remove / restore / purge / reorder: those are the human's.
pub const STRUCTURE_ACTIONS: &[&str] = &[
    "create_child",
    "create_parent",
    "adopt",
    "adopt_parent",
    "release",
];

const fn p(name: &'static str, ty: Ty, required: bool) -> Param {
    Param {
        name,
        ty,
        required,
        choices: &[],
        bounds: None,
    }
}
const fn c(name: &'static str, required: bool, choices: &'static [&'static str]) -> Param {
    Param {
        name,
        ty: Ty::Str,
        required,
        choices,
        bounds: None,
    }
}
const fn int(name: &'static str, min: i64, max: i64) -> Param {
    Param {
        name,
        ty: Ty::Int,
        required: false,
        choices: &[],
        bounds: Some((min, max)),
    }
}
const fn all(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec {
        name,
        scope: Scope::All,
        params,
        read_only: false,
        destructive: false,
        idempotent: false,
        talk: false,
    }
}
const fn crew_member(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec {
        scope: Scope::CrewMember,
        ..all(name, params)
    }
}
const fn cap(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec {
        scope: Scope::Captain,
        ..all(name, params)
    }
}
/// Read-only (and so idempotent).
const fn ro(t: ToolSpec) -> ToolSpec {
    ToolSpec {
        read_only: true,
        idempotent: true,
        ..t
    }
}
const fn destructive(t: ToolSpec) -> ToolSpec {
    ToolSpec {
        destructive: true,
        ..t
    }
}
const fn idempotent(t: ToolSpec) -> ToolSpec {
    ToolSpec {
        idempotent: true,
        ..t
    }
}
const fn talk(t: ToolSpec) -> ToolSpec {
    ToolSpec { talk: true, ..t }
}

use Ty::*;

pub const CATALOG: &[ToolSpec] = &[
    ro(ToolSpec {
        scope: Scope::CaseAdmin,
        ..all(crewd_proto::admin_secretary::SNAPSHOT_TOOL, &[])
    }),
    ro(ToolSpec {
        scope: Scope::CaseAdmin,
        ..all(
            crewd_proto::admin_secretary::RECEIPT_TOOL,
            &[p("request_id", Str, true)],
        )
    }),
    idempotent(ToolSpec {
        scope: Scope::CaseAdmin,
        ..all(
            crewd_proto::admin_secretary::PUBLISH_TOOL,
            &[
                p("request_id", Str, true),
                p("source_revision", Str, true),
                p("generation", Str, true),
                p("body", Str, true),
                p("references", SecretaryReferences, true),
            ],
        )
    }),
    ro(all(
        "admin_case_custody_get",
        &[p("crew_id", Str, true), p("case_id", Str, true)],
    )),
    ro(all(
        "admin_case_custody_list",
        &[p("after", Str, false), int("limit", 1, 100)],
    )),
    idempotent(cap(
        "admin_case_custody_accept",
        &[
            p("request_id", Str, true),
            p("expected_revision", Str, true),
            p("note", Str, true),
        ],
    )),
    idempotent(all(
        "admin_case_handoff_request",
        &[p("request_id", Str, true), p("reason", Str, true)],
    )),
    ro(all("admin_case_handoff_get", &[p("request_id", Str, true)])),
    ro(all(
        "admin_case_handoff_list",
        &[p("after", Str, false), int("limit", 1, 100)],
    )),
    idempotent(cap(
        "admin_case_handoff_accept",
        &[
            p("request_id", Str, true),
            p("expected_revision", Str, true),
            p("note", Str, true),
        ],
    )),
    ro(all(
        "my_work_items",
        &[
            p("after", Str, false),
            int("limit", 1, 200),
            p("include_finished", Bool, false),
            p("include_removed", Bool, false),
        ],
    )),
    ro(all(
        "channel_work_list",
        &[
            p("channel_id", Str, true),
            p("after", Str, false),
            int("limit", 1, 100),
        ],
    )),
    idempotent(all(
        "channel_work_link",
        &[
            p("channel_id", Str, true),
            p("expected_link_revision", Str, true),
            p("object", ChannelWorkObject, true),
        ],
    )),
    destructive(idempotent(all(
        "channel_work_unlink",
        &[
            p("channel_id", Str, true),
            p("expected_link_revision", Str, true),
            p("object", ChannelWorkObject, true),
        ],
    ))),
    idempotent(all(
        "document_create",
        &[
            p("request_id", Str, true),
            p("crew_id", Str, true),
            p("title", Str, true),
            p("body", Str, true),
        ],
    )),
    ro(all(
        "document_get",
        &[
            p("id", Str, true),
            int("offset", 0, 1048576),
            int("limit", 1, 16384),
            p("expected_revision", Str, false),
        ],
    )),
    ro(all(
        "document_list",
        &[
            p("crew_id", Str, true),
            p("after", Str, false),
            int("limit", 1, 100),
            p("include_deleted", Bool, false),
        ],
    )),
    idempotent(all(
        "document_update",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("title", Str, true),
            p("body", Str, true),
        ],
    )),
    destructive(idempotent(all(
        "document_delete",
        &[p("id", Str, true), p("expected_revision", Str, true)],
    ))),
    idempotent(all(
        "document_patch",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("edits", DocumentEdits, true),
        ],
    )),
    ro(all(
        "view_folder_list",
        &[
            p("crew_id", Str, true),
            c("view", true, &["messages", "todo", "documents", "progress"]),
            c("part", false, &["folders", "assignments"]),
            p("after", Str, false),
            int("limit", 1, 100),
        ],
    )),
    idempotent(all(
        "view_folder_create",
        &[
            p("crew_id", Str, true),
            c("view", true, &["messages", "todo", "documents", "progress"]),
            p("request_id", Str, true),
            p("expected_tree_revision", Str, true),
            p("name", Str, true),
            p("parent_id", Str, false),
        ],
    )),
    idempotent(all(
        "view_folder_update",
        &[
            p("crew_id", Str, true),
            c("view", true, &["messages", "todo", "documents", "progress"]),
            p("id", Str, true),
            p("expected_tree_revision", Str, true),
            p("name", Str, true),
            p("parent_id", Str, false),
            p("position", Int, true),
        ],
    )),
    destructive(idempotent(all(
        "view_folder_delete",
        &[
            p("crew_id", Str, true),
            c("view", true, &["messages", "todo", "documents", "progress"]),
            p("id", Str, true),
            p("expected_tree_revision", Str, true),
        ],
    ))),
    idempotent(all(
        "view_folder_assign",
        &[
            p("crew_id", Str, true),
            c("view", true, &["messages", "todo", "documents", "progress"]),
            p("expected_tree_revision", Str, true),
            p("object", FolderObject, true),
            p("folder_id", Str, false),
        ],
    )),
    idempotent(all(
        "work_item_transfer_request",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("to", Str, true),
            p("note", Str, true),
        ],
    )),
    idempotent(all(
        "work_item_transfer_accept",
        &[p("id", Str, true), p("expected_revision", Str, true)],
    )),
    idempotent(all(
        "work_item_transfer_withdraw",
        &[p("id", Str, true), p("expected_revision", Str, true)],
    )),
    ro(all("todo_get", &[p("id", Str, true)])),
    ro(cap(
        crewd_collab::requirement_ruling::CAPTAIN_CHECK_TOOL,
        &[
            p("id", Str, true),
            p("after", Str, false),
            p("expected_revision", Str, false),
        ],
    )),
    idempotent(cap(
        crewd_collab::requirement_ruling::CAPTAIN_CLOSE_TOOL,
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("request_id", Str, true),
            p("note", Str, true),
        ],
    )),
    ro(all(
        "work_closure_history",
        &[
            p("id", Str, true),
            p("before_seq", Str, false),
            int("limit", 1, 200),
        ],
    )),
    ro(all("work_item_get", &[p("id", Str, true)])),
    ro(all(
        "work_item_history",
        &[
            p("id", Str, true),
            p("before", Str, false),
            int("limit", 1, 200),
        ],
    )),
    all(
        "work_item_create",
        &[
            p("request_id", Str, false),
            p("channel_id", Str, false),
            p("title", Str, true),
            p("goal", Str, false),
            c("level", false, &["local", "public", "production"]),
            p("acceptance", Str, false),
            p("owner", Str, false),
            p("crew", Str, false),
            p("parent", Str, false),
            p("depends_on", StrList, false),
            p("sources", StrList, false),
            p("next_check_at", Str, false),
        ],
    ),
    idempotent(all(
        "work_item_close",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            c("kind", true, &["done", "duplicate", "superseded"]),
            p("evidence", Evidence, true),
            p("note", Str, true),
        ],
    )),
    idempotent(ToolSpec {
        scope: Scope::CaseAdmin,
        ..all(
            crewd_collab::admin_case_link::UNLINK_TOOL,
            &[
                p("request_id", Str, true),
                p("id", Str, true),
                p("expected_revision", Str, true),
                p("related_id", Str, true),
                p("expected_related_revision", Str, true),
                c("edge", true, &["parent", "dependency"]),
                p("evidence", Evidence, true),
                p("note", Str, true),
            ],
        )
    }),
    idempotent(all(
        "work_item_update",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            c("status", false, &["open", "active", "done", "dropped"]),
            p("next_check_at", Str, false),
            p("note", Str, false),
            p("evidence", Evidence, false),
        ],
    )),
    idempotent(all(
        "work_item_block",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("on", WorkBlockers, true),
            p("can_continue", WorkContinuation, true),
        ],
    )),
    idempotent(all(
        "work_item_assign",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("owner", Str, true),
            p("note", Str, true),
            p("coordination_decision_id", Str, false),
        ],
    )),
    idempotent(cap(
        crewd_collab::work_coordination::SPLIT_TOOL,
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("coordination_decision_id", Str, true),
            p("children", CoordinationChildren, true),
        ],
    )),
    idempotent(all(
        "work_item_reassign_lost",
        &[
            p("id", Str, true),
            p("expected_revision", Str, true),
            p("owner", Str, true),
            p("note", Str, true),
        ],
    )),
    talk(all(
        "post",
        &[
            p("channel", Str, false),
            p("to", Str, false),
            p("message", Str, false),
            p("messages", Messages, false),
            p("mentions", Mentions, false),
            p("reply_to", Ref, false),
            c("category", false, CATEGORIES),
        ],
    )),
    ro(all(
        "read_channel",
        &[
            p("channel", Str, false),
            int("limit", 1, 200),
            p("before", Ref, false),
        ],
    )),
    ro(all(
        "search",
        &[
            p("query", Str, true),
            p("channel", Str, false),
            p("crew", Str, false),
            p("after", Str, false),
            p("before", Str, false),
            c("in", false, &["messages", "objects", "all"]),
            p("type", Str, false),
            p("cursor", Str, false),
            int("limit", 1, 200),
        ],
    )),
    ro(crew_member(
        "organization_search_expand",
        &[
            p("target", ManagedLinkTarget, true),
            p("cursor", Str, false),
            int("limit", 1, 200),
            p("include_system_defaults", Bool, false),
        ],
    )),
    ro(all(
        "list_channels",
        &[p("crew", Str, false), p("include_archived", Bool, false)],
    )),
    all(
        "channel_create",
        &[
            p("name", Str, true),
            p("purpose", Str, false),
            p("members", StrList, false),
        ],
    ),
    all(
        "channel_invite",
        &[p("channel", Str, true), p("members", StrList, true)],
    ),
    all("channel_leave", &[p("channel", Str, true)]),
    destructive(all("channel_archive", &[p("channel", Str, true)])),
    ro(all("channel_archive_check", &[p("channel", Str, true)])),
    destructive(all(
        "channel_archive_verified",
        &[
            p("channel", Str, true),
            p("expected_revision", Str, true),
            p("evidence", Str, true),
            p("note", Str, true),
        ],
    )),
    all(
        "channel_restore_verified",
        &[
            p("channel", Str, true),
            p("expected_revision", Str, true),
            p("note", Str, true),
        ],
    ),
    all(
        "channel_owner",
        &[
            p("channel", Str, true),
            p("to", Str, false),
            p("force", Bool, false),
        ],
    ),
    talk(idempotent(all(
        "channel_listen",
        &[
            p("channel", Str, true),
            int("minutes", 1, 480),
            p("senders", StrList, false),
            p("off", Bool, false),
        ],
    ))),
    ro(all("directory", &[p("query", Str, false)])),
    all(
        "ask",
        &[
            p("question", Str, true),
            p("resume_note", Str, false),
            p("fallback_after_minutes", Num, false),
            p("fallback", Str, false),
        ],
    ),
    idempotent(crew_member(
        "ask_decision",
        &[
            p("scope", ManagedObjectScope, true),
            p("requestId", Str, true),
            p("draft", DecisionAskDraft, true),
        ],
    )),
    ro(crew_member(
        "ask_decision_request_get",
        &[
            p("scope", ManagedObjectScope, true),
            p("requestId", Str, true),
        ],
    )),
    all(
        "withdraw_ask",
        &[
            p("ask", Str, true),
            p("reason", Str, true),
            p("crew", Str, false),
        ],
    ),
    all(
        "respond_todo",
        &[p("number", Int, true), p("response", Str, true)],
    ),
    ro(all("get_quota", &[])),
    all(
        "schedule_wakeup",
        &[
            int("after_minutes", 1, 1440),
            p("at", Str, false),
            p("note", Str, true),
        ],
    ),
    idempotent(all(
        "set_session_profile",
        &[p("model", Str, false), p("effort", Str, false)],
    )),
    idempotent(cap(
        "rename_crew",
        &[p("name", Str, true), p("crew", Str, false)],
    )),
    destructive(idempotent(cap(
        "change_workdir",
        &[
            p("new_path", Str, true),
            p("crew", Str, false),
            p("include_children", Bool, false),
            p("confirm", Bool, false),
        ],
    ))),
    idempotent(cap(
        "set_crew_status",
        &[p("text", Str, true), p("crew", Str, false)],
    )),
    idempotent(cap(
        "delegation_policy",
        &[p("text", Str, false), p("crew", Str, false)],
    )),
    cap(
        "start_session",
        &[
            p("brief", Str, true),
            p("title", Str, false),
            c("runner", false, RUNNERS),
            p("isolation", Bool, true),
            p("model", Str, false),
            p("effort", Str, false),
            p("crew", Str, false),
        ],
    ),
    destructive(cap(
        "stop_session",
        &[p("session_id", Str, true), p("reason", Str, true)],
    )),
    ro(cap("inspect_session", &[p("session_id", Str, true)])),
    cap(
        "nudge_session",
        &[p("session_id", Str, true), c("key", true, NUDGE_KEYS)],
    ),
    ro(cap("list_sessions", &[p("crew", Str, false)])),
    destructive(cap(
        "handoff_captain",
        &[
            p("crew", Str, false),
            p("to_session", Str, false),
            c("runner", false, RUNNERS),
            p("model", Str, false),
            p("effort", Str, false),
            p("title", Str, false),
            p("opening_brief", Str, false),
        ],
    )),
    destructive(cap(
        "crew_structure",
        &[
            c("action", true, STRUCTURE_ACTIONS),
            p("crew", Str, false),
            p("target", Str, false),
            p("to", Str, false),
            p("title", Str, false),
            p("brief", Str, false),
            c("runner", false, RUNNERS),
        ],
    )),
    ro(crew_member(
        "managed_table_v2_get",
        &[
            p("id", Str, true),
            int("contractVersion", 2, 3),
            c(
                "action",
                false,
                &["table", "defineCheck", "history", "historyValue"],
            ),
            p("scope", ManagedObjectScope, false),
            p("crewId", Str, false),
            p("excludingTableId", Str, false),
            p("title", Str, false),
            p("purpose", Str, false),
            p("cursor", Str, false),
            p("source", ManagedTableHistorySource, false),
            p("key", ManagedTableHistoryKey, false),
            p("path", ManagedTableHistoryPath, false),
        ],
    )),
    ro(crew_member(
        "managed_tables_v2_list",
        &[
            p("after", Str, false),
            int("contractVersion", 2, 3),
            p("includeArchived", Bool, false),
        ],
    )),
    ro(crew_member(
        "managed_row_v2_get",
        &[
            p("id", Str, true),
            int("contractVersion", 2, 3),
            c("action", false, &["row", "links"]),
            p("rowId", Str, false),
            p("fieldId", Str, false),
            p("expectedSchemaRevision", Str, false),
            p("expectedRowRevision", Str, false),
        ],
    )),
    ro(crew_member(
        "managed_rows_v2_list",
        &[
            p("tableId", Str, true),
            p("after", Str, false),
            int("contractVersion", 2, 3),
            p("includeArchived", Bool, false),
        ],
    )),
    ToolSpec {
        destructive: true,
        idempotent: true,
        ..crew_member(
            "managed_table_v2_submit",
            &[
                int("contractVersion", 2, 3),
                p("scope", ManagedObjectScope, true),
                p("crewId", Str, true),
                p("requestId", Str, true),
                p("operation", ManagedTableV2Operation, true),
            ],
        )
    },
    ro(crew_member(
        "managed_table_v2_request_get",
        &[
            int("contractVersion", 2, 3),
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
    ro(crew_member(
        "managed_link_resolve_target",
        &[p("target", ManagedLinkTarget, true)],
    )),
    ro(crew_member(
        "managed_links_list",
        &[
            p("query", ManagedLinkQuery, true),
            p("after", Str, false),
            Param {
                required: true,
                ..int("limit", 1, 64)
            },
        ],
    )),
    ToolSpec {
        destructive: true,
        idempotent: true,
        ..crew_member(
            "managed_link_submit",
            &[
                p("scope", ManagedObjectScope, true),
                p("crewId", Str, true),
                p("requestId", Str, true),
                p("operation", ManagedLinkV2Operation, true),
            ],
        )
    },
    ro(crew_member(
        "managed_link_request_get",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
    ro(crew_member(
        "artifact_history_status",
        &[p("crewId", Str, true), p("jobId", Str, false)],
    )),
    idempotent(crew_member(
        "artifact_history_begin",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
    idempotent(crew_member(
        "artifact_history_step",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
            p("jobId", Str, true),
            p("expectedRevision", Str, true),
            p("expectedCursor", ArtifactHistoryCursor, true),
            Param {
                required: true,
                ..int("limit", 1, 64)
            },
        ],
    )),
    ro(crew_member(
        "artifact_history_request_get",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
    ro(crew_member(
        "artifact_preview",
        &[p("crewId", Str, true), p("input", ArtifactInput, true)],
    )),
    ro(crew_member(
        "artifacts_list",
        &[p("after", ArtifactReference, false)],
    )),
    ro(crew_member(
        "artifact_get",
        &[p("reference", ArtifactReference, true)],
    )),
    ro(crew_member(
        "artifact_backlinks",
        &[
            p("reference", ArtifactReference, true),
            p("after", Str, false),
            p("includeLegacySources", Bool, false),
        ],
    )),
    ro(crew_member(
        "artifact_subject",
        &[
            p("subject", ArtifactSubject, true),
            p("after", Str, false),
            p("includeLegacySources", Bool, false),
        ],
    )),
    ToolSpec {
        destructive: true,
        idempotent: true,
        ..crew_member(
            "artifact_submit",
            &[
                p("scope", ManagedObjectScope, true),
                p("crewId", Str, true),
                p("requestId", Str, true),
                p("change", ArtifactChange, true),
            ],
        )
    },
    ro(crew_member(
        "artifact_request_get",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
    ro(crew_member("managed_object_scope_get", &[])),
    ro(crew_member(
        "managed_object_get",
        &[p("id", Str, true), p("includeSystemDefaults", Bool, false)],
    )),
    ro(crew_member(
        "managed_tables_list",
        &[p("after", Str, false)],
    )),
    ro(crew_member(
        "managed_rows_list",
        &[p("tableId", Str, true), p("after", Str, false)],
    )),
    ro(crew_member(
        "managed_decisions_list",
        &[
            p("after", Str, false),
            p("includeSystemDefaults", Bool, false),
        ],
    )),
    ro(crew_member("managed_risks_list", &[p("after", Str, false)])),
    ro(crew_member(
        "managed_lessons_list",
        &[p("after", Str, false)],
    )),
    ToolSpec {
        destructive: true,
        idempotent: true,
        ..crew_member(
            "managed_object_from_message",
            &[
                p("scope", ManagedObjectScope, true),
                p("crewId", Str, true),
                p("requestId", Str, true),
                p("sourceMessageId", Str, true),
                p("change", ManagedMessageChange, true),
            ],
        )
    },
    ro(crew_member(
        "managed_message_request_get",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
    ro(crew_member(
        "managed_object_source_get",
        &[p("id", Str, true)],
    )),
    ro(crew_member(
        "managed_objects_from_message",
        &[p("messageId", Str, true), p("after", Str, false)],
    )),
    ToolSpec {
        destructive: true,
        idempotent: true,
        ..crew_member(
            "managed_object_submit",
            &[
                p("scope", ManagedObjectScope, true),
                p("crewId", Str, true),
                p("requestId", Str, true),
                p("change", ManagedObjectChange, true),
            ],
        )
    },
    ro(crew_member(
        "managed_object_request_get",
        &[
            p("scope", ManagedObjectScope, true),
            p("crewId", Str, true),
            p("requestId", Str, true),
        ],
    )),
];

/// Removed or renamed tool names (§9, §22.3, §22.7 Q4, S65, S70) and what replaces each, written
/// the way an agent calls it (a tool name, optionally with the parameter that does the job). An
/// empty replacement = gone without a successor (M4 or the human took it over). Calling an old
/// name with a replacement answers `r.tool_removed`, naming it.
pub const REMOVED: &[(&str, &str)] = &[
    // §9: work tracking moved to M4.
    ("continue_work", ""),
    ("confirm_todo_sweep", ""),
    ("plan_add", ""),
    ("plan_update", ""),
    ("plan_list", ""),
    // Q4: the human's @ marker replaces attention flags; the human orders the sidebar.
    ("raise_attention", "post(mentions: [{kind: \"human\"}])"),
    ("clear_attention", ""),
    ("crew_ordering_signals", ""),
    ("arrange_crews", ""),
    ("post_to_crew", "post"),
    ("read_whiteboard", "read_channel"),
    ("search_whiteboard", "search"),
    ("add_human_todo", "ask"),
    ("withdraw_human_todo", "withdraw_ask"),
    ("listen", "channel_listen"),
    // S51: pending decisions are the human's to answer.
    ("answer_decision", ""),
    // S70 merges and S65 removals (M3b).
    ("human_watching", "list_channels"),
    ("contact", "post(to:)"),
    ("channel_transfer", "channel_owner(to:)"),
    ("channel_accept", "channel_owner"),
    ("channel_unlisten", "channel_listen(off: true)"),
    ("handoff_captain_to_session", "handoff_captain(to_session:)"),
    ("create_and_handoff_captain", "handoff_captain(runner:)"),
    ("report_to_parent", "post(to:)"),
    ("message_child_crew", "post(to:)"),
    ("read_delegation_policy", "delegation_policy"),
    ("update_delegation_policy", "delegation_policy(text:)"),
    ("adopt_crew", "crew_structure(action: \"adopt\")"),
    ("release_crew", "crew_structure(action: \"release\")"),
    (
        "create_parent_crew",
        "crew_structure(action: \"create_parent\")",
    ),
    ("adopt_parent", "crew_structure(action: \"adopt_parent\")"),
    (
        "create_child_crew",
        "crew_structure(action: \"create_child\")",
    ),
];

/// What replaces a removed tool (`None`: not a removed name, or gone without a successor).
pub fn replacement(name: &str) -> Option<&'static str> {
    REMOVED
        .iter()
        .find(|(old, _)| *old == name)
        .map(|(_, new)| *new)
        .filter(|new| !new.is_empty())
}

pub(crate) fn requires_managed_scope(name: &str) -> bool {
    find(name).is_some_and(|spec| spec.scope == Scope::CrewMember)
}

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    CATALOG.iter().find(|t| t.name == name)
}

/// 这个身份此刻能不能看见 / 调这个工具：只看角色。总机组没有特例（S68）：它的机长与任何机组的机长同一张表，
/// 它的其他 session 与任何 worker 同一张表。
pub fn visible(t: &ToolSpec, me: &Identity) -> bool {
    match t.scope {
        Scope::All => true,
        Scope::CrewMember => matches!(me.role, Role::Worker | Role::Captain),
        Scope::Captain => me.role == Role::Captain,
        Scope::CaseAdmin => false,
    }
}

pub(crate) fn visible_in_case(t: &ToolSpec, me: &Identity) -> bool {
    t.scope == Scope::CaseAdmin && me.role == Role::Admin
}

fn definition(t: &ToolSpec, loc: Locale) -> ToolDef {
    ToolDef {
        name: t.name.to_string(),
        description: text::get(loc, t.name).to_string(),
        input_schema: input_schema(t, loc),
        annotations: Some(t.hints()),
    }
}

#[cfg(test)]
pub(crate) fn case_tools_for(me: &Identity, loc: Locale) -> Vec<ToolDef> {
    case_tools_for_purpose(me, loc, false)
}
pub(crate) fn case_tools_for_purpose(me: &Identity, loc: Locale, secretary: bool) -> Vec<ToolDef> {
    CATALOG
        .iter()
        .filter(|t| {
            (visible(t, me) || visible_in_case(t, me))
                && if secretary {
                    super::admin_case_scope::secretary_tool(t.name)
                } else {
                    super::admin_case_scope::allowed_tool(t.name)
                }
        })
        .map(|t| definition(t, loc))
        .collect()
}

pub fn tools_for(me: &Identity, loc: Locale) -> Vec<ToolDef> {
    CATALOG
        .iter()
        .filter(|t| visible(t, me))
        .map(|t| definition(t, loc))
        .collect()
}

fn work_blockers_schema() -> Value {
    json!({"type":"array","minItems":1,"maxItems":16,"items":{"oneOf":[
        {"type":"object","properties":{"type":{"type":"string","enum":["work"]},"id":{"type":"string"}},"required":["type","id"],"additionalProperties":false},
        {"type":"object","properties":{"type":{"type":"string","enum":["human"]},"id":{"type":"string"}},"required":["type","id"],"additionalProperties":false},
        {"type":"object","properties":{"type":{"type":"string","enum":["new_human"]},"question":{"type":"string"},"default":{"type":"object","properties":{"text":{"type":"string"},"after_minutes":{"type":"integer","minimum":1}},"required":["text","after_minutes"],"additionalProperties":false}},"required":["type","question"],"additionalProperties":false}
    ]}})
}
fn work_continuation_schema(loc: Locale) -> Value {
    let mut item = input_schema(find("work_item_create").expect("existing create"), loc);
    if let Some(props) = item["properties"].as_object_mut() {
        props.remove("request_id");
        props.remove("channel_id");
    }
    item["properties"]
        .as_object_mut()
        .expect("create object")
        .remove("parent");
    json!({"oneOf":[
        {"type":"object","properties":{"type":{"type":"string","enum":["nothing"]}},"required":["type"],"additionalProperties":false},
        {"type":"object","properties":{"type":{"type":"string","enum":["split"]},"items":{"type":"array","minItems":1,"maxItems":20,"items":item}},"required":["type","items"],"additionalProperties":false}
    ]})
}

fn artifact_input_schema() -> Value {
    json!({"type":"object","properties":{
        "kind":{"type":"string","enum":["commit","pullRequest","file","test","link"]},
        "value":{"type":"string","minLength":1,"maxLength":4096},
        "repository":{"type":["string","null"],"minLength":1,"maxLength":512}
    },"required":["kind","value"],"additionalProperties":false})
}
fn artifact_reference_schema() -> Value {
    json!({"type":"object","properties":{
        "namespace":{"oneOf":[
            {"type":"object","properties":{"kind":{"const":"global"}},"required":["kind"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"repository"},"id":{"type":"string","minLength":1,"maxLength":512}},"required":["kind","id"],"additionalProperties":false},
            {"type":"object","properties":{"kind":{"const":"unassigned"},"crewId":{"type":"string"}},"required":["kind","crewId"],"additionalProperties":false}
        ]},"key":{"type":"string","minLength":1,"maxLength":4096}
    },"required":["namespace","key"],"additionalProperties":false})
}
fn artifact_subject_schema() -> Value {
    json!({"type":"object","properties":{"kind":{"type":"string","enum":["work","requirement","decision","lesson","risk","row","table"]},"id":{"type":"string"}},"required":["kind","id"],"additionalProperties":false})
}
fn artifact_change_schema() -> Value {
    let relation = json!({"type":"string","enum":["evidence","appliesTo","relates"]});
    json!({"oneOf":[
        {"type":"object","properties":{"kind":{"const":"register"},"input":artifact_input_schema()},"required":["kind","input"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"link"},"input":artifact_input_schema(),"subject":artifact_subject_schema(),"relation":relation},"required":["kind","input","subject","relation"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"linkExisting"},"reference":artifact_reference_schema(),"subject":artifact_subject_schema(),"relation":relation},"required":["kind","reference","subject","relation"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"unlink"},"reference":artifact_reference_schema(),"subject":artifact_subject_schema(),"relation":relation,"expectedEdgeId":{"type":"string","pattern":"^[1-9][0-9]*$"}},"required":["kind","reference","subject","relation","expectedEdgeId"],"additionalProperties":false}
    ]})
}

fn managed_change_schema() -> Value {
    let text = json!({"type":"string"});
    let object = |properties: Value, required: &[&str]| json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    let field = object(
        json!({
            "name":{"type":"string","minLength":1,"maxLength":256},
            "fieldType":{"type":"string","enum":["text","integer","real"]},
            "nullable":{"type":"boolean"},
        }),
        &["name", "fieldType", "nullable"],
    );
    let values = json!({"type":"array","minItems":1,"maxItems":32,"items":{"oneOf":[
        object(json!({"type":{"const":"null"}}), &["type"]),
        object(json!({"type":{"const":"text"},"value":{"type":"string","maxLength":65536}}), &["type","value"]),
        object(json!({"type":{"const":"integer"},"value":{"type":"string","pattern":"^(0|-?[1-9][0-9]*)$"}}), &["type","value"]),
        object(json!({"type":{"const":"real"},"value":{"type":"number"}}), &["type","value"]),
    ]}});
    let decision_id = json!({"type":"string","pattern":"^D[1-9][0-9]*$"});
    let revision = json!({"type":"string","pattern":"^[1-9][0-9]*$"});
    let risk_id = json!({"type":"string","pattern":"^R[1-9][0-9]*$"});
    let lesson_id = json!({"type":"string","pattern":"^L[1-9][0-9]*$"});
    let bounded_text = json!({"type":"string","minLength":1,"maxLength":1500});
    let optional_text = json!({"type":["string","null"],"minLength":1,"maxLength":1500});
    let references = |kinds: &[&str], minimum: u8, maximum: u8| {
        let reference = object(
            json!({"kind":{"type":"string","enum":kinds},
                   "id":{"type":"string","minLength":1,"maxLength":128}}),
            &["kind", "id"],
        );
        json!({"type":"array","minItems":minimum,"maxItems":maximum,"uniqueItems":true,"items":reference})
    };
    let decision = object(
        json!({
            "question":{"type":"string","minLength":1,"maxLength":60},
            "choice":{"type":"string","minLength":1,"maxLength":1500},
            "rationale":{"type":"string","minLength":1,"maxLength":1500},
            "alternatives":{"type":["string","null"],"minLength":1,"maxLength":1500},
            "reversibility":{"type":["string","null"],"enum":["reversible","hardToReverse","irreversible",null]},
            "about":references(&["crew","work","requirement","row","risk"], 1, 8),
        }),
        &["question", "choice", "rationale", "about"],
    );
    let owner = json!({"oneOf":[
        object(json!({"kind":{"const":"human"}}), &["kind"]),
        object(json!({"kind":{"const":"session"},"sessionId":{"type":"string","minLength":1,"maxLength":128}}),
               &["kind","sessionId"]),
    ]});
    let risk = object(
        json!({
            "summary":{"type":"string","minLength":1,"maxLength":120},
            "impact":{"type":"string","enum":["low","medium","high"]},
            "likelihood":{"type":["string","null"],"enum":["low","medium","high",null]},
            "mitigation":optional_text,
            "about":references(&["crew","work","requirement"], 1, 8),
            "mitigatedBy":references(&["work","decision"], 0, 8),
        }),
        &["summary", "impact", "about", "mitigatedBy"],
    );
    let lesson = |source_limit| {
        object(
            json!({
                "happened":bounded_text,
                "lesson":{"type":"string","minLength":1,"maxLength":120},
                "detection":optional_text,
                "learnedFrom":references(&["work","ask"], 0, source_limit),
                "evidence":{"type":"array","maxItems":8,"uniqueItems":true,"items":bounded_text},
                "appliesTo":references(&["crew","work","table"], 0, 8),
            }),
            &["happened", "lesson", "learnedFrom", "evidence", "appliesTo"],
        )
    };
    json!({"oneOf":[
        object(json!({"kind":{"const":"createTable"},"title":{"type":"string","minLength":1,"maxLength":256},
                      "fields":{"type":"array","minItems":1,"maxItems":32,"items":field}}),
               &["kind","title","fields"]),
        object(json!({"kind":{"const":"addRow"},"tableId":text,"values":values}),
               &["kind","tableId","values"]),
        object(json!({"kind":{"const":"editRow"},"rowId":text,"expectedRevision":text,"values":values}),
               &["kind","rowId","expectedRevision","values"]),
        object(json!({"kind":{"const":"recordDecision"},"draft":decision}), &["kind","draft"]),
        object(json!({"kind":{"const":"replaceDecision"},"decisionId":decision_id,"expectedRevision":revision,"draft":decision}),
               &["kind","decisionId","expectedRevision","draft"]),
        object(json!({"kind":{"const":"withdrawDecision"},"decisionId":decision_id,"expectedRevision":revision,
                      "reason":{"type":"string","minLength":1,"maxLength":1500}}),
               &["kind","decisionId","expectedRevision","reason"]),
        object(json!({"kind":{"const":"recordRisk"},"draft":risk,"owner":owner}), &["kind","draft"]),
        object(json!({"kind":{"const":"updateRisk"},"riskId":risk_id,"expectedRevision":revision,"draft":risk}),
               &["kind","riskId","expectedRevision","draft"]),
        object(json!({"kind":{"const":"assignRisk"},"riskId":risk_id,"expectedRevision":revision,"owner":owner}),
               &["kind","riskId","expectedRevision","owner"]),
        object(json!({"kind":{"const":"reviewRisk"},"riskId":risk_id,"expectedRevision":revision}),
               &["kind","riskId","expectedRevision"]),
        object(json!({"kind":{"const":"setRiskStatus"},"riskId":risk_id,"expectedRevision":revision,
                      "status":{"type":"string","enum":["open","mitigated","closed"]},"reason":bounded_text}),
               &["kind","riskId","expectedRevision","status","reason"]),
        object(json!({"kind":{"const":"recordRiskOccurrence"},"riskId":risk_id,"expectedRevision":revision,
                      "lessonDraft":lesson(7)}), &["kind","riskId","expectedRevision","lessonDraft"]),
        object(json!({"kind":{"const":"recordLesson"},"draft":lesson(8)}), &["kind","draft"]),
        object(json!({"kind":{"const":"confirmLesson"},"lessonId":lesson_id,"expectedRevision":revision}),
               &["kind","lessonId","expectedRevision"]),
        object(json!({"kind":{"const":"staleLesson"},"lessonId":lesson_id,"expectedRevision":revision,
                      "reason":bounded_text}), &["kind","lessonId","expectedRevision","reason"]),
    ]})
}

fn decision_ask_draft_schema() -> Value {
    let object = |properties: Value, required: &[&str]| json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    let option = object(
        json!({"label":{"type":"string","minLength":1,"maxLength":120}}),
        &["label"],
    );
    let about = object(
        json!({
            "kind":{"type":"string","enum":["crew","work","requirement","row","risk"]},
            "id":{"type":"string","minLength":1,"maxLength":128},
        }),
        &["kind", "id"],
    );
    let default = object(
        json!({
            "optionId":{"type":"string","enum":["A","B","C","D","E","F","G","H"]},
            "afterMinutes":{"type":"integer","minimum":1,"maximum":10080},
        }),
        &["optionId", "afterMinutes"],
    );
    object(
        json!({
            "question":{"type":"string","minLength":1,"maxLength":60},
            "context":{"type":"string","minLength":1,"maxLength":900},
            "options":{"type":"array","minItems":2,"maxItems":8,"uniqueItems":true,"items":option},
            "about":{"type":"array","minItems":1,"maxItems":8,"uniqueItems":true,"items":about},
            "default":{"oneOf":[default,{"type":"null"}]},
            "reversibility":{"type":["string","null"],"enum":["reversible","hardToReverse","irreversible",null]},
        }),
        &["question", "context", "options", "about"],
    )
}

/// Closed unions are authored once in proto. Inline their exact schema for MCP tools.
fn managed_parameter(method: &str, parameter: &str) -> Value {
    static API: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let api = API.get_or_init(crewd_proto::api_schema);
    fn inline(value: &Value, api: &Value, depth: u16) -> Value {
        assert!(depth < 128, "unexpected recursive V2 parameter schema");
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    let path = reference
                        .strip_prefix('#')
                        .expect("local proto schema reference");
                    let target = api.pointer(path).expect("resolved proto schema reference");
                    let mut expanded = inline(target, api, depth + 1);
                    for (key, value) in object {
                        if key != "$ref" {
                            expanded[key] = inline(value, api, depth + 1)
                        }
                    }
                    return expanded;
                }
                Value::Object(
                    object
                        .iter()
                        .map(|(key, value)| (key.clone(), inline(value, api, depth + 1)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|value| inline(value, api, depth + 1))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let params = inline(&api["methods"][method]["params"], api, 0);
    let result = params["properties"][parameter].clone();
    assert!(
        result.is_object(),
        "V2 tool parameter must exist in actual proto"
    );
    result
}

pub fn input_schema(t: &ToolSpec, loc: Locale) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    let legacy_fields = crewd_proto::managed_table_tools::legacy_fields(t.name);
    for prm in t.params {
        if legacy_fields.is_some_and(|fields| !fields.contains(&prm.name)) {
            continue;
        }
        let mut s = match prm.ty {
            Str => json!({"type": "string"}),
            Int => json!({"type": "integer"}),
            Num => json!({"type": "number"}),
            Bool => json!({"type": "boolean"}),
            StrList => json!({"type": "array", "items": {"type": "string"}}),
            Ref => json!({"type": ["string", "integer"]}),
            Evidence => json!({"type":"object","properties":{
                "kind":{"type":"string","enum":["commit","url","text"]},"value":{"type":"string"}
            },"required":["kind","value"],"additionalProperties":false}),
            ChannelWorkObject => {
                json!({"type":"object","properties":{"kind":{"type":"string","enum":["requirement","work","humanTask"]},"id":{"type":"string","maxLength":4096}},"required":["kind","id"],"additionalProperties":false})
            }
            FolderObject => {
                json!({"type":"object","properties":{"kind":{"type":"string","enum":["channel","requirement","work","humanTask","document"]},"id":{"type":"string","maxLength":4096}},"required":["kind","id"],"additionalProperties":false})
            }
            DocumentEdits => {
                json!({"type":"array","minItems":1,"maxItems":20,"items":{"type":"object","properties":{"start":{"type":"integer","minimum":0,"maximum":1048576},"end":{"type":"integer","minimum":0,"maximum":1048576},"text":{"type":"string","maxLength":16384}},"required":["start","end","text"],"additionalProperties":false}})
            }
            WorkBlockers => work_blockers_schema(),
            WorkContinuation => work_continuation_schema(loc),
            ManagedObjectScope => json!({"type":"object","properties":{
                "dbId":{"type":"string"},"restoreEpoch":{"type":"string"}
            },"required":["dbId","restoreEpoch"],"additionalProperties":false}),
            ArtifactHistoryCursor => json!({"type":"object","properties":{
                "phase":{"type":"string","enum":["legacyEvents","legacySnapshots","events","lessons","complete"]},
                "after":{"type":"string"}
            },"required":["phase","after"],"additionalProperties":false}),
            ArtifactInput => artifact_input_schema(),
            ArtifactReference => artifact_reference_schema(),
            ArtifactSubject => artifact_subject_schema(),
            ArtifactChange => artifact_change_schema(),
            ManagedTableV2Operation => {
                managed_parameter(crewd_proto::managed_tables_v2::TABLE_SUBMIT, "operation")
            }
            ManagedTableHistorySource => {
                managed_parameter(crewd_proto::managed_tables_v3::HISTORY, "source")
            }
            ManagedTableHistoryKey => {
                managed_parameter(crewd_proto::managed_tables_v3::HISTORY_VALUE, "key")
            }
            ManagedTableHistoryPath => {
                managed_parameter(crewd_proto::managed_tables_v3::HISTORY_VALUE, "path")
            }
            ManagedLinkV2Operation => {
                managed_parameter(crewd_proto::managed_tables_v2::LINK_SUBMIT, "operation")
            }
            ManagedLinkQuery => {
                managed_parameter(crewd_proto::managed_tables_v2::LINK_LIST, "query")
            }
            ManagedLinkTarget => managed_parameter(
                crewd_proto::managed_tables_v2::MANAGED_LINK_RESOLVE_TARGET,
                "target",
            ),
            ManagedObjectChange => managed_change_schema(),
            ManagedMessageChange => {
                let mut schema = managed_change_schema();
                schema["oneOf"].as_array_mut().unwrap().retain(|change| {
                    matches!(
                        change["properties"]["kind"]["const"].as_str(),
                        Some("recordDecision" | "recordRisk" | "recordLesson" | "addRow")
                    )
                });
                schema
            }
            DecisionAskDraft => decision_ask_draft_schema(),
            SecretaryReferences => {
                json!({"type":"array","minItems":1,"maxItems":40,"items":{"type":"object","properties":{"kind":{"type":"string","enum":["human_wait","case","patrol"]},"id":{"type":"string","minLength":1,"maxLength":256}},"required":["kind","id"],"additionalProperties":false}})
            }
            CoordinationChildren => json!({"type":"array","minItems":2,"maxItems":8,"items":{
                "type":"object",
                "properties":{
                    "title":{"type":"string","maxLength":200},
                    "goal":{"type":"string","maxLength":2000},
                    "acceptance":{"type":"string","maxLength":2000},
                    "owner":{"type":["string","null"]},
                    "depends_on":{"type":"array","maxItems":16,"items":{"type":"string"}}
                },
                "required":["title","goal","acceptance","owner"],
                "additionalProperties":false
            }}),
            Mentions => json!({"type": "array", "items": {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": MENTION_KINDS},
                    "target_id": {"type": "string"},
                },
                "required": ["kind"],
                "additionalProperties": false,
            }}),
            Messages => json!({"type": "array", "maxItems": 6, "items": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "category": {"type": "string", "enum": CATEGORIES},
                },
                "required": ["text"],
                "additionalProperties": false,
            }}),
        };
        if let Some((lo, hi)) = prm.bounds {
            s["minimum"] = json!(lo);
            s["maximum"] = json!(hi);
        }
        if !prm.choices.is_empty() {
            s["enum"] = json!(prm.choices);
        }
        let key = format!("{}.{}", t.name, prm.name);
        s["description"] = Value::String(text::get(loc, &key).to_string());
        props.insert(prm.name.to_string(), s);
        if prm.required {
            required.push(prm.name);
        }
    }
    let mut schema = json!({
        "type": "object",
        "properties": props,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    if legacy_fields.is_some() {
        versioned_table_schema(t, loc, schema)
    } else {
        schema
    }
}

fn versioned_table_schema(t: &ToolSpec, loc: Locale, mut legacy: Value) -> Value {
    let selector_description = text::get(loc, &format!("{}.contractVersion", t.name));
    legacy["properties"]["contractVersion"] = json!({
        "type":"integer", "const":2, "description":selector_description,
    });
    let new = crewd_proto::managed_table_tools::version_three_schema(t.name)
        .expect("versioned tool must have a closed V3 DTO");
    let mut branches = vec![legacy];
    let alternatives = new
        .get("oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| vec![new]);
    for mut branch in alternatives {
        assert_eq!(branch["type"], "object");
        let properties = branch["properties"]
            .as_object_mut()
            .expect("typed object properties");
        for (name, property) in properties.iter_mut() {
            assert!(
                t.params.iter().any(|parameter| parameter.name == name),
                "typed tool field must have catalog metadata"
            );
            property["description"] =
                Value::String(text::get(loc, &format!("{}.{name}", t.name)).into());
        }
        properties.insert(
            "contractVersion".into(),
            json!({
                "type":"integer", "const":3, "description":selector_description,
            }),
        );
        let required = branch
            .as_object_mut()
            .unwrap()
            .entry("required")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("typed required fields");
        required.push(json!("contractVersion"));
        branches.push(branch);
    }

    // Keep every distinct parameter explanation once in the rendered union;
    // top-level properties also retain the complete parameter documentation.
    let mut documented = std::collections::BTreeSet::new();
    for branch in &mut branches {
        for (name, property) in branch["properties"].as_object_mut().unwrap() {
            if let Some(description) = property.get("description").and_then(Value::as_str)
                && !documented.insert((name.clone(), description.to_owned()))
            {
                property.as_object_mut().unwrap().remove("description");
            }
        }
    }

    // The closed oneOf branches own all types and required keys. The outer
    // object only permits their known names; repeating every full property
    // union here adds no constraint and exceeds the pinned SDK's byte budget.
    let mut properties = Map::new();
    for branch in &branches {
        for name in branch["properties"]
            .as_object()
            .expect("closed branch")
            .keys()
        {
            properties.entry(name.clone()).or_insert_with(|| {
                json!({
                    "description":text::get(loc, &format!("{}.{name}", t.name)),
                })
            });
        }
    }
    let schema = crewd_proto::managed_table_tools::coalesce_tagged_schema(
        json!({"type":"object","properties":properties,"oneOf":branches,"additionalProperties":false}),
    ).expect("fixed versioned table schema is bounded");
    assert!(
        serde_json::to_vec(&schema)
            .expect("serializable fixed schema")
            .len()
            <= crewd_proto::managed_table_tools::TOOL_SCHEMA_MAX_BYTES,
        "fixed versioned table schemas must fit the table wire budget",
    );
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Identity, Role};
    use crewd_collab::Locale;

    /// The supplements' removed list (index "Removed without a same-shape successor" + merged).
    const M3B_REMOVED: &[&str] = &[
        "contact",
        "report_to_parent",
        "message_child_crew",
        "human_watching",
        "channel_transfer",
        "channel_accept",
        "channel_unlisten",
        "handoff_captain_to_session",
        "create_and_handoff_captain",
        "read_delegation_policy",
        "update_delegation_policy",
        "adopt_crew",
        "release_crew",
        "create_parent_crew",
        "adopt_parent",
        "create_child_crew",
    ];

    fn who(role: Role) -> Identity {
        Identity {
            session_id: "s-1".into(),
            crew_id: "crew-a".into(),
            role,
            label: "示例".into(),
        }
    }

    fn names(id: &Identity) -> Vec<String> {
        tools_for(id, Locale::Zh)
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    #[test]
    fn names_are_unique_and_removed_tools_are_absent() {
        let mut seen = std::collections::BTreeSet::new();
        for t in CATALOG {
            assert!(seen.insert(t.name), "重名 {}", t.name);
        }
        for (gone, _) in REMOVED {
            assert!(find(gone).is_none(), "{gone} 应已删除（§9 / §22.7 Q4）");
        }
    }

    /// Every removed name is absent, listed once, and a non-empty replacement starts with a tool
    /// that exists; the names merged or removed in M3b (the supplements' list) all name one.
    #[test]
    fn removed_names_are_absent_and_name_a_replacement() {
        let mut seen = std::collections::BTreeSet::new();
        for (old, new) in REMOVED {
            assert!(seen.insert(*old), "{old} listed twice");
            assert!(find(old).is_none(), "{old} is still in CATALOG");
            if !new.is_empty() {
                let tool = new.split('(').next().unwrap();
                assert!(find(tool).is_some(), "{old} -> {new}: {tool} is not a tool");
            }
        }
        for old in M3B_REMOVED {
            assert!(
                replacement(old).is_some(),
                "{old} must name its replacement in REMOVED"
            );
        }
        assert_eq!(replacement("post"), None);
        assert_eq!(replacement("continue_work"), None, "no successor");
    }

    #[test]
    fn worker_and_captain_get_different_tables() {
        let worker = names(&who(Role::Worker));
        let captain = names(&who(Role::Captain));
        assert!(worker.contains(&"post".to_string()));
        assert!(!worker.contains(&"start_session".to_string()));
        assert!(captain.contains(&"start_session".to_string()));
        assert!(!worker.contains(&"admin_case_handoff_accept".to_string()));
        assert!(captain.contains(&"admin_case_handoff_accept".to_string()));
        for tool in ["admin_case_custody_get", "admin_case_custody_list"] {
            assert!(worker.contains(&tool.to_string()));
            assert!(captain.contains(&tool.to_string()));
        }
        assert!(!worker.contains(&"admin_case_custody_accept".to_string()));
        assert!(captain.contains(&"admin_case_custody_accept".to_string()));
        assert!(
            !worker
                .iter()
                .any(|name| name == crewd_collab::work_coordination::SPLIT_TOOL)
        );
        assert!(
            captain
                .iter()
                .any(|name| name == crewd_collab::work_coordination::SPLIT_TOOL)
        );
    }

    #[test]
    fn secretary_purpose_has_only_the_three_closed_tools() {
        let me = who(Role::Admin);
        let got: std::collections::BTreeSet<_> = case_tools_for_purpose(&me, Locale::Zh, true)
            .into_iter()
            .map(|t| t.name)
            .collect();
        let expected = std::collections::BTreeSet::from([
            crewd_proto::admin_secretary::SNAPSHOT_TOOL.to_string(),
            crewd_proto::admin_secretary::PUBLISH_TOOL.to_string(),
            crewd_proto::admin_secretary::RECEIPT_TOOL.to_string(),
        ]);
        assert_eq!(got, expected);
        for role in [Role::Worker, Role::Captain, Role::Admin] {
            assert!(names(&who(role)).iter().all(|t| !expected.contains(t)));
        }
        assert!(
            case_tools_for(&me, Locale::Zh)
                .iter()
                .all(|t| !expected.contains(&t.name))
        );
    }

    /// S76: every tool carries its hints (values = the index table); passive (B14) is derived
    /// from them: read-only or talk.
    #[test]
    fn annotations_are_on_every_tool_and_passive_is_derived() {
        let mut defs = tools_for(&who(Role::Captain), Locale::Zh);
        defs.extend(
            case_tools_for(&who(Role::Admin), Locale::Zh)
                .into_iter()
                .filter(|tool| find(&tool.name).is_some_and(|spec| spec.scope == Scope::CaseAdmin)),
        );
        defs.extend(case_tools_for_purpose(&who(Role::Admin), Locale::Zh, true));
        assert_eq!(defs.len(), CATALOG.len());
        for d in &defs {
            let h = d
                .annotations
                .unwrap_or_else(|| panic!("{} has no annotations", d.name));
            let t = find(&d.name).unwrap();
            assert_eq!(h, t.hints(), "{}", d.name);
            assert!(!h.open_world_hint, "{}", d.name);
            if h.read_only_hint {
                assert!(
                    t.passive() && h.idempotent_hint && !h.destructive_hint,
                    "{}",
                    d.name
                );
            }
        }
        let with = |f: fn(&ToolSpec) -> bool| -> Vec<&str> {
            CATALOG.iter().filter(|t| f(t)).map(|t| t.name).collect()
        };
        assert_eq!(
            with(|t| t.read_only),
            [
                "admin_case_custody_get",
                "admin_case_custody_list",
                "admin_case_handoff_get",
                "admin_case_handoff_list",
                "my_work_items",
                "channel_work_list",
                "document_get",
                "document_list",
                "view_folder_list",
                "todo_get",
                crewd_collab::requirement_ruling::CAPTAIN_CHECK_TOOL,
                "work_closure_history",
                "work_item_get",
                "work_item_history",
                "read_channel",
                "search",
                "organization_search_expand",
                "list_channels",
                "channel_archive_check",
                "directory",
                "ask_decision_request_get",
                "get_quota",
                "inspect_session",
                "list_sessions",
                "managed_table_v2_get",
                "managed_tables_v2_list",
                "managed_row_v2_get",
                "managed_rows_v2_list",
                "managed_table_v2_request_get",
                "managed_link_resolve_target",
                "managed_links_list",
                "managed_link_request_get",
                "artifact_history_status",
                "artifact_history_request_get",
                "artifact_preview",
                "artifacts_list",
                "artifact_get",
                "artifact_backlinks",
                "artifact_subject",
                "artifact_request_get",
                "managed_object_scope_get",
                "managed_object_get",
                "managed_tables_list",
                "managed_rows_list",
                "managed_decisions_list",
                "managed_risks_list",
                "managed_lessons_list",
                "managed_message_request_get",
                "managed_object_source_get",
                "managed_objects_from_message",
                "managed_object_request_get",
            ]
        );
        assert_eq!(
            with(|t| t.destructive),
            [
                "channel_work_unlink",
                "document_delete",
                "view_folder_delete",
                "channel_archive",
                "channel_archive_verified",
                "change_workdir",
                "stop_session",
                "handoff_captain",
                "crew_structure",
                "managed_table_v2_submit",
                "managed_link_submit",
                "artifact_submit",
                "managed_object_from_message",
                "managed_object_submit",
            ]
        );
        assert_eq!(
            with(|t| t.idempotent && !t.read_only),
            [
                "admin_case_custody_accept",
                "admin_case_handoff_request",
                "admin_case_handoff_accept",
                "channel_work_link",
                "channel_work_unlink",
                "document_create",
                "document_update",
                "document_delete",
                "document_patch",
                "view_folder_create",
                "view_folder_update",
                "view_folder_delete",
                "view_folder_assign",
                "work_item_transfer_request",
                "work_item_transfer_accept",
                "work_item_transfer_withdraw",
                crewd_collab::requirement_ruling::CAPTAIN_CLOSE_TOOL,
                "work_item_close",
                "admin_case_work_unlink",
                "work_item_update",
                "work_item_block",
                "work_item_assign",
                crewd_collab::work_coordination::SPLIT_TOOL,
                "work_item_reassign_lost",
                "channel_listen",
                "ask_decision",
                "set_session_profile",
                "rename_crew",
                "change_workdir",
                "set_crew_status",
                "delegation_policy",
                "managed_table_v2_submit",
                "managed_link_submit",
                "artifact_history_begin",
                "artifact_history_step",
                "artifact_submit",
                "managed_object_from_message",
                "managed_object_submit",
            ]
        );
        assert_eq!(
            with(ToolSpec::passive),
            [
                "admin_case_custody_get",
                "admin_case_custody_list",
                "admin_case_handoff_get",
                "admin_case_handoff_list",
                "my_work_items",
                "channel_work_list",
                "document_get",
                "document_list",
                "view_folder_list",
                "todo_get",
                crewd_collab::requirement_ruling::CAPTAIN_CHECK_TOOL,
                "work_closure_history",
                "work_item_get",
                "work_item_history",
                "post",
                "read_channel",
                "search",
                "organization_search_expand",
                "list_channels",
                "channel_archive_check",
                "channel_listen",
                "directory",
                "ask_decision_request_get",
                "get_quota",
                "inspect_session",
                "list_sessions",
                "managed_table_v2_get",
                "managed_tables_v2_list",
                "managed_row_v2_get",
                "managed_rows_v2_list",
                "managed_table_v2_request_get",
                "managed_link_resolve_target",
                "managed_links_list",
                "managed_link_request_get",
                "artifact_history_status",
                "artifact_history_request_get",
                "artifact_preview",
                "artifacts_list",
                "artifact_get",
                "artifact_backlinks",
                "artifact_subject",
                "artifact_request_get",
                "managed_object_scope_get",
                "managed_object_get",
                "managed_tables_list",
                "managed_rows_list",
                "managed_decisions_list",
                "managed_risks_list",
                "managed_lessons_list",
                "managed_message_request_get",
                "managed_object_source_get",
                "managed_objects_from_message",
                "managed_object_request_get",
            ]
        );
        let stop = find("stop_session").unwrap();
        assert!(stop.destructive && !stop.passive());
    }

    #[test]
    fn schema_is_strict_and_carries_md_descriptions() {
        let post = find("post").unwrap();
        let s = input_schema(post, Locale::Zh);
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(
            s["properties"]["channel"]["description"],
            crate::agent::text::get(Locale::Zh, "post.channel")
        );
        // §24.2: @ only wakes; the `broadcast` mention kind is gone.
        assert_eq!(
            s["properties"]["mentions"]["items"]["properties"]["kind"]["enum"],
            serde_json::json!(["session", "captain", "human"])
        );
        assert_eq!(
            s["properties"]["reply_to"]["type"],
            serde_json::json!(["string", "integer"])
        );
        let ask = input_schema(find("ask").unwrap(), Locale::Zh);
        assert_eq!(ask["required"], serde_json::json!(["question"]));
        let none = input_schema(find("get_quota").unwrap(), Locale::Zh);
        assert_eq!(none["properties"], serde_json::json!({}));
    }

    #[test]
    fn child_crew_text_steers_collaboration_to_channels() {
        // 人 2026-10-06：协作与话题用群，子机组只表达组织结构（部门 / 团队）。
        for loc in [Locale::Zh, Locale::En] {
            assert!(
                crate::agent::text::get(loc, "crew_structure").contains("channel_create"),
                "{loc:?}"
            );
            assert!(
                crate::agent::text::get(loc, "channel_create").len() > 40,
                "{loc:?}"
            );
        }
    }

    #[test]
    fn every_tool_and_param_has_text_in_both_locales() {
        for loc in [Locale::Zh, Locale::En] {
            let have = crate::agent::text::section_names(loc);
            for t in CATALOG {
                assert!(have.contains(&t.name), "{loc:?} 缺 ## {}", t.name);
                for p in t.params {
                    let key = format!("{}.{}", t.name, p.name);
                    assert!(have.iter().any(|h| *h == key), "{loc:?} 缺 ## {key}");
                }
            }
        }
    }

    /// 递归检查：每一层 object 都 `additionalProperties:false`，`required` 的名字都在 `properties` 里，`enum` 非空。
    fn check_schema(path: &str, v: &serde_json::Value) {
        if let Some(obj) = v.as_object() {
            if obj.get("type") == Some(&serde_json::json!("object")) {
                assert_eq!(
                    obj.get("additionalProperties"),
                    Some(&serde_json::json!(false)),
                    "{path}"
                );
                let props = obj
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .unwrap_or_else(|| panic!("{path} has no properties"));
                for r in obj
                    .get("required")
                    .and_then(|r| r.as_array())
                    .into_iter()
                    .flatten()
                {
                    let r = r.as_str().unwrap();
                    assert!(
                        props.contains_key(r),
                        "{path}: required {r} not in properties"
                    );
                }
            }
            if let Some(e) = obj.get("enum") {
                assert!(
                    e.as_array().is_some_and(|a| !a.is_empty()),
                    "{path}: empty enum"
                );
            }
            for (k, child) in obj {
                check_schema(&format!("{path}.{k}"), child);
            }
        } else if let Some(arr) = v.as_array() {
            for (i, child) in arr.iter().enumerate() {
                check_schema(&format!("{path}[{i}]"), child);
            }
        }
    }

    #[test]
    fn versioned_table_tools_fit_the_wire_budget() {
        let mut evidence = Vec::new();
        for loc in [Locale::Zh, Locale::En] {
            for t in CATALOG
                .iter()
                .filter(|t| crewd_proto::managed_table_tools::legacy_fields(t.name).is_some())
            {
                let schema = input_schema(t, loc);
                let encoded = serde_json::to_vec(&schema).unwrap();
                println!("{loc:?} {} inputSchema {} bytes", t.name, encoded.len());
                evidence.push(json!({"locale":format!("{loc:?}"),"tool":t.name,
                    "bytes":encoded.len(),"inputSchema":schema}));
            }
        }
        if let Some(path) = std::env::var_os("PCW_TABLE_SCHEMA_EVIDENCE_PATH") {
            std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
        let budget = crewd_proto::managed_table_tools::TOOL_SCHEMA_MAX_BYTES;
        assert!(crewd_exec::codex::proto::CREW_MCP_INPUT_SCHEMA_MAX_BYTES >= budget);
        for item in evidence {
            assert!(
                item["bytes"].as_u64().unwrap() <= budget as u64,
                "{} {} schema exceeds the actual trusted crew budget {budget}",
                item["locale"],
                item["tool"]
            );
        }
    }

    #[test]
    fn every_schema_is_strict_at_every_object_level() {
        for loc in [Locale::Zh, Locale::En] {
            for t in CATALOG {
                check_schema(t.name, &input_schema(t, loc));
            }
        }
    }

    #[test]
    fn admin_sees_the_existing_worker_set_without_managed_object_tools() {
        let worker: Vec<_> = names(&who(Role::Worker))
            .into_iter()
            .filter(|name| !requires_managed_scope(name))
            .collect();
        assert_eq!(names(&who(Role::Admin)), worker);
    }

    /// S68: no management special case. The management crew's captain gets the table of any
    /// captain, its other sessions the table of any worker (`respond_todo` and `start_session`
    /// included).
    #[tokio::test]
    async fn management_crew_sees_the_same_tables_as_any_crew() {
        use crate::agent::testkit::Fixture;
        let f = Fixture::new().await;
        let mgmt = f.management_crew().await;
        f.add_session(&mgmt, "s-boss", Role::Captain, "示例总机长")
            .await;
        f.add_session(&mgmt, "s-mw", Role::Worker, "示例管理 worker")
            .await;
        let of = |id: Identity| names(&id);
        let boss = of(f.me("s-boss").await);
        assert_eq!(boss, of(f.me(&f.captain).await));
        assert_eq!(of(f.me("s-mw").await), of(f.me(&f.worker).await));
        for t in ["respond_todo", "start_session", "ask"] {
            assert!(boss.contains(&t.to_string()), "{t}");
        }
        assert_eq!(
            boss,
            CATALOG
                .iter()
                .filter(|t| t.scope != Scope::CaseAdmin)
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn md_has_no_orphan_tool_or_param_sections() {
        for loc in [Locale::Zh, Locale::En] {
            for h in crate::agent::text::section_names(loc) {
                if h == "instructions"
                    || ["r.", "trace.", "inject."].iter().any(|p| h.starts_with(p))
                {
                    continue;
                }
                let (tool, param) = match h.split_once('.') {
                    Some((t, p)) => (t, Some(p)),
                    None => (h, None),
                };
                let spec =
                    find(tool).unwrap_or_else(|| panic!("{loc:?}: ## {h} has no tool in CATALOG"));
                if let Some(p) = param {
                    assert!(
                        spec.params.iter().any(|x| x.name == p),
                        "{loc:?}: ## {h} has no param in CATALOG"
                    );
                }
            }
        }
    }

    #[test]
    fn numeric_bounds_match_the_md() {
        let prop = |tool: &str, param: &str| {
            input_schema(find(tool).unwrap(), Locale::Zh)["properties"][param].clone()
        };
        for (tool, param, max) in [
            ("read_channel", "limit", 200),
            ("search", "limit", 200),
            ("channel_listen", "minutes", 480),
            ("schedule_wakeup", "after_minutes", 1440),
        ] {
            let s = prop(tool, param);
            assert_eq!(s["type"], "integer", "{tool}.{param}");
            assert_eq!(s["minimum"], 1, "{tool}.{param}");
            assert_eq!(s["maximum"], max, "{tool}.{param}");
        }
    }
}

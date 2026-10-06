//! 工具表（spec §22.3 表 + §22.7 Q2 / Q4）。只有名字、范围、参数形状；文字全在 tools.*.md。
use serde_json::{Map, Value, json};

use crate::agent::text::{self, Locale};
use crate::agent::{Identity, Role};
use crewd_proto::agent::ToolDef;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Captain,
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
    /// `[{text, category?}]`
    Messages,
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub required: bool,
    pub choices: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub scope: Scope,
    /// 内建总机组上不列（`McpServer.swift:565`、`:745-753`）。
    pub not_on_builtin: bool,
    pub params: &'static [Param],
}

pub const CATEGORIES: &[&str] = &[
    "progress", "done", "blocked", "question", "finding", "handoff", "ack", "note",
];
pub const MENTION_KINDS: &[&str] = &["session", "captain", "human", "broadcast"];
const RUNNERS: &[&str] = &["claude", "codex"];
const CHANNEL_KINDS: &[&str] = &["topic", "dm"];

const fn p(name: &'static str, ty: Ty, required: bool) -> Param {
    Param {
        name,
        ty,
        required,
        choices: &[],
    }
}
const fn c(name: &'static str, required: bool, choices: &'static [&'static str]) -> Param {
    Param {
        name,
        ty: Ty::Str,
        required,
        choices,
    }
}
const fn all(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec {
        name,
        scope: Scope::All,
        not_on_builtin: false,
        params,
    }
}
const fn cap(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec {
        name,
        scope: Scope::Captain,
        not_on_builtin: false,
        params,
    }
}
const fn not_builtin(t: ToolSpec) -> ToolSpec {
    ToolSpec {
        not_on_builtin: true,
        ..t
    }
}

use Ty::*;

pub const CATALOG: &[ToolSpec] = &[
    all(
        "post",
        &[
            p("channel", Str, false),
            p("message", Str, false),
            p("messages", Messages, false),
            p("mentions", Mentions, false),
            p("reply_to", Ref, false),
            c("category", false, CATEGORIES),
        ],
    ),
    all(
        "read_channel",
        &[
            p("channel", Str, false),
            p("limit", Int, false),
            p("before", Ref, false),
        ],
    ),
    all(
        "search",
        &[
            p("query", Str, true),
            p("channel", Str, false),
            p("after", Str, false),
            p("before", Str, false),
            p("limit", Int, false),
        ],
    ),
    all("list_channels", &[]),
    all("human_watching", &[]),
    all(
        "channel_create",
        &[
            p("name", Str, true),
            p("purpose", Str, false),
            p("members", StrList, true),
            c("kind", false, CHANNEL_KINDS),
        ],
    ),
    all(
        "channel_invite",
        &[p("channel", Str, true), p("members", StrList, true)],
    ),
    all("channel_leave", &[p("channel", Str, true)]),
    all("channel_archive", &[p("channel", Str, true)]),
    all(
        "channel_transfer",
        &[
            p("channel", Str, true),
            p("to", Str, true),
            p("force", Bool, false),
        ],
    ),
    all("channel_accept", &[p("channel", Str, true)]),
    all(
        "channel_listen",
        &[
            p("channel", Str, true),
            p("minutes", Num, false),
            p("senders", StrList, false),
        ],
    ),
    all("channel_unlisten", &[p("channel", Str, true)]),
    all("directory", &[p("query", Str, false)]),
    all("contact", &[p("to", Str, true), p("message", Str, true)]),
    not_builtin(all(
        "ask",
        &[
            p("question", Str, true),
            p("resume_note", Str, false),
            p("fallback_after_minutes", Num, false),
            p("fallback", Str, false),
        ],
    )),
    all(
        "withdraw_ask",
        &[p("ask", Str, true), p("reason", Str, true)],
    ),
    not_builtin(all(
        "respond_todo",
        &[p("number", Int, true), p("response", Str, true)],
    )),
    all("get_quota", &[]),
    all(
        "schedule_wakeup",
        &[
            p("after_minutes", Num, false),
            p("at", Str, false),
            p("note", Str, true),
        ],
    ),
    all(
        "set_session_profile",
        &[p("model", Str, false), p("effort", Str, false)],
    ),
    cap("rename_crew", &[p("name", Str, true)]),
    cap(
        "change_workdir",
        &[
            p("new_path", Str, true),
            p("crew", Str, false),
            p("include_children", Bool, false),
            p("confirm", Bool, false),
        ],
    ),
    cap("set_crew_status", &[p("text", Str, true)]),
    cap("read_delegation_policy", &[]),
    cap("update_delegation_policy", &[p("text", Str, true)]),
    not_builtin(cap(
        "start_session",
        &[
            p("brief", Str, true),
            p("title", Str, false),
            c("runner", false, RUNNERS),
            p("isolation", Bool, true),
            p("model", Str, false),
            p("effort", Str, false),
        ],
    )),
    cap(
        "stop_session",
        &[p("session_id", Str, true), p("reason", Str, true)],
    ),
    cap("inspect_session", &[p("session_id", Str, true)]),
    cap(
        "nudge_session",
        &[p("session_id", Str, true), p("input", Str, true)],
    ),
    cap("list_sessions", &[]),
    cap(
        "handoff_captain_to_session",
        &[p("session_id", Str, true), p("target_crew_id", Str, false)],
    ),
    cap(
        "create_and_handoff_captain",
        &[
            c("runner", true, RUNNERS),
            p("model", Str, false),
            p("effort", Str, false),
            p("title", Str, false),
            p("opening_brief", Str, false),
            p("target_crew_id", Str, false),
        ],
    ),
    cap("report_to_parent", &[p("message", Str, true)]),
    cap(
        "message_child_crew",
        &[p("crew", Str, true), p("message", Str, true)],
    ),
    cap("adopt_crew", &[p("crew", Str, true)]),
    cap("release_crew", &[p("crew", Str, true), p("to", Str, false)]),
    cap("create_parent_crew", &[p("title", Str, false)]),
    cap("adopt_parent", &[p("crew", Str, true)]),
    cap(
        "create_child_crew",
        &[
            p("brief", Str, true),
            p("title", Str, false),
            c("runner", false, RUNNERS),
        ],
    ),
];

/// 已删的旧工具名（§9、§22.3、§22.7 Q4）与改了名的旧名：测试断言它们不在表里。
pub const REMOVED: &[&str] = &[
    "continue_work",
    "confirm_todo_sweep",
    "plan_add",
    "plan_update",
    "plan_list",
    "raise_attention",
    "clear_attention",
    "crew_ordering_signals",
    "arrange_crews",
    "post_to_crew",
    "read_whiteboard",
    "search_whiteboard",
    "add_human_todo",
    "withdraw_human_todo",
    "listen",
    "answer_decision",
];

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    CATALOG.iter().find(|t| t.name == name)
}

/// 这个身份此刻能不能看见 / 调这个工具。
pub fn visible(t: &ToolSpec, me: &Identity) -> bool {
    (t.scope == Scope::All || me.role == Role::Captain) && !(t.not_on_builtin && me.builtin_crew)
}

pub fn tools_for(me: &Identity, loc: Locale) -> Vec<ToolDef> {
    CATALOG
        .iter()
        .filter(|t| visible(t, me))
        .map(|t| ToolDef {
            name: t.name.to_string(),
            description: text::get(loc, t.name).to_string(),
            input_schema: input_schema(t, loc),
        })
        .collect()
}

pub fn input_schema(t: &ToolSpec, loc: Locale) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for prm in t.params {
        let mut s = match prm.ty {
            Str => json!({"type": "string"}),
            Int => json!({"type": "integer"}),
            Num => json!({"type": "number"}),
            Bool => json!({"type": "boolean"}),
            StrList => json!({"type": "array", "items": {"type": "string"}}),
            Ref => json!({"type": ["string", "integer"]}),
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
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Identity, Role};
    use crewd_collab::Locale;

    fn who(role: Role, builtin: bool) -> Identity {
        Identity {
            session_id: "s-1".into(),
            crew_id: "crew-a".into(),
            role,
            label: "示例".into(),
            builtin_crew: builtin,
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
        for gone in REMOVED {
            assert!(find(gone).is_none(), "{gone} 应已删除（§9 / §22.7 Q4）");
        }
        assert_eq!(CATALOG.len(), 40);
    }

    #[test]
    fn worker_and_captain_get_different_tables() {
        let worker = names(&who(Role::Worker, false));
        let captain = names(&who(Role::Captain, false));
        assert!(worker.contains(&"post".to_string()));
        assert!(!worker.contains(&"start_session".to_string()));
        assert!(captain.contains(&"start_session".to_string()));
        assert_eq!(worker.len(), 21);
        assert_eq!(captain.len(), 40);
    }

    #[test]
    fn builtin_crew_hides_ask_respond_and_start_session() {
        let chief = names(&who(Role::Captain, true));
        for hidden in ["ask", "respond_todo", "start_session"] {
            assert!(!chief.contains(&hidden.to_string()), "{hidden}");
        }
        assert!(chief.contains(&"report_to_parent".to_string()));
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
        assert_eq!(
            s["properties"]["mentions"]["items"]["properties"]["kind"]["enum"][3],
            "broadcast"
        );
        assert_eq!(
            s["properties"]["reply_to"]["type"],
            serde_json::json!(["string", "integer"])
        );
        let ask = input_schema(find("ask").unwrap(), Locale::Zh);
        assert_eq!(ask["required"], serde_json::json!(["question"]));
        let none = input_schema(find("list_channels").unwrap(), Locale::Zh);
        assert_eq!(none["properties"], serde_json::json!({}));
    }

    #[test]
    fn child_crew_text_steers_collaboration_to_channels() {
        // 人 2026-10-06：协作与话题用群，子机组只表达组织结构（部门 / 团队）。
        for loc in [Locale::Zh, Locale::En] {
            assert!(
                crate::agent::text::get(loc, "create_child_crew").contains("channel_create"),
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
}

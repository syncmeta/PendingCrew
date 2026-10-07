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
    /// 整数参数的闭区间（md 里写的范围；schema 的 minimum / maximum）。
    pub bounds: Option<(i64, i64)>,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub scope: Scope,
    pub params: &'static [Param],
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
    }
}
const fn cap(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec {
        name,
        scope: Scope::Captain,
        params,
    }
}

use Ty::*;

pub const CATALOG: &[ToolSpec] = &[
    all(
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
    ),
    all(
        "read_channel",
        &[
            p("channel", Str, false),
            int("limit", 1, 200),
            p("before", Ref, false),
        ],
    ),
    all(
        "search",
        &[
            p("query", Str, true),
            p("channel", Str, false),
            p("crew", Str, false),
            p("after", Str, false),
            p("before", Str, false),
            int("limit", 1, 200),
        ],
    ),
    all(
        "list_channels",
        &[p("crew", Str, false), p("include_archived", Bool, false)],
    ),
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
    all("channel_archive", &[p("channel", Str, true)]),
    all(
        "channel_owner",
        &[
            p("channel", Str, true),
            p("to", Str, false),
            p("force", Bool, false),
        ],
    ),
    all(
        "channel_listen",
        &[
            p("channel", Str, true),
            int("minutes", 1, 480),
            p("senders", StrList, false),
            p("off", Bool, false),
        ],
    ),
    all("directory", &[p("query", Str, false)]),
    all(
        "ask",
        &[
            p("question", Str, true),
            p("resume_note", Str, false),
            p("fallback_after_minutes", Num, false),
            p("fallback", Str, false),
        ],
    ),
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
    all("get_quota", &[]),
    all(
        "schedule_wakeup",
        &[
            int("after_minutes", 1, 1440),
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
    cap(
        "stop_session",
        &[p("session_id", Str, true), p("reason", Str, true)],
    ),
    cap("inspect_session", &[p("session_id", Str, true)]),
    cap(
        "nudge_session",
        &[p("session_id", Str, true), c("key", true, NUDGE_KEYS)],
    ),
    cap("list_sessions", &[p("crew", Str, false)]),
    cap(
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
];

/// What replaces a removed tool (`None`: not a removed name, or gone without a successor).
pub fn replacement(name: &str) -> Option<&'static str> {
    REMOVED
        .iter()
        .find(|(old, _)| *old == name)
        .map(|(_, new)| *new)
        .filter(|new| !new.is_empty())
}

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    CATALOG.iter().find(|t| t.name == name)
}

/// 这个身份此刻能不能看见 / 调这个工具：只看角色。总机组没有特例（S68）：它的机长与任何机组的机长同一张表，
/// 它的其他 session 与任何 worker 同一张表。
pub fn visible(t: &ToolSpec, me: &Identity) -> bool {
    t.scope == Scope::All || me.role == Role::Captain
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
        assert_eq!(CATALOG.len(), 33);
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
        assert_eq!(worker.len(), 17);
        assert_eq!(captain.len(), 33);
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
    fn every_schema_is_strict_at_every_object_level() {
        for loc in [Locale::Zh, Locale::En] {
            for t in CATALOG {
                check_schema(t.name, &input_schema(t, loc));
            }
        }
    }

    #[test]
    fn admin_sees_exactly_the_worker_set() {
        assert_eq!(names(&who(Role::Admin)), names(&who(Role::Worker)));
    }

    /// S68: no management special case. The management crew's captain gets the table of any
    /// captain, its other sessions the table of any worker (`respond_todo` and `start_session`
    /// included).
    #[tokio::test]
    async fn management_crew_sees_the_same_tables_as_any_crew() {
        use crate::agent::testkit::Fixture;
        let f = Fixture::new().await;
        let mgmt = f
            .state
            .db
            .call(|s| crewd_collab::crews::management(s.conn()))
            .await
            .unwrap()
            .unwrap()
            .expect("crew.create creates the management crew");
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
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn md_has_no_orphan_tool_or_param_sections() {
        for loc in [Locale::Zh, Locale::En] {
            for h in crate::agent::text::section_names(loc) {
                if ["r.", "trace.", "inject."].iter().any(|p| h.starts_with(p)) {
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

//! agent 面（spec §22.3）：`crewd mcp` / `crewd hook` 与 serve 之间的 RPC。
//! 只有 `agent.attach` 成功过的连接能调其余三个；`agent.tools_changed` 只推给 attach 过的那条连接，
//! 不是 `events.subscribe` 的主题。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod methods {
    pub const ATTACH: &str = "agent.attach";
    pub const TOOLS: &str = "agent.tools";
    pub const CALL: &str = "agent.call";
    pub const HOOK: &str = "agent.hook";
}

/// serve → helper 的推送：角色变了，工具表要重取。`data` 是 [`ToolsChanged`]。
pub const TOOLS_CHANGED: &str = "agent.tools_changed";

/// `agent.tools_changed` 的 `data`：此刻的角色（线上 `{"role": "captain" | "worker" | "admin"}`；
/// admin 的工具表同 worker，但线上照实报）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolsChanged {
    pub role: crate::Role,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachParams {
    pub session_id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    /// M3a 的 `crewd_proto::Role`（线上 "captain" / "worker"）。
    pub role: crate::Role,
    pub crew_id: String,
    pub label: String,
    /// MCP initialize instructions from the active locale's tools md.
    pub instructions: String,
    /// Stable public surface fingerprint, appended to MCP serverInfo.version.
    pub surface_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// MCP tool annotations (hints for the agent's client). Absent from a serve that predates
    /// them: the helper then lists the tool without annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolHints>,
}

/// MCP `ToolAnnotations` hints, under their MCP wire names (`readOnlyHint` …); `crewd mcp` passes
/// them through as they are. Every crewd tool works inside crewd only: `openWorldHint` is false.
/// Not `deny_unknown_fields`: a newer serve may add a hint (MCP's `title` …) that an older helper
/// should skip, not fail the whole tool list on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolHints {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallParams {
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

impl CallResult {
    pub fn ok(text: String) -> Self {
        Self {
            text,
            is_error: false,
        }
    }
    pub fn err(text: String) -> Self {
        Self {
            text,
            is_error: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HookKind {
    PostToolUse,
    Stop,
    SessionStart,
}

impl HookKind {
    pub const ALL: [HookKind; 3] = [Self::PostToolUse, Self::Stop, Self::SessionStart];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PostToolUse => "post-tool-use",
            Self::Stop => "stop",
            Self::SessionStart => "session-start",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookParams {
    pub event: HookKind,
    /// claude 喂给 hook 的 stdin JSON，原样转来。
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookResult {
    /// helper 原样打到 stdout 的内容；None = 什么都不打。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
}

/// 契约里 agent 那一段（`api_schema()["agent"]`）。**必须用 `api_schema` 的同一个 `SchemaGenerator`**
/// （`crate::sub::<T>(g)`，父模块的私有函数子模块看得见）：类型只进顶层 `$defs`、这里全是 `$ref`。
/// 用 `schema_for!` 会给每个类型自带一份 `$defs`，M3a 的 `every_ref_resolves_to_a_single_top_level_def` 会红。
pub(crate) fn schema(g: &mut schemars::SchemaGenerator) -> Value {
    let method = |params: Value, result: Value| json!({ "params": params, "result": result });
    let mut methods = serde_json::Map::new();
    methods.insert(
        methods::ATTACH.into(),
        method(crate::sub::<AttachParams>(g), crate::sub::<AttachResult>(g)),
    );
    methods.insert(
        methods::TOOLS.into(),
        method(
            json!({ "type": "null" }),
            json!({ "type": "array", "items": crate::sub::<ToolDef>(g) }),
        ),
    );
    methods.insert(
        methods::CALL.into(),
        method(crate::sub::<CallParams>(g), crate::sub::<CallResult>(g)),
    );
    methods.insert(
        methods::HOOK.into(),
        method(crate::sub::<HookParams>(g), crate::sub::<HookResult>(g)),
    );
    // HookKind 只出现在 HookParams 里，`sub::<HookParams>` 已把它放进 `$defs`；这里再取一次保证它有名字。
    let _ = crate::sub::<HookKind>(g);
    let mut push = serde_json::Map::new();
    push.insert(TOOLS_CHANGED.into(), crate::sub::<ToolsChanged>(g));
    json!({ "methods": methods, "push": push })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attach_params_are_camel_case_and_strict() {
        let p: AttachParams =
            serde_json::from_value(json!({"sessionId": "s-1", "token": "t"})).unwrap();
        assert_eq!(p.session_id, "s-1");
        let e = serde_json::from_value::<AttachParams>(
            json!({"sessionId": "s-1", "token": "t", "crew": "x"}),
        )
        .unwrap_err();
        assert!(e.to_string().contains("unknown field"), "{e}");
    }

    #[test]
    fn hook_kind_wire_names() {
        for (k, s) in [
            (HookKind::PostToolUse, "post-tool-use"),
            (HookKind::Stop, "stop"),
            (HookKind::SessionStart, "session-start"),
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(s));
            assert_eq!(k.as_str(), s);
            assert_eq!(HookKind::parse(s), Some(k));
        }
        assert_eq!(HookKind::parse("user-prompt-submit"), None, "M4b 才加");
        // B16: no approvals, so no PreToolUse hook; an old settings file's event is simply unknown.
        assert_eq!(HookKind::parse("pre-tool-use"), None);
    }

    #[test]
    fn call_params_default_args_to_null_and_result_is_camel_case() {
        let p: CallParams = serde_json::from_value(json!({"tool": "post"})).unwrap();
        assert_eq!(p.args, serde_json::Value::Null);
        let r = serde_json::to_value(CallResult::err("x".into())).unwrap();
        assert_eq!(r, json!({"text": "x", "isError": true}));
        assert!(!CallResult::ok("y".into()).is_error);
    }

    #[test]
    fn tools_changed_is_one_role_field() {
        let v = serde_json::to_value(ToolsChanged {
            role: crate::Role::Captain,
        })
        .unwrap();
        assert_eq!(v, json!({"role": "captain"}));
        let back: ToolsChanged = serde_json::from_value(v).unwrap();
        assert_eq!(back.role, crate::Role::Captain);
        assert!(serde_json::from_value::<ToolsChanged>(json!({"role": "worker", "x": 1})).is_err());
        let api = crate::api_schema();
        assert!(
            api["$defs"]["ToolsChanged"].is_object(),
            "顶层 $defs 缺 ToolsChanged"
        );
    }

    #[test]
    fn tool_hints_use_the_mcp_wire_names_and_may_be_absent() {
        let d = ToolDef {
            name: "post".into(),
            description: "x".into(),
            input_schema: json!({"type": "object"}),
            annotations: Some(ToolHints {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
                open_world_hint: false,
            }),
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(
            v["annotations"],
            json!({"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false})
        );
        let old: ToolDef = serde_json::from_value(
            json!({"name": "post", "description": "x", "inputSchema": {"type": "object"}}),
        )
        .unwrap();
        assert_eq!(old.annotations, None);
        // A hint this helper does not know yet is skipped, not a failure.
        let newer: ToolDef = serde_json::from_value(json!({
            "name": "post", "description": "x", "inputSchema": {"type": "object"},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true,
                            "openWorldHint": false, "title": "Post"}
        }))
        .unwrap();
        assert!(newer.annotations.unwrap().read_only_hint);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("annotations")
                .is_none()
        );
    }

    #[test]
    fn hook_result_omits_absent_stdout() {
        assert_eq!(
            serde_json::to_value(HookResult { stdout: None }).unwrap(),
            json!({})
        );
    }

    #[test]
    fn schema_lists_the_four_methods_with_top_level_refs() {
        // 走整份契约：agent 段用 api_schema 的同一个生成器，类型只进顶层 `$defs`
        // （M3a 的 `every_ref_resolves_to_a_single_top_level_def` 管全份；这里核 agent 段自己）。
        let api = crate::api_schema();
        let s = &api["agent"];
        for m in [
            methods::ATTACH,
            methods::TOOLS,
            methods::CALL,
            methods::HOOK,
        ] {
            assert!(s["methods"][m]["params"].is_object(), "{m}");
            assert!(s["methods"][m]["result"].is_object(), "{m}");
        }
        assert!(
            s.get("$defs").is_none() && s.get("types").is_none(),
            "不许自带一份类型表"
        );
        for t in [
            "AttachParams",
            "AttachResult",
            "ToolDef",
            "ToolHints",
            "CallParams",
            "CallResult",
            "HookKind",
            "HookParams",
            "HookResult",
        ] {
            assert!(api["$defs"][t].is_object(), "顶层 $defs 缺 {t}");
        }
    }
}

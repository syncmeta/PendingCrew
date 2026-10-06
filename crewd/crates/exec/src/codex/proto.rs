//! codex app-server 的参数构造与协议判定（Swift `CodexProtocol` 的移植）。全是纯函数。
//!
//! 方法名与字段名逐个对过 `codex app-server generate-json-schema --experimental`
//! （codex-cli 0.160.1，2026-10-06）。schema 管不到的（`config` 里的键、`_meta` 里的值、
//! 服务端报错的原话）在计划的「未核实」表里，靠 Swift 在 0.137 上的真机记录与二进制里的字符串。
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::driver::{Control, UntrustedContext};
use crate::health::{Health, HealthKind};
use crate::session::Profile;

/// `initialize.clientInfo.name`。
pub const CLIENT_NAME: &str = "crewd";

/// 不订阅的通知：只渲染 `item/completed`，增量流对执行器没用。后两条量大（命令输出
/// 逐块推送），`item/completed` 的 `aggregatedOutput` 里有全文。推理增量保留（M2d 可能要）。
pub const OPTED_OUT_NOTIFICATIONS: [&str; 3] = [
    "item/agentMessage/delta",
    "item/commandExecution/outputDelta",
    "item/fileChange/outputDelta",
];

/// 每个回合都带的白板上下文键（codex 按键去重：值没变就不再注入）。
pub const WHITEBOARD_CONTEXT_KEY: &str = "crew_whiteboard";

/// `initialize` 的参数。`experimentalApi` 解锁 `turn/start.additionalContext`；
/// `requestAttestation` 不要（我们不生成上游证明）。
pub fn initialize_params(version: &str) -> Value {
    json!({
        "clientInfo": { "name": CLIENT_NAME, "title": CLIENT_NAME, "version": version },
        "capabilities": {
            "experimentalApi": true,
            "requestAttestation": false,
            "optOutNotificationMethods": OPTED_OUT_NOTIFICATIONS,
        },
    })
}

/// 开 / 接一条 thread 的选项。`None` = 不传，让 codex 按它自己的配置优先级定
/// （人的原话：「codex 本身的默认模型是什么逻辑就用什么逻辑」），实际值从回包里读。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadOptions {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `Some(true)` → `serviceTier: "fast"`，`Some(false)` → `"default"`，`None` 不传。
    pub fast: Option<bool>,
    /// 世界观（M3 渲染），走 `developerInstructions`——经 stdin，`ps` 看不见。
    pub developer_instructions: Option<String>,
    /// `config.mcp_servers` 整块（[`mcp_servers`] 生成）。
    pub mcp_servers: Option<Value>,
}

/// `thread/start`：审批走 codex 自己的 `auto_review`，沙箱 `workspace-write`。
/// MCP 只能挂在 `config.mcp_servers` 下（顶层没有 `mcpServers`，未知键会被静默丢掉）。
pub fn thread_start_params(cwd: &Path, o: &ThreadOptions) -> Value {
    let mut p = Map::new();
    p.insert("cwd".into(), json!(cwd.display().to_string()));
    p.insert("approvalPolicy".into(), json!("on-request"));
    p.insert("sandbox".into(), json!("workspace-write"));
    p.insert("approvalsReviewer".into(), json!("auto_review"));
    if let Some(m) = o.model.as_deref().filter(|m| !m.is_empty()) {
        p.insert("model".into(), json!(m));
    }
    if let Some(fast) = o.fast {
        p.insert("serviceTier".into(), json!(service_tier(fast)));
    }
    if let Some(d) = o
        .developer_instructions
        .as_deref()
        .filter(|d| !d.is_empty())
    {
        p.insert("developerInstructions".into(), json!(d));
    }
    let mut config = Map::new();
    if let Some(e) = o.effort.as_deref().filter(|e| !e.is_empty()) {
        config.insert("model_reasoning_effort".into(), json!(e));
    }
    if let Some(m) = &o.mcp_servers {
        config.insert("mcp_servers".into(), m.clone());
    }
    if !config.is_empty() {
        p.insert("config".into(), Value::Object(config));
    }
    Value::Object(p)
}

/// `thread/resume`：同 `thread/start` 的设置 + `threadId` + `excludeTurns: true`
/// （不带历史回合：长 thread 的整段历史是一行几 MB 的 JSON，Swift 吃过超时的亏）。
pub fn thread_resume_params(thread_id: &str, cwd: &Path, o: &ThreadOptions) -> Value {
    let mut p = thread_start_params(cwd, o);
    p["threadId"] = json!(thread_id);
    p["excludeTurns"] = json!(true);
    p
}

fn service_tier(fast: bool) -> &'static str {
    if fast { "fast" } else { "default" }
}

/// 服务端报的档位 → fast 开关。认不出的新档位返回 `None`（不冒充「标准」）。
pub fn fast_mode_for_tier(tier: &str) -> Option<bool> {
    match tier.to_ascii_lowercase().as_str() {
        "fast" | "priority" => Some(true),
        "default" => Some(false),
        _ => None,
    }
}

/// `config.mcp_servers`：`{"crew": {"command": helper, "args": [...]}}`。
pub fn mcp_servers(helper: &Path, args: &[String]) -> Value {
    json!({ "crew": { "command": helper.display().to_string(), "args": args } })
}

/// `turn/start`：正文是唯一的 `input` 条目；不可信上下文走 `additionalContext`
/// （`kind: "untrusted"`——队友的话是数据，不是给这个 agent 的指令）。空值的键不带。
pub fn turn_start_params(thread_id: &str, text: &str, context: &UntrustedContext) -> Value {
    let mut p = json!({
        "threadId": thread_id,
        "input": [{ "type": "text", "text": text, "text_elements": [] }],
    });
    let entries: Map<String, Value> = context
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), json!({ "value": v, "kind": "untrusted" })))
        .collect();
    if !entries.is_empty() {
        p["additionalContext"] = Value::Object(entries);
    }
    p
}

pub fn turn_interrupt_params(thread_id: &str, turn_id: &str) -> Value {
    json!({ "threadId": thread_id, "turnId": turn_id })
}

/// `thread/settings/update`；`Compact` 不是设置，返回 `None`。
pub fn settings_update_params(thread_id: &str, op: &Control) -> Option<Value> {
    let mut p = json!({ "threadId": thread_id });
    match op {
        Control::Model(m) => p["model"] = json!(m),
        Control::Effort(e) => p["effort"] = json!(e),
        Control::Fast(f) => p["serviceTier"] = json!(service_tier(*f)),
        Control::Compact => return None,
    }
    Some(p)
}

pub fn compact_params(thread_id: &str) -> Value {
    json!({ "threadId": thread_id })
}

/// `thread/start` / `thread/resume` 回包里的 thread id。
pub fn thread_id_of(result: &Value) -> Option<String> {
    result
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 握手回包里 codex 自己解析出来的档位（以它为准，不管我们传没传）。
pub fn profile_of(result: &Value) -> Profile {
    let s = |k: &str| {
        result
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Profile {
        model: s("model"),
        effort: s("reasoningEffort"),
        fast: s("serviceTier").and_then(|t| fast_mode_for_tier(&t)),
    }
}

/// 服务端请求的种类。**每一种都要回**——不回，codex 就在那儿等，回合永远不结束
/// （Swift「运行中…转圈不结束」的病根）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerRequestKind {
    /// `item/*/requestApproval`，以及旧式的 `applyPatchApproval` / `execCommandApproval`。
    Approval,
    /// `mcpServer/elicitation/request`。
    Elicitation,
    /// `account/*`（如 `account/chatgptAuthTokens/refresh`）：登录态失效的强信号。
    Account,
    /// `currentTime/read`：问现在几点，照实回。
    CurrentTime,
    /// 其它一切（`item/tool/requestUserInput`、`item/tool/call`、`attestation/generate`、将来的新方法）。
    Unsupported,
}

pub fn server_request_kind(method: &str) -> ServerRequestKind {
    if method.ends_with("/requestApproval")
        || method == "applyPatchApproval"
        || method == "execCommandApproval"
    {
        ServerRequestKind::Approval
    } else if method == "mcpServer/elicitation/request" {
        ServerRequestKind::Elicitation
    } else if method.starts_with("account/") {
        ServerRequestKind::Account
    } else if method == "currentTime/read" {
        ServerRequestKind::CurrentTime
    } else {
        ServerRequestKind::Unsupported
    }
}

/// 回给服务端的东西。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Result(Value),
    Error { code: i64, message: String },
}

/// 一条服务端请求的处置：回什么、要不要告诉人、是不是登录失效。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRequestOutcome {
    pub reply: Reply,
    /// 执行器替 agent 拒了一个本该人来答的请求——要让人知道（Swift Todo #6）。
    pub notice: Option<String>,
    /// `account/*`：翻 `AuthRequired`。
    pub auth_required: bool,
}

/// 决定怎么回一条服务端请求。`now_unix` = 现在的 Unix 秒（`currentTime/read` 用）。
pub fn answer_server_request(method: &str, params: &Value, now_unix: i64) -> ServerRequestOutcome {
    let unsupported = |notice: bool| ServerRequestOutcome {
        reply: Reply::Error {
            code: -32601,
            message: format!("client does not handle {method}"),
        },
        notice: notice.then(|| unanswerable_notice(method, params)),
        auth_required: false,
    };
    match server_request_kind(method) {
        // auto_review 在 codex 里面把常规审批办了；漏到客户端的审批一律拒，不做审批卡片、
        // 也不悄悄放行。
        ServerRequestKind::Approval => ServerRequestOutcome {
            reply: Reply::Error {
                code: -32000,
                message: "approval request reached client while auto_review is enabled".into(),
            },
            notice: None,
            auth_required: false,
        },
        ServerRequestKind::Elicitation => {
            // codex 把「MCP 工具调用要不要批」做成一条 elicitation，`_meta.codex_approval_kind
            // == "mcp_tool_call"`。crew 只挂我们自己的 `crew` 一个 MCP server，它的工具是 session
            // 自己的通讯神经，必须放行（拒了它 post_to_crew 就发不出去）。真表单没有 UI 可填：拒。
            let tool_call = params
                .pointer("/_meta/codex_approval_kind")
                .and_then(Value::as_str)
                == Some("mcp_tool_call");
            if tool_call {
                ServerRequestOutcome {
                    reply: Reply::Result(json!({ "action": "accept" })),
                    notice: None,
                    auth_required: false,
                }
            } else {
                ServerRequestOutcome {
                    reply: Reply::Result(json!({ "action": "decline" })),
                    notice: Some(unanswerable_notice(method, params)),
                    auth_required: false,
                }
            }
        }
        ServerRequestKind::Account => ServerRequestOutcome {
            auth_required: true,
            ..unsupported(false)
        },
        ServerRequestKind::CurrentTime => ServerRequestOutcome {
            reply: Reply::Result(json!({ "currentTimeAt": now_unix })),
            notice: None,
            auth_required: false,
        },
        ServerRequestKind::Unsupported => unsupported(true),
    }
}

/// 「codex 要一个我们给不出的回答、已代它拒绝」的人话。
pub fn unanswerable_notice(method: &str, params: &Value) -> String {
    let summary = ["message", "prompt", "reason", "question"]
        .iter()
        .find_map(|k| {
            params
                .get(*k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| format!("codex 发来一个需要人回答的请求（{method}）"));
    format!("codex 想要一个回答，执行器给不了，已代它拒绝：{summary}")
}

/// 登录失效（`account/*` 请求）时的 health。
pub fn auth_required_health() -> Health {
    Health::new(
        HealthKind::AuthRequired,
        "Codex 登录态失效（它请求客户端刷新 ChatGPT 令牌，执行器做不到）——在终端跑 codex login 重新登录后重启这个 session。",
    )
}

/// 从协议字段认健康状态（不是从正文猜）：
/// - `account/rateLimits/updated`：`rateLimitReachedType` 非空 / `spendControlReached`
/// - `error`（`willRetry: false`）/ 失败的 `turn/completed`：`codexErrorInfo` =
///   `usageLimitExceeded` / `unauthorized`；「requires a newer version of Codex」；其余失败一律
///   `TurnFailed`（失败不能看起来像空闲）。会重试的 `error` 不算。
pub fn health_from(method: &str, params: &Value) -> Option<Health> {
    if method == "account/rateLimits/updated" {
        let limits = params.get("rateLimits")?;
        if let Some(reached) = limits
            .get("rateLimitReachedType")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Some(Health::new(
                HealthKind::UsageLimit,
                format!("Codex 额度已到上限（{reached}），等额度重置后继续。"),
            ));
        }
        if limits.get("spendControlReached").and_then(Value::as_bool) == Some(true) {
            return Some(Health::new(
                HealthKind::UsageLimit,
                "Codex 用量控制上限已触发，等额度重置后继续。",
            ));
        }
        return None;
    }
    let (error, terminal) = match method {
        "error" => (
            params.get("error"),
            params.get("willRetry").and_then(Value::as_bool) == Some(false),
        ),
        "turn/completed" => {
            let turn = params.get("turn")?;
            let failed = turn.get("status").and_then(Value::as_str) == Some("failed");
            (turn.get("error").filter(|e| !e.is_null()), failed)
        }
        _ => return None,
    };
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if let Some(m) = message
        && m.to_ascii_lowercase()
            .contains("requires a newer version of codex")
    {
        return Some(Health::new(
            HealthKind::VersionIncompatible,
            format!("{m}；本机 Codex 版本太旧，停掉所有 Codex session 后由人升级。"),
        ));
    }
    match error
        .and_then(|e| e.get("codexErrorInfo"))
        .and_then(Value::as_str)
    {
        Some("usageLimitExceeded") => Some(Health::new(
            HealthKind::UsageLimit,
            format!(
                "{}；等额度重置后继续。",
                message.unwrap_or("Codex 额度已到上限")
            ),
        )),
        Some("unauthorized") => Some(Health::new(
            HealthKind::AuthRequired,
            message
                .unwrap_or("Codex 登录态失效，请重新登录后重启这个 session。")
                .to_string(),
        )),
        _ if terminal => Some(Health::new(
            HealthKind::TurnFailed,
            message
                .map(|m| format!("Codex 回合失败：{m}"))
                .unwrap_or_else(|| "Codex 回合失败，服务端没有给错误详情。".to_string()),
        )),
        _ => None,
    }
}

/// 一次 `thread/tokenUsage/updated` 的读数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextUsage {
    pub turn_id: String,
    /// 上下文里现在有多少（`last.totalTokens`）。
    pub context_tokens: i64,
    /// 模型上下文窗口（服务端没给 = 0）。
    pub context_window: i64,
    /// 本 thread 累计消耗（`total.totalTokens`）。
    pub cumulative_tokens: i64,
}

pub fn context_usage(params: &Value, expected_thread: &str) -> Option<ContextUsage> {
    if params.get("threadId").and_then(Value::as_str) != Some(expected_thread) {
        return None;
    }
    let usage = params.get("tokenUsage")?;
    let context_tokens = usage.pointer("/last/totalTokens").and_then(Value::as_i64)?;
    let cumulative_tokens = usage
        .pointer("/total/totalTokens")
        .and_then(Value::as_i64)?;
    let context_window = match usage.get("modelContextWindow") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_i64()?,
    };
    if context_tokens < 0 || cumulative_tokens < 0 || context_window < 0 {
        return None;
    }
    Some(ContextUsage {
        turn_id: params.get("turnId").and_then(Value::as_str)?.to_string(),
        context_tokens,
        context_window,
        cumulative_tokens,
    })
}

/// 自动压缩阈值（Swift `CodexCompactionPolicy`）。
pub const COMPACT_AT: f64 = 0.70;
/// 上下文过了这条线、且这一回合烧掉的 token ≥ 窗口 × [`COMPACT_SPEND_MULTIPLE`] 时也压。
pub const COMPACT_AT_IF_EXPENSIVE: f64 = 0.45;
pub const COMPACT_SPEND_MULTIPLE: f64 = 2.0;
/// 刚压过（距上次压缩不足这么多回合）不再压，免得压缩本身的开销引发循环。
pub const MIN_TURNS_BETWEEN_COMPACTIONS: u32 = 2;

/// 回合结束时要不要触发 codex 自己的压缩器。只在回合边界做，不截断任何东西。
pub fn should_compact(usage: &ContextUsage, turn_tokens: i64, turns_since_compaction: u32) -> bool {
    if usage.context_window <= 0 || turns_since_compaction < MIN_TURNS_BETWEEN_COMPACTIONS {
        return false;
    }
    let window = usage.context_window as f64;
    let pressure = usage.context_tokens as f64 / window;
    pressure >= COMPACT_AT
        || (pressure >= COMPACT_AT_IF_EXPENSIVE
            && turn_tokens.max(0) as f64 >= window * COMPACT_SPEND_MULTIPLE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn opts() -> ThreadOptions {
        ThreadOptions {
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            fast: Some(true),
            developer_instructions: Some("你是 crew 的一员".into()),
            mcp_servers: Some(mcp_servers(
                &PathBuf::from("/opt/crewd"),
                &["mcp".into(), "--agent".into(), "codex".into()],
            )),
        }
    }

    #[test]
    fn initialize_carries_client_info_experimental_api_and_opt_outs() {
        let p = initialize_params("0.1.0");
        assert_eq!(p["clientInfo"]["name"], "crewd");
        assert_eq!(p["clientInfo"]["version"], "0.1.0");
        assert_eq!(p["capabilities"]["experimentalApi"], true);
        assert_eq!(p["capabilities"]["requestAttestation"], false);
        assert_eq!(
            p["capabilities"]["optOutNotificationMethods"][0],
            "item/agentMessage/delta"
        );
    }

    #[test]
    fn thread_start_pins_policy_and_puts_mcp_and_effort_under_config() {
        let p = thread_start_params(Path::new("/repo"), &opts());
        assert_eq!(p["cwd"], "/repo");
        assert_eq!(p["approvalPolicy"], "on-request");
        assert_eq!(p["sandbox"], "workspace-write");
        assert_eq!(p["approvalsReviewer"], "auto_review");
        assert_eq!(p["model"], "gpt-5.5");
        assert_eq!(p["serviceTier"], "fast");
        assert_eq!(p["developerInstructions"], "你是 crew 的一员");
        assert_eq!(p["config"]["model_reasoning_effort"], "high");
        assert_eq!(p["config"]["mcp_servers"]["crew"]["command"], "/opt/crewd");
        assert_eq!(p["config"]["mcp_servers"]["crew"]["args"][2], "codex");
        assert!(p.get("mcpServers").is_none(), "顶层没有 mcpServers");
        assert!(p.get("threadId").is_none());
    }

    #[test]
    fn thread_start_omits_what_was_not_chosen() {
        let p = thread_start_params(Path::new("/repo"), &ThreadOptions::default());
        for key in ["model", "serviceTier", "developerInstructions", "config"] {
            assert!(p.get(key).is_none(), "{key} 不该出现：{p}");
        }
        assert_eq!(
            thread_start_params(
                Path::new("/r"),
                &ThreadOptions {
                    fast: Some(false),
                    ..ThreadOptions::default()
                }
            )["serviceTier"],
            "default"
        );
    }

    #[test]
    fn resume_keeps_the_settings_and_excludes_turns() {
        let p = thread_resume_params("thr-old", Path::new("/repo"), &opts());
        assert_eq!(p["threadId"], "thr-old");
        assert_eq!(p["excludeTurns"], true);
        assert_eq!(p["approvalsReviewer"], "auto_review");
        assert_eq!(p["config"]["mcp_servers"]["crew"]["command"], "/opt/crewd");
    }

    #[test]
    fn turn_start_puts_untrusted_context_beside_a_single_text_input() {
        let mut ctx = UntrustedContext::new();
        ctx.insert(WHITEBOARD_CONTEXT_KEY.into(), "B: 别动 main".into());
        ctx.insert("empty".into(), String::new());
        let p = turn_start_params("thr", "开工", &ctx);
        assert_eq!(p["threadId"], "thr");
        assert_eq!(
            p["input"],
            json!([{ "type": "text", "text": "开工", "text_elements": [] }])
        );
        assert_eq!(
            p["additionalContext"],
            json!({ "crew_whiteboard": { "value": "B: 别动 main", "kind": "untrusted" } })
        );
        let bare = turn_start_params("thr", "开工", &UntrustedContext::new());
        assert!(bare.get("additionalContext").is_none());
    }

    #[test]
    fn interrupt_settings_and_compact_params() {
        assert_eq!(
            turn_interrupt_params("thr", "turn-1"),
            json!({ "threadId": "thr", "turnId": "turn-1" })
        );
        assert_eq!(
            settings_update_params("thr", &Control::Model("o5".into())).unwrap(),
            json!({ "threadId": "thr", "model": "o5" })
        );
        assert_eq!(
            settings_update_params("thr", &Control::Effort("low".into())).unwrap(),
            json!({ "threadId": "thr", "effort": "low" })
        );
        assert_eq!(
            settings_update_params("thr", &Control::Fast(false)).unwrap(),
            json!({ "threadId": "thr", "serviceTier": "default" })
        );
        assert_eq!(settings_update_params("thr", &Control::Compact), None);
        assert_eq!(compact_params("thr"), json!({ "threadId": "thr" }));
    }

    #[test]
    fn thread_id_and_resolved_profile_come_from_the_response() {
        let r = json!({
            "thread": { "id": "thr-9" },
            "model": "gpt-5.5",
            "reasoningEffort": "medium",
            "serviceTier": "priority",
        });
        assert_eq!(thread_id_of(&r).as_deref(), Some("thr-9"));
        assert_eq!(
            profile_of(&r),
            Profile {
                model: Some("gpt-5.5".into()),
                effort: Some("medium".into()),
                fast: Some(true),
            }
        );
        assert_eq!(thread_id_of(&json!({ "thread": { "id": "" } })), None);
        assert_eq!(fast_mode_for_tier("default"), Some(false));
        assert_eq!(
            fast_mode_for_tier("turbo-2027"),
            None,
            "认不出的新档位不冒充标准"
        );
    }

    #[test]
    fn every_real_server_request_method_is_classified() {
        use ServerRequestKind::*;
        // codex-cli 0.160.1 `ServerRequest.json` 的全部 11 个方法。
        for (m, k) in [
            ("item/commandExecution/requestApproval", Approval),
            ("item/fileChange/requestApproval", Approval),
            ("item/permissions/requestApproval", Approval),
            ("applyPatchApproval", Approval),
            ("execCommandApproval", Approval),
            ("mcpServer/elicitation/request", Elicitation),
            ("account/chatgptAuthTokens/refresh", Account),
            ("currentTime/read", CurrentTime),
            ("item/tool/requestUserInput", Unsupported),
            ("item/tool/call", Unsupported),
            ("attestation/generate", Unsupported),
        ] {
            assert_eq!(server_request_kind(m), k, "{m}");
        }
        assert_eq!(server_request_kind("future/brandNew"), Unsupported);
    }

    #[test]
    fn every_server_request_gets_a_reply() {
        let approval =
            answer_server_request("item/commandExecution/requestApproval", &json!({}), 0);
        assert!(matches!(approval.reply, Reply::Error { code: -32000, .. }));
        assert_eq!(approval.notice, None);

        let tool = answer_server_request(
            "mcpServer/elicitation/request",
            &json!({ "serverName": "crew", "message": "Allow post_to_crew?",
                     "_meta": { "codex_approval_kind": "mcp_tool_call" } }),
            0,
        );
        assert_eq!(tool.reply, Reply::Result(json!({ "action": "accept" })));
        assert_eq!(tool.notice, None);

        let form = answer_server_request(
            "mcpServer/elicitation/request",
            &json!({ "serverName": "crew", "mode": "form", "message": "What is your name?",
                     "requestedSchema": { "type": "object" } }),
            0,
        );
        assert_eq!(form.reply, Reply::Result(json!({ "action": "decline" })));
        assert!(
            form.notice
                .as_deref()
                .unwrap()
                .contains("What is your name?")
        );

        let account = answer_server_request(
            "account/chatgptAuthTokens/refresh",
            &json!({ "reason": "unauthorized" }),
            0,
        );
        assert!(matches!(account.reply, Reply::Error { code: -32601, .. }));
        assert!(account.auth_required);
        assert_eq!(account.notice, None);

        let time = answer_server_request(
            "currentTime/read",
            &json!({ "threadId": "t" }),
            1_790_000_000,
        );
        assert_eq!(
            time.reply,
            Reply::Result(json!({ "currentTimeAt": 1_790_000_000i64 }))
        );

        let other = answer_server_request("item/tool/requestUserInput", &json!({}), 0);
        assert!(matches!(other.reply, Reply::Error { code: -32601, .. }));
        assert!(
            other
                .notice
                .as_deref()
                .unwrap()
                .contains("item/tool/requestUserInput")
        );
    }

    #[test]
    fn usage_limit_comes_from_the_failed_turn_protocol_field() {
        let h = health_from(
            "turn/completed",
            &json!({ "turn": { "id": "t", "status": "failed", "error": {
                "message": "Weekly limit reached", "codexErrorInfo": "usageLimitExceeded" } } }),
        )
        .unwrap();
        assert_eq!(h.kind, HealthKind::UsageLimit);
        assert!(h.detail.contains("Weekly limit reached"));
    }

    #[test]
    fn outdated_cli_is_visible_even_with_object_error_info() {
        let m = "The 'gpt-6-astra' model requires a newer version of Codex. Please upgrade...";
        let h = health_from(
            "turn/completed",
            &json!({ "turn": { "status": "failed", "error": { "message": m,
                "codexErrorInfo": { "httpConnectionFailed": { "httpStatusCode": 400 } } } } }),
        )
        .unwrap();
        assert_eq!(h.kind, HealthKind::VersionIncompatible);
        assert!(h.detail.contains(m));
    }

    #[test]
    fn unknown_terminal_failure_is_visible_but_retrying_error_is_not() {
        let failed = health_from(
            "turn/completed",
            &json!({ "turn": { "status": "failed", "error": { "message": "new server error",
                "codexErrorInfo": { "futureVariant": 1 } } } }),
        )
        .unwrap();
        assert_eq!(failed.kind, HealthKind::TurnFailed);
        assert!(failed.detail.contains("new server error"));
        assert_eq!(
            health_from("turn/completed", &json!({ "turn": { "status": "failed" } }))
                .unwrap()
                .kind,
            HealthKind::TurnFailed
        );
        assert_eq!(
            health_from(
                "turn/completed",
                &json!({ "turn": { "status": "completed" } })
            ),
            None
        );
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "transient" }, "willRetry": true })
            ),
            None
        );
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "gone" }, "willRetry": false })
            )
            .unwrap()
            .kind,
            HealthKind::TurnFailed
        );
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "busy", "codexErrorInfo": "serverOverloaded" },
                         "willRetry": true })
            ),
            None,
            "无关的可重试错误不冒充额度"
        );
    }

    #[test]
    fn unauthorized_and_rate_limit_snapshots() {
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "401", "codexErrorInfo": "unauthorized" },
                         "willRetry": false })
            )
            .unwrap()
            .kind,
            HealthKind::AuthRequired
        );
        assert_eq!(
            health_from(
                "account/rateLimits/updated",
                &json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } })
            )
            .unwrap()
            .kind,
            HealthKind::UsageLimit
        );
        assert_eq!(
            health_from(
                "account/rateLimits/updated",
                &json!({ "rateLimits": { "spendControlReached": true } })
            )
            .unwrap()
            .kind,
            HealthKind::UsageLimit
        );
        assert_eq!(
            health_from(
                "account/rateLimits/updated",
                &json!({ "rateLimits": { "rateLimitReachedType": null, "primary": {} } })
            ),
            None
        );
    }

    #[test]
    fn context_usage_reads_last_for_context_and_total_for_cost() {
        let u = context_usage(
            &json!({ "threadId": "a", "turnId": "t", "tokenUsage": {
                "last": { "totalTokens": 128_486 }, "total": { "totalTokens": 2_312_214 },
                "modelContextWindow": 258_400 } }),
            "a",
        )
        .unwrap();
        assert_eq!(
            u,
            ContextUsage {
                turn_id: "t".into(),
                context_tokens: 128_486,
                context_window: 258_400,
                cumulative_tokens: 2_312_214,
            }
        );
        assert_eq!(context_usage(&json!({ "threadId": "other" }), "a"), None);
        let unknown_window = context_usage(
            &json!({ "threadId": "a", "turnId": "t2", "tokenUsage": {
                "last": { "totalTokens": 35_000 }, "total": { "totalTokens": 70_000 },
                "modelContextWindow": null } }),
            "a",
        )
        .unwrap();
        assert_eq!(unknown_window.context_window, 0);
    }

    #[test]
    fn compaction_needs_pressure_and_skips_recent_compaction() {
        let u = ContextUsage {
            turn_id: "t".into(),
            context_tokens: 128_486,
            context_window: 258_400,
            cumulative_tokens: 2_312_214,
        };
        assert!(should_compact(&u, 2_300_000, 3));
        assert!(!should_compact(&u, 50_000, 3));
        assert!(!should_compact(&u, 2_300_000, 1));
        let nearly_full = ContextUsage {
            context_tokens: 190_000,
            ..u.clone()
        };
        assert!(should_compact(&nearly_full, 40_000, 3));
        let no_window = ContextUsage {
            context_window: 0,
            ..nearly_full
        };
        assert!(!should_compact(&no_window, 40_000, 3));
    }
}

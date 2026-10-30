//! Internal closed operations carried by the existing runtime mailbox.
use crewd_proto::codex::*;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub enum CodexOperation {
    Configure(CodexSettingsPatch),
    PreparedQueue {
        id: u64,
        text: String,
        request_id: String,
        inputs: super::input::Prepared,
    },
    PreparedSteer {
        text: String,
        expected_turn_id: String,
        request_id: String,
        inputs: super::input::Prepared,
    },
    Queue {
        id: u64,
        text: String,
        request_id: String,
    },
    Name(String),
    Steer {
        text: String,
        expected_turn_id: String,
        request_id: String,
    },
    Review(CodexReviewTarget),
    Interrupt {
        expected_turn_id: String,
    },
    History(CodexHistoryParams),
    RateLimits,
    Models,
    Tools,
    PermissionProfiles(crewd_proto::codex_inputs::CodexPermissionProfilesParams),
    HistoryMutation {
        action: CodexHistoryAction,
        before_turn_id: Option<String>,
    },
    Fork {
        last_turn_id: Option<String>,
    },
    Inspect,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexOutcome {
    Observed(Value),
    Rejected(String),
    Unknown {
        provider_accepted: bool,
        detail: String,
    },
}
impl CodexOutcome {
    pub fn unsupported() -> Self {
        Self::Rejected("PCW Codex controls require a Codex runtime".into())
    }
}

pub fn approval_native(p: &CodexApprovalPolicy) -> Value {
    match p {
        CodexApprovalPolicy::Simple(s) => json!(match s {
            CodexSimpleApproval::Untrusted => "untrusted",
            CodexSimpleApproval::OnRequest => "on-request",
            CodexSimpleApproval::Never => "never",
        }),
        CodexApprovalPolicy::Granular { granular: g } => {
            json!({"granular":{"mcp_elicitations":g.mcp_elicitations,"request_permissions":g.request_permissions,"rules":g.rules,"sandbox_approval":g.sandbox_approval,"skill_approval":g.skill_approval}})
        }
    }
}
pub fn observed_settings(s: &Value) -> CodexObservedSettings {
    let text = |k: &str| s.get(k).and_then(Value::as_str).map(str::to_string);
    let approval_policy = s.get("approvalPolicy").and_then(|v| match v.as_str() {
        Some("untrusted") => Some(CodexApprovalPolicy::Simple(CodexSimpleApproval::Untrusted)),
        Some("on-request") => Some(CodexApprovalPolicy::Simple(CodexSimpleApproval::OnRequest)),
        Some("never") => Some(CodexApprovalPolicy::Simple(CodexSimpleApproval::Never)),
        _ => v.get("granular").and_then(|g| {
            Some(CodexApprovalPolicy::Granular {
                granular: CodexGranularApproval {
                    mcp_elicitations: g.get("mcp_elicitations")?.as_bool()?,
                    request_permissions: g
                        .get("request_permissions")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    rules: g.get("rules")?.as_bool()?,
                    sandbox_approval: g.get("sandbox_approval")?.as_bool()?,
                    skill_approval: g
                        .get("skill_approval")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
            })
        }),
    });
    CodexObservedSettings {
        model: text("model"),
        model_provider: text("modelProvider"),
        effort: text("effort").or_else(|| text("reasoningEffort")),
        fast: text("serviceTier")
            .as_deref()
            .and_then(super::proto::fast_mode_for_tier),
        service_tier: text("serviceTier"),
        approval_policy,
        approvals_reviewer: match s.get("approvalsReviewer").and_then(Value::as_str) {
            Some("user") => Some(CodexApprovalsReviewer::User),
            Some("auto_review" | "guardian_subagent") => Some(CodexApprovalsReviewer::AutoReview),
            _ => None,
        },
        sandbox: s.get("sandboxPolicy").and_then(|s| {
            Some(match s.get("type")?.as_str()? {
                "readOnly" => CodexSandboxPolicy::ReadOnly {
                    network_access: s
                        .get("networkAccess")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                "workspaceWrite" => CodexSandboxPolicy::WorkspaceWrite {
                    network_access: s
                        .get("networkAccess")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                "dangerFullAccess" => CodexSandboxPolicy::DangerFullAccess,
                _ => return None,
            })
        }),
        collaboration_mode: s
            .pointer("/collaborationMode/mode")
            .and_then(|s| serde_json::from_value(s.clone()).ok()),
        summary: s
            .get("summary")
            .and_then(|s| serde_json::from_value(s.clone()).ok()),
        permission_profile: s
            .pointer("/activePermissionProfile/id")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

pub fn settings_params(
    thread: &str,
    p: &CodexSettingsPatch,
    observed: &Value,
) -> Result<Value, String> {
    p.validate().map_err(str::to_string)?;
    let mut v = json!({"threadId":thread});
    if let Some(m) = &p.model {
        v["model"] = json!(m);
    }
    if let Some(e) = &p.effort {
        v["effort"] = json!(e);
    }
    if let Some(f) = p.fast {
        v["serviceTier"] = json!(if f { "fast" } else { "default" });
    }
    if let Some(a) = &p.approval_policy {
        v["approvalPolicy"] = approval_native(a);
    }
    if let Some(r) = &p.approvals_reviewer {
        v["approvalsReviewer"] = json!(match r {
            CodexApprovalsReviewer::User => "user",
            CodexApprovalsReviewer::AutoReview => "auto_review",
        });
    }
    if let Some(s) = &p.sandbox {
        v["sandboxPolicy"] = serde_json::to_value(s).expect("closed sandbox");
    }
    if let Some(permissions) = &p.permissions {
        v["permissions"] = json!(permissions);
    }
    if let Some(s) = &p.summary {
        v["summary"] = serde_json::to_value(s).expect("closed summary");
    }
    if let Some(mode) = &p.collaboration_mode {
        let model = p
            .model
            .as_deref()
            .or_else(|| observed.get("model").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .ok_or("collaboration mode needs an observed or explicitly selected model")?;
        let effort = p
            .effort
            .as_deref()
            .or_else(|| observed.get("effort").and_then(Value::as_str));
        v["collaborationMode"] = json!({"mode":mode,"settings":{"model":model,"reasoning_effort":effort,"developer_instructions":null}});
    }
    Ok(v)
}

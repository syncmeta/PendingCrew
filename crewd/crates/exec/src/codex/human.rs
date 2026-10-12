//! Native human requests. The runner owns the permission policy; this module only
//! preserves its request and validates the response that a person supplies.
use crewd_term::{DecisionKind, PendingDecision};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerRequest {
    pub fingerprint: String,
    pub method: String,
    pub params: Value,
}

#[derive(Clone)]
pub(crate) struct Form {
    pub decision: PendingDecision,
    pub choices: Vec<Value>,
    pub method: String,
    pub params: Value,
}

impl Form {
    pub fn new(method: &str, params: &Value) -> Option<Self> {
        params.get("threadId")?.as_str()?;
        if method != "mcpServer/elicitation/request" {
            params.get("turnId")?.as_str()?;
        }
        let mut choices = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        let mut add = |label: &str, value: Value| {
            labels.push(label.into());
            choices.push(value);
        };
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                params.get("itemId")?.as_str()?;
                add("允许这一次", json!({"decision":"accept"}));
                add("允许本会话", json!({"decision":"acceptForSession"}));
                add("拒绝", json!({"decision":"decline"}));
                add("取消当前回合", json!({"decision":"cancel"}));
            }
            "item/permissions/requestApproval" => {
                let permissions = params.get("permissions")?.as_object()?;
                add(
                    "仅在本回合授予请求的权限",
                    json!({"permissions":permissions,"scope":"turn"}),
                );
                add(
                    "在本会话授予请求的权限",
                    json!({"permissions":permissions,"scope":"session"}),
                );
                add("不授予权限", json!({"permissions":{},"scope":"turn"}));
            }
            "item/tool/requestUserInput" => {
                let qs = params.get("questions")?.as_array()?;
                if qs.is_empty()
                    || qs.iter().any(|q| {
                        q.get("id").and_then(Value::as_str).is_none()
                            || q.get("question").and_then(Value::as_str).is_none()
                    })
                {
                    return None;
                }
                // Single-choice questions can use the existing decision buttons.
                // Multiple questions and free text use respond_request with the original form.
                if qs.len() == 1
                    && let Some(options) = qs[0].get("options").and_then(Value::as_array)
                {
                    for option in options {
                        let label = option.get("label")?.as_str()?;
                        add(
                            label,
                            json!({"answers":{qs[0]["id"].as_str()?:{"answers":[label]}}}),
                        );
                    }
                }
            }
            "mcpServer/elicitation/request" => {
                // Crew transport elicitation remains handled by the existing MCP contract.
                // Form submission needs a typed form renderer; do not pretend an empty
                // content object is a valid acceptance of an arbitrary schema.
                if params
                    .pointer("/_meta/codex_approval_kind")
                    .and_then(Value::as_str)
                    != Some("mcp_tool_call")
                    && params.get("mode").and_then(Value::as_str) != Some("url")
                {
                    return None;
                }
                if params.get("serverName").and_then(Value::as_str)
                    == Some(super::proto::CREW_MCP_SERVER)
                    && params
                        .pointer("/_meta/codex_approval_kind")
                        .and_then(Value::as_str)
                        == Some("mcp_tool_call")
                {
                    return None;
                }
                add("同意", json!({"action":"accept"}));
                add("拒绝", json!({"action":"decline"}));
                add("取消", json!({"action":"cancel"}));
            }
            _ => return None,
        }
        let prompt = if method == "item/tool/requestUserInput" {
            params["questions"]
                .as_array()?
                .iter()
                .filter_map(|q| q["question"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            let mut parts = vec![method.to_string()];
            for key in [
                "serverName",
                "reason",
                "command",
                "cwd",
                "grantRoot",
                "message",
                "url",
            ] {
                if let Some(s) = params
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    parts.push(format!("{key}: {s}"));
                }
            }
            if method == "item/permissions/requestApproval" {
                parts.push(params["permissions"].to_string());
            }
            parts.join("\n")
        };
        Some(Self {
            decision: PendingDecision {
                kind: DecisionKind::RunnerRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                },
                prompt,
                options: labels,
                numbered: false,
                selected: None,
            },
            choices,
            method: method.into(),
            params: params.clone(),
        })
    }

    pub fn info(&self) -> RunnerRequest {
        RunnerRequest {
            fingerprint: self.decision.fingerprint(),
            method: self.method.clone(),
            params: self.params.clone(),
        }
    }

    pub fn valid_response(&self, value: &Value) -> bool {
        if self.method != "item/tool/requestUserInput" {
            return self.choices.contains(value);
        }
        let Some(answers) = value.get("answers").and_then(Value::as_object) else {
            return false;
        };
        let qs = self.params["questions"]
            .as_array()
            .expect("validated questions");
        if answers.len() != qs.len() || value.as_object().is_none_or(|v| v.len() != 1) {
            return false;
        }
        qs.iter().all(|q| {
            let id = q["id"].as_str().expect("validated id");
            let Some(answer) = answers.get(id).and_then(Value::as_object) else {
                return false;
            };
            let Some(values) = answer.get("answers").and_then(Value::as_array) else {
                return false;
            };
            if answer.len() != 1 || values.is_empty() || values.iter().any(|v| !v.is_string()) {
                return false;
            }
            let Some(options) = q.get("options").and_then(Value::as_array) else {
                return true;
            };
            q.get("isOther").and_then(Value::as_bool) == Some(true)
                || values
                    .iter()
                    .all(|v| options.iter().any(|o| o.get("label") == Some(v)))
        })
    }
}

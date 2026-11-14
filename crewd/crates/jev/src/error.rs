//! 一次调用失败的分类。错误信息里绝不带 key（key 只在 Authorization 头里，reqwest 的错误不含请求头）。
use serde::{Deserialize, Serialize};
use std::fmt;

/// Observation only: no arbitrary text, URL, body, header or credential can enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Unauthorized,
    Rejected,
    RateLimited,
    Overloaded,
    Server,
    Timeout,
    NetworkUnknown,
    NetworkConnect,
    NetworkRead,
    Tls,
    Malformed,
    InvalidNative,
    OuterDeadline,
    ConfigurationChanged,
    Unconfigured,
    InvalidInput,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureDiagnostic {
    pub kind: FailureKind,
    pub status: Option<u16>,
}

impl FailureDiagnostic {
    pub fn new(kind: FailureKind) -> Self {
        Self { kind, status: None }
    }

    /// Only a real typed rustls error permits the more specific TLS label.
    pub fn transport(error: &(dyn std::error::Error + 'static), fallback: FailureKind) -> Self {
        let mut current = Some(error);
        for _ in 0..16 {
            let Some(error) = current else { break };
            if error.downcast_ref::<rustls::Error>().is_some() {
                return Self::new(FailureKind::Tls);
            }
            current = error.source();
        }
        Self::new(fallback)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// 401 / 403：key 缺失或无效。不重试。
    Unauthorized { status: u16 },
    /// 其余 4xx（422 = 我们的请求体不合官方 schema）。不重试；body 截到 500 字符进诊断日志。
    Rejected { status: u16, body: String },
    /// 429，重试用尽。
    RateLimited,
    /// 529，重试用尽。
    Overloaded,
    /// 其余 5xx，重试用尽。
    Server { status: u16 },
    /// 单次超时或总时限用尽。
    Timeout,
    /// 连接 / TLS / 读应答体失败，重试用尽。
    Transport(String),
    /// 2xx 但应答体解析不了。不重试。
    Malformed(String),
}

impl CallError {
    pub fn diagnostic(&self) -> FailureDiagnostic {
        let (kind, status) = match self {
            Self::Unauthorized { status } => (FailureKind::Unauthorized, Some(*status)),
            Self::Rejected { status, .. } => (FailureKind::Rejected, Some(*status)),
            Self::RateLimited => (FailureKind::RateLimited, Some(429)),
            Self::Overloaded => (FailureKind::Overloaded, Some(529)),
            Self::Server { status } => (FailureKind::Server, Some(*status)),
            Self::Timeout => (FailureKind::Timeout, None),
            Self::Transport(_) => (FailureKind::NetworkUnknown, None),
            Self::Malformed(_) => (FailureKind::Malformed, None),
        };
        FailureDiagnostic { kind, status }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            CallError::RateLimited
                | CallError::Overloaded
                | CallError::Server { .. }
                | CallError::Timeout
                | CallError::Transport(_)
        )
    }

    /// 由 HTTP 状态码分类（2xx 不经过这里）。
    pub fn from_status(status: u16, body: &str) -> CallError {
        match status {
            401 | 403 => CallError::Unauthorized { status },
            429 => CallError::RateLimited,
            529 => CallError::Overloaded,
            500..=599 => CallError::Server { status },
            _ => CallError::Rejected {
                status,
                body: body.chars().take(500).collect(),
            },
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Unauthorized { status } => write!(f, "unauthorized ({status})"),
            CallError::Rejected { status, body } => write!(f, "rejected ({status}): {body}"),
            CallError::RateLimited => write!(f, "rate limited (429)"),
            CallError::Overloaded => write!(f, "overloaded (529)"),
            CallError::Server { status } => write!(f, "server error ({status})"),
            CallError::Timeout => write!(f, "timeout"),
            CallError::Transport(m) => write!(f, "transport: {m}"),
            CallError::Malformed(m) => write!(f, "malformed response: {m}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_projection_uses_types_and_never_error_text() {
        let errors = [
            CallError::Transport(
                "private-url https://private.invalid Authorization=sentinel".into(),
            ),
            CallError::Malformed("private parse body".into()),
            CallError::Rejected {
                status: 422,
                body: "private server body".into(),
            },
        ];
        for error in errors {
            let text = serde_json::to_string(&error.diagnostic()).unwrap();
            for forbidden in ["private", "https://", "Authorization", "sentinel"] {
                assert!(!text.contains(forbidden));
            }
        }
        let tls = rustls::Error::General("private TLS detail".into());
        let diagnostic = FailureDiagnostic::transport(&tls, FailureKind::NetworkUnknown);
        assert_eq!(diagnostic.kind, FailureKind::Tls);
        assert!(
            !serde_json::to_string(&diagnostic)
                .unwrap()
                .contains("private")
        );
        let opaque = std::io::Error::other("certificate TLS DNS text is not typed evidence");
        assert_eq!(
            FailureDiagnostic::transport(&opaque, FailureKind::NetworkUnknown).kind,
            FailureKind::NetworkUnknown
        );
    }

    #[test]
    fn statuses_are_classified_like_the_docs_say() {
        assert_eq!(
            CallError::from_status(401, ""),
            CallError::Unauthorized { status: 401 }
        );
        assert_eq!(CallError::from_status(429, ""), CallError::RateLimited);
        assert_eq!(CallError::from_status(529, ""), CallError::Overloaded);
        assert_eq!(
            CallError::from_status(503, ""),
            CallError::Server { status: 503 }
        );
        let long = "x".repeat(2000);
        match CallError::from_status(422, &long) {
            CallError::Rejected { status, body } => {
                assert_eq!(status, 422);
                assert_eq!(body.chars().count(), 500);
            }
            other => panic!("{other:?}"),
        }
        assert!(CallError::RateLimited.is_retryable());
        assert!(CallError::Overloaded.is_retryable());
        assert!(!CallError::Unauthorized { status: 401 }.is_retryable());
        assert!(
            !CallError::Rejected {
                status: 422,
                body: String::new()
            }
            .is_retryable()
        );
        assert!(!CallError::Malformed(String::new()).is_retryable());
    }
}

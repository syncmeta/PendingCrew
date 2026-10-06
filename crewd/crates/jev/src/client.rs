//! `POST {base}/v1/systemone` 的异步客户端：Bearer key、发前脱敏、单次超时 + 总时限、
//! 429 / 529 / 5xx / 连接错误按指数退避重试（尊重 Retry-After，有上限），401 / 422 不重试。
use crate::error::CallError;
use crate::model::{Request, Response};
use crate::scrub::scrub_request;
use reqwest::header::{CONTENT_TYPE, HeaderMap, RETRY_AFTER};
use std::fmt;
use std::time::{Duration, Instant};

/// key 只经这里流转：没有 Display，Debug 打星号；`expose` 只在拼 Authorization 头时调用。
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// 去首尾空白；空串或含空白 / 控制字符 → None（钥匙串里多存了换行之类）。
    pub fn new(raw: &str) -> Option<ApiKey> {
        let t = raw.trim();
        if t.is_empty() || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
            None
        } else {
            Some(ApiKey(t.to_string()))
        }
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// 含第一次。
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(300),
            max_delay: Duration::from_secs(2),
        }
    }
}

impl RetryPolicy {
    /// 第 `failed_attempt` 次（从 1 起）失败后等多久：Retry-After 优先，否则 base × 4^(n−1)（封顶 max_delay）。
    /// 服务端要求的 Retry-After 比 max_delay 还长 → None：不提前硬重试（那只会再吃一次 429），直接放弃、交机长。
    pub fn delay(&self, failed_attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        match retry_after {
            Some(wait) if wait > self.max_delay => None,
            Some(wait) => Some(wait),
            None => Some(
                self.base_delay
                    .saturating_mul(4u32.saturating_pow(failed_attempt.saturating_sub(1)))
                    .min(self.max_delay),
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub base_url: String,
    pub api_key: ApiKey,
    pub retry: RetryPolicy,
    pub connect_timeout: Duration,
    /// 单次请求（连接到读完应答）的上限。
    pub attempt_timeout: Duration,
    /// 含重试与等待的总时限：判断是给调度用的，晚了就没用了，宁可交机长。
    pub deadline: Duration,
}

impl ClientConfig {
    pub fn new(base_url: impl Into<String>, api_key: ApiKey) -> ClientConfig {
        ClientConfig {
            base_url: base_url.into(),
            api_key,
            retry: RetryPolicy::default(),
            connect_timeout: Duration::from_secs(3),
            attempt_timeout: Duration::from_secs(5),
            deadline: Duration::from_secs(12),
        }
    }
}

/// 一次 `evaluate` 的账：试了几次、总耗时、脱敏命中几处。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CallReport {
    pub attempts: u32,
    pub latency: Duration,
    pub scrub_hits: usize,
}

#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    key: ApiKey,
    retry: RetryPolicy,
    attempt_timeout: Duration,
    deadline: Duration,
}

const ENDPOINT_PATH: &str = "/v1/systemone";

fn is_local(host: Option<&str>) -> bool {
    matches!(host, Some("127.0.0.1") | Some("localhost") | Some("[::1]"))
}

/// base URL → 完整端点。只收 https；http 只许本机（测试用的假服务器）。不许带账号、查询串、片段。
pub fn endpoint_for(base_url: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base_url.trim()).map_err(|e| format!("base URL 无效：{e}"))?;
    match url.scheme() {
        "https" => {}
        "http" if is_local(url.host_str()) => {}
        other => {
            return Err(format!(
                "base URL 必须是 https（只有本机地址可以用 http），现在是 {other}"
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("base URL 里不能带账号密码".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("base URL 里不能带查询串或片段".into());
    }
    // 已经填了完整端点（…/v1/systemone）的，不再拼一遍。
    let base = url.as_str().trim_end_matches('/');
    let base = base.strip_suffix(ENDPOINT_PATH).unwrap_or(base);
    // 只填到 /v1 的，也不拼成 /v1/v1/systemone。
    let base = base.strip_suffix("/v1").unwrap_or(base);
    Ok(format!("{base}{ENDPOINT_PATH}"))
}

impl Client {
    pub fn new(config: ClientConfig) -> Result<Client, String> {
        let endpoint = endpoint_for(&config.base_url)?;
        let local = reqwest::Url::parse(&endpoint)
            .map(|u| is_local(u.host_str()))
            .unwrap_or(false);
        let mut builder = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            // 不跟随重定向：Authorization 头不跟着跳到别处。
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("crewd-jev/", env!("CARGO_PKG_VERSION")));
        if local {
            // 本机端点不走代理（系统代理 / 环境变量代理都不该看到本机流量，测试也因此稳定）。
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|e| format!("HTTP 客户端初始化失败：{e}"))?;
        Ok(Client {
            http,
            endpoint,
            key: config.api_key,
            retry: config.retry,
            attempt_timeout: config.attempt_timeout,
            deadline: config.deadline,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 发一次判断。请求先脱敏（唯一的出口，谁调都绕不过），再按策略重试。
    pub async fn evaluate(&self, request: &Request) -> (Result<Response, CallError>, CallReport) {
        let (result, report) = self.evaluate_raw(request).await;
        (result.map(|(response, _)| response), report)
    }

    /// 同 `evaluate`，另外交出应答体原文（jev-eval 录夹具用）。
    pub async fn evaluate_raw(
        &self,
        request: &Request,
    ) -> (Result<(Response, String), CallError>, CallReport) {
        let started = Instant::now();
        let mut request = request.clone();
        let scrub_hits = scrub_request(&mut request);
        let report = |attempts| CallReport {
            attempts,
            latency: started.elapsed(),
            scrub_hits,
        };
        let body = match serde_json::to_vec(&request) {
            Ok(b) => b,
            Err(e) => {
                return (
                    Err(CallError::Malformed(format!("请求序列化失败：{e}"))),
                    report(0),
                );
            }
        };
        let mut attempts = 0;
        let result = loop {
            let remaining = self.deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break Err(CallError::Timeout);
            }
            attempts += 1;
            let (outcome, retry_after) = self
                .attempt(&body, self.attempt_timeout.min(remaining))
                .await;
            match outcome {
                Ok(response) => break Ok(response),
                Err(e) if e.is_retryable() && attempts < self.retry.max_attempts => {
                    // 等不起（Retry-After 超过退避上限，或等完就过了总时限）→ 放弃，交机长。
                    let Some(delay) = self.retry.delay(attempts, retry_after) else {
                        break Err(e);
                    };
                    if started.elapsed() + delay >= self.deadline {
                        break Err(e);
                    }
                    tokio::time::sleep(delay).await;
                }
                Err(e) => break Err(e),
            }
        };
        (result, report(attempts))
    }

    async fn attempt(
        &self,
        body: &[u8],
        timeout: Duration,
    ) -> (Result<(Response, String), CallError>, Option<Duration>) {
        let sent = self
            .http
            .post(&self.endpoint)
            .bearer_auth(self.key.expose())
            .header(CONTENT_TYPE, "application/json")
            .timeout(timeout)
            .body(body.to_vec())
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) => return (Err(transport_error(e)), None),
        };
        let status = response.status().as_u16();
        let wait = retry_after(response.headers());
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => return (Err(transport_error(e)), wait),
        };
        if (200..300).contains(&status) {
            let parsed = serde_json::from_slice::<Response>(&bytes)
                .map(|r| (r, String::from_utf8_lossy(&bytes).into_owned()))
                .map_err(|e| CallError::Malformed(e.to_string()));
            return (parsed, None);
        }
        (
            Err(CallError::from_status(
                status,
                &String::from_utf8_lossy(&bytes),
            )),
            wait,
        )
    }
}

fn transport_error(e: reqwest::Error) -> CallError {
    if e.is_timeout() {
        return CallError::Timeout;
    }
    // reqwest 顶层只说「error sending request」；把 source 链接上（连接被拒、DNS、TLS……）才看得出原因。
    // 不带 URL（without_url）；key 只在请求头里，错误链里没有它。
    let e = e.without_url();
    let mut message = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        message.push_str(": ");
        message.push_str(&s.to_string());
        source = s.source();
    }
    CallError::Transport(message)
}

/// 只认整数秒的 Retry-After（HTTP 日期形式忽略，走退避）。
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_https_and_local_http_only() {
        assert_eq!(
            endpoint_for("https://api.typesafe.ai").unwrap(),
            "https://api.typesafe.ai/v1/systemone"
        );
        assert_eq!(
            endpoint_for(" https://proxy.example.com/jev/ ").unwrap(),
            "https://proxy.example.com/jev/v1/systemone"
        );
        for already_full in [
            "https://api.typesafe.ai/v1/systemone",
            "https://api.typesafe.ai/v1/systemone/",
        ] {
            assert_eq!(
                endpoint_for(already_full).unwrap(),
                "https://api.typesafe.ai/v1/systemone",
                "填了完整端点也不能拼成 /v1/systemone/v1/systemone"
            );
        }
        assert_eq!(
            endpoint_for("https://proxy.example.com/jev/v1/systemone").unwrap(),
            "https://proxy.example.com/jev/v1/systemone"
        );
        for with_version in ["https://api.typesafe.ai/v1", "https://api.typesafe.ai/v1/"] {
            assert_eq!(
                endpoint_for(with_version).unwrap(),
                "https://api.typesafe.ai/v1/systemone",
                "填到 /v1 的不能拼成 /v1/v1/systemone"
            );
        }
        assert_eq!(
            endpoint_for("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080/v1/systemone"
        );
        assert!(endpoint_for("http://localhost:1").is_ok());
        assert!(endpoint_for("http://[::1]:1").is_ok());
        assert!(
            endpoint_for("http://api.typesafe.ai").is_err(),
            "明文发 key"
        );
        assert!(endpoint_for("ftp://api.typesafe.ai").is_err());
        assert!(endpoint_for("https://u:p@api.typesafe.ai").is_err());
        assert!(endpoint_for("https://api.typesafe.ai/?x=1").is_err());
        assert!(endpoint_for("api.typesafe.ai").is_err());
    }

    #[test]
    fn key_is_trimmed_validated_and_never_printed() {
        let k = ApiKey::new("  abc123\n").unwrap();
        assert_eq!(k.expose(), "abc123");
        assert_eq!(format!("{k:?}"), "ApiKey(***)");
        assert!(ApiKey::new("").is_none());
        assert!(ApiKey::new(" \n").is_none());
        assert!(ApiKey::new("ab c").is_none());
    }

    #[test]
    fn backoff_grows_by_four_is_capped_and_a_long_retry_after_gives_up() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay(1, None), Some(Duration::from_millis(300)));
        assert_eq!(p.delay(2, None), Some(Duration::from_millis(1200)));
        assert_eq!(p.delay(3, None), Some(Duration::from_secs(2)));
        assert_eq!(p.delay(40, None), Some(Duration::from_secs(2)), "不溢出");
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(1))),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            p.delay(1, Some(Duration::from_secs(60))),
            None,
            "服务端要我们等的比最长退避还久：不再重试，交机长"
        );
    }

    #[test]
    fn retry_after_reads_integer_seconds_only() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert(
            RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&h), None);
    }
}

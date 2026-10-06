//! 对外的门面：给决策输入，拿 `Verdict`。构造 → 脱敏 → 调用 → 判定 → 记诊断日志，一条龙。
//! 没开 / 没 key：不碰网络，直接 `Defer(Unconfigured)`（spec §7.1：未配置 = 低置信度）。
//! 诊断日志写失败不影响判断，只计数（`diag_failures`）。
use crate::client::{ApiKey, Client, ClientConfig};
use crate::decisions::attribute::{self, AttributeInput, Attribution};
use crate::decisions::route::{self, RouteTarget, RouteTodoInput};
use crate::decisions::urgency::{self, Urgency, UrgencyInput};
use crate::decisions::wake::{self, WakeInput, WakeSet};
use crate::decisions::{Built, DecisionKind, DeferReason, Outcome, Thresholds, Verdict, clip};
use crate::diag::{DiagLog, DiagRecord, EXCERPT_CHARS};
use crate::model::{DEFAULT_BASE_URL, DEFAULT_MODEL, Response};
use crate::scrub::scrub_request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 设置页的 Jev 一节（spec §7.4）。key 不在这里：它在 `KeyStore`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub thresholds: Thresholds,
}

impl Default for JevConfig {
    fn default() -> Self {
        JevConfig {
            enabled: true,
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            thresholds: Thresholds::default(),
        }
    }
}

type Interpret<T> = fn(&Built, &Response, &Thresholds) -> Verdict<T>;

#[derive(Debug)]
pub struct Jev {
    client: Option<Client>,
    model: String,
    thresholds: Thresholds,
    diag: Option<DiagLog>,
    diag_failures: AtomicU64,
}

impl Jev {
    /// 关着或没有 key → 未配置（不报错）；base URL 不合法 → Err（设置页要能提示）。
    pub fn new(
        config: &JevConfig,
        key: Option<ApiKey>,
        diag: Option<DiagLog>,
    ) -> Result<Jev, String> {
        let client = match (config.enabled, key) {
            (true, Some(key)) => Some(Client::new(ClientConfig::new(
                config.base_url.clone(),
                key,
            ))?),
            _ => None,
        };
        Ok(Jev::with_client(client, config, diag))
    }

    /// 测试与评测用：自带调好超时的 `Client`。
    pub fn with_client(client: Option<Client>, config: &JevConfig, diag: Option<DiagLog>) -> Jev {
        Jev {
            client,
            model: config.model.clone(),
            thresholds: config.thresholds,
            diag,
            diag_failures: AtomicU64::new(0),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.client.is_some()
    }

    pub fn diag_failures(&self) -> u64 {
        self.diag_failures.load(Ordering::Relaxed)
    }

    pub async fn route_todo(&self, input: &RouteTodoInput) -> Verdict<RouteTarget> {
        let built = route::build(input, route::seed(input));
        self.decide(DecisionKind::RouteTodo, built, route::interpret)
            .await
    }

    pub async fn wake(&self, input: &WakeInput) -> Verdict<WakeSet> {
        self.decide(DecisionKind::Wake, wake::build(input), wake::interpret)
            .await
    }

    pub async fn attribute(&self, input: &AttributeInput) -> Verdict<Attribution> {
        let built = attribute::build(input, attribute::seed(input));
        self.decide(DecisionKind::Attribute, built, attribute::interpret)
            .await
    }

    pub async fn urgency(&self, input: &UrgencyInput) -> Verdict<Urgency> {
        self.decide(
            DecisionKind::Urgency,
            urgency::build(input),
            urgency::interpret,
        )
        .await
    }

    async fn decide<T: Clone + Serialize>(
        &self,
        kind: DecisionKind,
        built: Result<Built, String>,
        interpret: Interpret<T>,
    ) -> Verdict<T> {
        let built = match built {
            Ok(b) => b,
            Err(e) => {
                let v = Verdict::defer(DeferReason::InvalidInput(e));
                self.log(kind, None, 0, 0, Duration::ZERO, &v);
                return v;
            }
        };
        let Some(client) = &self.client else {
            let v = Verdict::defer(DeferReason::Unconfigured);
            self.log(kind, None, 0, 0, Duration::ZERO, &v);
            return v;
        };
        let mut request = built.request(&self.model);
        // 先在这里脱敏：诊断日志的摘要取自脱敏后的 state。客户端还会再脱一遍（幂等，命中 0）。
        let scrub_hits = scrub_request(&mut request);
        let (result, report) = client.evaluate(&request).await;
        let verdict = match result {
            Ok(response) => interpret(&built, &response, &self.thresholds),
            Err(e) => Verdict::defer(DeferReason::Call(e)),
        };
        let excerpt = Some((&built, &request.state));
        self.log(
            kind,
            excerpt,
            scrub_hits,
            report.attempts,
            report.latency,
            &verdict,
        );
        verdict
    }

    fn log<T: Serialize>(
        &self,
        kind: DecisionKind,
        sent: Option<(&Built, &Value)>,
        scrub_hits: usize,
        attempts: u32,
        latency: Duration,
        verdict: &Verdict<T>,
    ) {
        let Some(diag) = &self.diag else { return };
        let state_text = sent.map(|(_, s)| s.to_string()).unwrap_or_default();
        let (outcome, reason) = match &verdict.outcome {
            Outcome::Act(_) => ("act", None),
            Outcome::Defer(r) => ("defer", Some(r.label())),
        };
        let record = DiagRecord {
            at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            decision: kind.as_str(),
            model: self.model.clone(),
            state_chars: state_text.chars().count(),
            state_excerpt: clip(&state_text, EXCERPT_CHARS),
            options: sent.map(|(b, _)| b.options.clone()).unwrap_or_default(),
            scrub_hits,
            attempts,
            latency_ms: latency.as_millis() as u64,
            outcome,
            reason,
            confidence: verdict.confidence,
            answer: serde_json::to_value(&verdict.best).unwrap_or(Value::Null),
        };
        if diag.append(&record).is_err() {
            self.diag_failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

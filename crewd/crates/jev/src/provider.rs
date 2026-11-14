//! Provider-neutral judgments. Native probabilities and generated suggestions never share authority.
//! One instance selects one provider; there is no fallback chain. Callers supply the
//! authoritative semantic version, then must revalidate it before committing effects.
use crate::client::{ApiKey, Client, ClientConfig};
use crate::error::{FailureDiagnostic, FailureKind};
use crate::model::{Answer, ChoiceCriteria, NoulCriteria, Question, Request};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, OnceCell, watch};

pub const BUDGET: Duration = Duration::from_secs(2);
const CACHE_LIMIT: usize = 256;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Judgment {
    Choice {
        instructions: Value,
        candidates: Vec<(String, Value)>,
    },
    Score {
        instructions: Value,
        levels: Vec<Value>,
    },
    Predicate {
        instructions: Value,
        yes: Value,
        no: Value,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Batch {
    pub state_version: String,
    pub state: Value,
    pub questions: BTreeMap<String, Judgment>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JudgmentAnswer {
    Choice {
        key: String,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Score {
        score: f64,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Predicate {
        probability: f64,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbabilitySource {
    Native,
    GeneratedEstimate,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum JudgmentResult {
    Answered {
        answers: BTreeMap<String, JudgmentAnswer>,
        model: String,
        probability_source: ProbabilitySource,
        suggestion_only: bool,
        raw_usage: Option<Value>,
        attempts: u32,
        latency_ms: u64,
        calibrated: bool,
    },
    Deferred {
        reason: String,
        attempts: Option<u32>,
        latency_ms: u64,
    },
}
impl JudgmentResult {
    fn deferred(reason: &str, attempts: Option<u32>, started: Instant) -> Self {
        Self::Deferred {
            reason: reason.into(),
            attempts,
            latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }
    }
}

/// A sidecar, never an execution grant or a new public judgment DTO.
#[derive(Clone, Debug)]
pub struct ObservedJudgment {
    pub result: JudgmentResult,
    pub diagnostic: Option<FailureDiagnostic>,
    pub attempts: Option<u32>,
    pub latency_ms: u64,
}
impl ObservedJudgment {
    fn new(result: JudgmentResult, diagnostic: Option<FailureDiagnostic>) -> Self {
        let (attempts, latency_ms) = match &result {
            JudgmentResult::Answered {
                attempts,
                latency_ms,
                ..
            } => (Some(*attempts), *latency_ms),
            JudgmentResult::Deferred {
                attempts,
                latency_ms,
                ..
            } => (*attempts, *latency_ms),
        };
        Self {
            result,
            diagnostic,
            attempts,
            latency_ms,
        }
    }
    fn deferred(reason: &str, kind: FailureKind, attempts: Option<u32>, started: Instant) -> Self {
        Self::new(
            JudgmentResult::deferred(reason, attempts, started),
            Some(FailureDiagnostic::new(kind)),
        )
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionConfig {
    pub enabled: bool,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub choice_min: f64,
    pub wake_yes: f64,
}
impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: "typesafe".into(),
            base_url: crate::model::DEFAULT_BASE_URL.into(),
            model: crate::model::DEFAULT_MODEL.into(),
            choice_min: 0.5,
            wake_yes: 0.5,
        }
    }
}
impl DecisionConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.provider != "typesafe" {
            return Err("provider is not implemented".into());
        }
        if self.model.trim().is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_control)
        {
            return Err("invalid model".into());
        }
        if !probability(self.choice_min) || !probability(self.wake_yes) {
            return Err("invalid threshold".into());
        }
        if self.base_url.len() > 2048 {
            return Err("invalid endpoint".into());
        }
        crate::client::endpoint_for(&self.base_url)?;
        Ok(())
    }
}
fn probability(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}
#[derive(Debug, Default)]
struct Cached {
    result: OnceCell<ObservedJudgment>,
    ready: Notify,
}
#[derive(Debug)]
pub struct DecisionProvider {
    config: DecisionConfig,
    client: Option<Arc<Client>>,
    cache: Mutex<BTreeMap<String, Arc<Cached>>>,
    revoked: watch::Sender<bool>,
}
impl DecisionProvider {
    pub fn new(config: DecisionConfig, key: Option<ApiKey>) -> Result<Self, String> {
        config.validate()?;
        let client = if config.enabled {
            key.map(|key| {
                let mut c = ClientConfig::new(&config.base_url, key);
                c.deadline = BUDGET;
                c.attempt_timeout = BUDGET;
                c.connect_timeout = BUDGET;
                Client::new(c)
            })
            .transpose()?
        } else {
            None
        };
        Ok(Self {
            config,
            client: client.map(Arc::new),
            cache: Mutex::new(BTreeMap::new()),
            revoked: watch::channel(false).0,
        })
    }
    /// Irreversible per-instance invalidation: no request can outlive its configuration.
    pub fn revoke(&self) {
        self.revoked.send_replace(true);
    }
    pub fn configured(&self) -> bool {
        self.client.is_some()
    }
    pub fn config(&self) -> &DecisionConfig {
        &self.config
    }
    /// Config/model/threshold changes construct a new instance: no cache crosses that boundary.
    pub async fn evaluate(&self, batch: &Batch) -> JudgmentResult {
        self.evaluate_until(batch, Instant::now() + BUDGET).await
    }
    /// The owner, not just its waiter, shares the caller's absolute deadline.
    /// A later cache waiter can never extend the first dispatch's deadline.
    pub async fn evaluate_until(&self, batch: &Batch, deadline: Instant) -> JudgmentResult {
        self.evaluate_observed_until(batch, deadline).await.result
    }
    /// Uses the very same owner, cache cell and caller deadline as old waiters.
    pub async fn evaluate_observed_until(
        &self,
        batch: &Batch,
        deadline: Instant,
    ) -> ObservedJudgment {
        let started = Instant::now();
        let deadline = deadline.min(started + BUDGET);
        if deadline <= started {
            return ObservedJudgment::deferred(
                "deadline",
                FailureKind::OuterDeadline,
                Some(0),
                started,
            );
        }
        if *self.revoked.borrow() {
            return ObservedJudgment::deferred(
                "configuration_changed",
                FailureKind::ConfigurationChanged,
                Some(0),
                started,
            );
        }
        let Some(client) = &self.client else {
            return ObservedJudgment::deferred(
                "unconfigured",
                FailureKind::Unconfigured,
                Some(0),
                started,
            );
        };
        let request = match self.request(batch) {
            Ok(r) => r,
            Err(()) => {
                return ObservedJudgment::deferred(
                    "invalid_input",
                    FailureKind::InvalidInput,
                    Some(0),
                    started,
                );
            }
        };
        let key = serde_json::to_string(batch).expect("typed JSON values serialize");
        let cell = {
            let mut cache = self.cache.lock().await;
            if let Some(c) = cache.get(&key) {
                c.clone()
            } else {
                if cache.len() >= CACHE_LIMIT {
                    let victim = cache
                        .iter()
                        .find(|(_, c)| c.result.get().is_some())
                        .map(|(k, _)| k.clone());
                    if let Some(v) = victim {
                        cache.remove(&v);
                    } else {
                        return ObservedJudgment::deferred(
                            "busy",
                            FailureKind::Busy,
                            Some(0),
                            started,
                        );
                    }
                }
                let c = Arc::new(Cached::default());
                cache.insert(key, c.clone());
                let output = c.clone();
                let client = client.clone();
                let batch = batch.clone();
                let mut revoked = self.revoked.subscribe();
                // A canceled caller cannot erase a dispatched judgment and start it again.
                // This sole owner remains bounded by the same total business deadline.
                tokio::spawn(async move {
                    let result = if *revoked.borrow() {
                        ObservedJudgment::deferred(
                            "configuration_changed",
                            FailureKind::ConfigurationChanged,
                            Some(0),
                            started,
                        )
                    } else {
                        tokio::select! { biased;
                            _ = revoked.changed() => ObservedJudgment::deferred("configuration_changed", FailureKind::ConfigurationChanged, None, started),
                            result = Self::call(&client, &request, &batch, started, deadline) => result,
                        }
                    };
                    let _ = output.result.set(result);
                    output.ready.notify_waiters();
                });
                c
            }
        };
        loop {
            let ready = cell.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(result) = cell.result.get() {
                return result.clone();
            }
            ready.await;
        }
    }
    async fn call(
        client: &Client,
        request: &Request,
        batch: &Batch,
        started: Instant,
        deadline: Instant,
    ) -> ObservedJudgment {
        match tokio::time::timeout_at(deadline.into(), client.evaluate_observed_raw(request)).await
        {
            Err(_) => {
                ObservedJudgment::deferred("deadline", FailureKind::OuterDeadline, None, started)
            }
            Ok(observed) => {
                let report = observed.report;
                let (response, raw) = match observed.result {
                    Ok(value) => value,
                    Err(error) => {
                        return ObservedJudgment::new(
                            JudgmentResult::deferred(
                                "provider_error",
                                Some(report.attempts),
                                started,
                            ),
                            Some(observed.diagnostic.unwrap_or_else(|| error.diagnostic())),
                        );
                    }
                };
                let answers = validate_answers(batch, &response.answers);
                match answers {
                    None => ObservedJudgment::new(
                        JudgmentResult::deferred(
                            "invalid_response",
                            Some(report.attempts),
                            started,
                        ),
                        Some(FailureDiagnostic {
                            kind: FailureKind::InvalidNative,
                            status: observed.http_status,
                        }),
                    ),
                    Some(answers) => ObservedJudgment::new(
                        JudgmentResult::Answered {
                            answers,
                            model: response.model,
                            probability_source: ProbabilitySource::Native,
                            suggestion_only: false,
                            raw_usage: serde_json::from_str::<Value>(&raw)
                                .ok()
                                .and_then(|v| v.get("usage").cloned()),
                            attempts: report.attempts,
                            latency_ms: report.latency.as_millis().min(u64::MAX as u128) as u64,
                            calibrated: false,
                        },
                        None,
                    ),
                }
            }
        }
    }
    fn request(&self, batch: &Batch) -> Result<Request, ()> {
        if serde_json::to_vec(batch).map_or(true, |v| v.len() > 65536)
            || batch.state_version.is_empty()
            || batch.questions.is_empty()
            || batch.questions.len() > 32
        {
            return Err(());
        }
        let mut questions = BTreeMap::new();
        for (id, q) in &batch.questions {
            if id.is_empty()
                || id.len() > 32
                || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                return Err(());
            }
            let wire = match q {
                Judgment::Choice {
                    instructions,
                    candidates,
                } => {
                    let keys: BTreeSet<_> = candidates.iter().map(|(k, _)| k).collect();
                    if candidates.is_empty()
                        || candidates.len() > 32
                        || keys.len() != candidates.len()
                        || keys.iter().any(|k| k.is_empty() || k.len() > 32)
                    {
                        return Err(());
                    }
                    Question::Choice {
                        instructions: instructions.clone(),
                        criteria: ChoiceCriteria(candidates.clone()),
                    }
                }
                Judgment::Score {
                    instructions,
                    levels,
                } => {
                    if levels.is_empty() || levels.len() > 32 {
                        return Err(());
                    }
                    Question::Score {
                        instructions: instructions.clone(),
                        criteria: levels.clone(),
                    }
                }
                Judgment::Predicate {
                    instructions,
                    yes,
                    no,
                } => Question::Noul {
                    instructions: instructions.clone(),
                    criteria: Some(NoulCriteria {
                        yes: yes.clone(),
                        no: no.clone(),
                    }),
                },
            };
            questions.insert(id.clone(), wire);
        }
        let request = Request {
            model: self.config.model.clone(),
            state: batch.state.clone(),
            questions,
        };
        if serde_json::to_vec(&request).map_or(true, |b| b.len() > 65536) {
            return Err(());
        }
        Ok(request)
    }
}
fn validate_answers(
    batch: &Batch,
    answers: &BTreeMap<String, Answer>,
) -> Option<BTreeMap<String, JudgmentAnswer>> {
    if batch.questions.len() != answers.len() {
        return None;
    }
    batch
        .questions
        .iter()
        .map(|(id, q)| {
            let a = answers.get(id)?;
            let value = match (q, a) {
                (
                    Judgment::Choice { candidates, .. },
                    Answer::Choice {
                        choice,
                        confidence,
                        probabilities,
                    },
                ) => {
                    let keys: BTreeSet<_> = candidates.iter().map(|(k, _)| k).collect();
                    if !keys.contains(choice)
                        || !probability(*confidence)
                        || probabilities.len() != keys.len()
                        || probabilities
                            .iter()
                            .any(|(k, p)| !keys.contains(k) || !probability(*p))
                    {
                        return None;
                    }
                    JudgmentAnswer::Choice {
                        key: choice.clone(),
                        confidence: *confidence,
                        probabilities: probabilities.clone(),
                    }
                }
                (
                    Judgment::Score { levels, .. },
                    Answer::Score {
                        score,
                        confidence,
                        probabilities,
                        ..
                    },
                ) => {
                    if !score.is_finite()
                        || *score < 0.0
                        || *score > (levels.len() - 1) as f64
                        || !probability(*confidence)
                        || probabilities.len() != levels.len()
                        || probabilities.iter().any(|(key, p)| {
                            !probability(*p)
                                || key
                                    .parse::<usize>()
                                    .ok()
                                    .filter(|i| *i < levels.len() && key == &i.to_string())
                                    .is_none()
                        })
                    {
                        return None;
                    }
                    JudgmentAnswer::Score {
                        score: *score,
                        confidence: *confidence,
                        probabilities: probabilities.clone(),
                    }
                }
                (Judgment::Predicate { .. }, Answer::Noul { noul }) if probability(*noul) => {
                    JudgmentAnswer::Predicate { probability: *noul }
                }
                _ => return None,
            };
            Some((id.clone(), value))
        })
        .collect()
}

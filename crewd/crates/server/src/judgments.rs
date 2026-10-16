//! Decision planning is separate from effects. All public/model names are opaque
//! request-local aliases; only a current authority snapshot can resolve them.
mod configuration;
pub(crate) use configuration::handle;
use crewd_collab::judgments::InputSnapshot;
use crewd_jev::provider::{
    Batch, DecisionConfig, Judgment, JudgmentAnswer, JudgmentResult, ProbabilitySource,
};
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) fn batch(s: &InputSnapshot, wake: bool) -> Batch {
    let mut questions = BTreeMap::new();
    if s.kind == "route" && !s.recipients.is_empty() {
        questions.insert("route".into(),Judgment::Choice {instructions:json!("Choose the member to receive this human requirement. This does not assign a Work owner or authorize tools."),
            candidates:s.recipients.iter().take(crewd_jev::decisions::MAX_MEMBERS).enumerate().map(|(i,m)|(format!("s{i}"),json!({"name":m.name,"captain":m.captain}))).collect()});
    }
    if s.kind == "message" {
        if wake {
            for (i, m) in s.recipients.iter().enumerate() {
                questions.insert(format!("wake_s{i}"),Judgment::Predicate{instructions:json!({"question":"Should this member wake now for the human message?","member":{"name":m.name,"captain":m.captain}}),yes:json!("needs to act now"),no:json!("does not need to act now")});
            }
        }
        if !s.links.is_empty() {
            let mut candidates: Vec<_> = s
                .links
                .iter()
                .enumerate()
                .map(|(i, c)| (format!("o{i}"), json!({"kind":c.kind,"summary":c.summary})))
                .collect();
            candidates.push(("none".into(), json!("no relation")));
            questions.insert("attribute".into(),Judgment::Choice{instructions:json!("Choose the existing object this message concerns, or none. This only records a reference; it never answers a question, changes visibility or assigns work."),candidates});
        }
    }
    Batch {
        state_version: s.version.clone(),
        state: json!({"text":s.text.chars().take(crewd_jev::decisions::MAX_TEXT_CHARS).collect::<String>()}),
        questions,
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Selection {
    pub recipients: Vec<String>,
    pub link: Option<usize>,
    pub adopted: bool,
}
/// Generated estimates can only be suggestions; they never enter the native
/// threshold adoption path, even if a future adapter supplies identical JSON.
pub(crate) fn selection(
    s: &InputSnapshot,
    cfg: &DecisionConfig,
    result: &JudgmentResult,
) -> Selection {
    let fallback = || Selection {
        recipients: s
            .captain
            .as_ref()
            .filter(|c| s.recipients.iter().any(|m| &m.id == *c))
            .cloned()
            .into_iter()
            .collect(),
        link: None,
        adopted: false,
    };
    let JudgmentResult::Answered {
        answers,
        probability_source: ProbabilitySource::Native,
        suggestion_only: false,
        ..
    } = result
    else {
        return fallback();
    };
    if s.kind == "route" {
        let member = match answers.get("route") {
            Some(JudgmentAnswer::Choice {
                key, confidence, ..
            }) if *confidence >= cfg.choice_min => alias_index(
                key,
                's',
                s.recipients.len().min(crewd_jev::decisions::MAX_MEMBERS),
            )
            .and_then(|i| s.recipients.get(i)),
            _ => None,
        };
        return member
            .map(|m| Selection {
                recipients: vec![m.id.clone()],
                link: None,
                adopted: true,
            })
            .unwrap_or_else(fallback);
    }

    let mut out = Selection {
        recipients: vec![],
        link: None,
        adopted: true,
    };
    for (i, m) in s.recipients.iter().enumerate() {
        match answers.get(&format!("wake_s{i}")) {
            Some(JudgmentAnswer::Predicate { probability }) if *probability >= cfg.wake_yes => {
                out.recipients.push(m.id.clone())
            }
            _ => {}
        }
    }
    if out.recipients.is_empty() {
        out.adopted = false;
    }
    out.link = match answers.get("attribute") {
        Some(JudgmentAnswer::Choice {
            key, confidence, ..
        }) if *confidence >= cfg.choice_min => alias_index(key, 'o', s.links.len()),
        _ => None,
    };
    out
}

use crate::AppState;
use crewd_collab::{Change, judgments as facts};
use crewd_jev::provider::DecisionProvider;
use crewd_proto::judgments::*;
use crewd_wake::{Ms, plan};
fn alias_index(key: &str, prefix: char, len: usize) -> Option<usize> {
    key.strip_prefix(prefix)
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|i| *i < len && key == format!("{prefix}{i}"))
}

use std::path::PathBuf;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, Semaphore, broadcast};
use tokio::task::{JoinHandle, JoinSet};

pub(crate) struct Runtime {
    state: Weak<AppState>,
    data: PathBuf,
    closed: Arc<AtomicBool>,
    slots: Arc<Semaphore>,
    provider: Mutex<Option<(String, Arc<DecisionProvider>)>>,
    tasks: Mutex<JoinSet<()>>,
    pump: Mutex<Option<JoinHandle<()>>>,
}
fn provider_config(c: &JudgmentConfig) -> DecisionConfig {
    DecisionConfig {
        enabled: c.enabled,
        provider: c.provider.clone(),
        base_url: c.base_url.clone(),
        model: c.model.clone(),
        choice_min: c.choice_min,
        wake_yes: c.wake_yes,
    }
}
impl Runtime {
    pub(crate) async fn start(
        state: &Arc<AppState>,
        data: PathBuf,
        active: bool,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        let active = active
            && state
                .db
                .call_result(|s| crewd_store::judgment_state::format_available(s.conn()))
                .await??;
        let runtime_id = state.runtime_binding_id.clone();
        state
            .collab(move |tx| {
                tx.conn()
                    .execute("DELETE FROM meta WHERE key=?1", [facts::ACTIVE_KEY])?;
                if active {
                    crewd_store::meta_set(tx.conn(), facts::ACTIVE_KEY, &runtime_id)?;
                }
                Ok(())
            })
            .await?;
        if !active {
            return Ok(None);
        }
        let this = Arc::new(Self {
            state: Arc::downgrade(state),
            data,
            closed: Arc::new(AtomicBool::new(false)),
            slots: Arc::new(Semaphore::new(32)),
            provider: Mutex::new(None),
            tasks: Mutex::new(JoinSet::new()),
            pump: Mutex::new(None),
        });
        // Settle old pending attempts without invoking a provider. A restored DB
        // has a different dbId and is historical instead of a new dispatch.
        let _ = state.judgment_runtime.set(this.clone());
        this.scan_pending(true).await?;
        let mut rx = state.hub.subscribe_collab();
        let weak = Arc::downgrade(&this);
        *this.pump.lock().await = Some(tokio::spawn(async move {
            loop {
                let event = rx.recv().await;
                let Some(runtime) = weak.upgrade() else { break };
                if runtime.closed.load(Ordering::Acquire) {
                    break;
                }
                match event {
                    Ok(Change::MessageAppended { message_id, .. }) => {
                        runtime
                            .schedule(JudgmentSourceKind::Message, message_id)
                            .await
                    }
                    Ok(Change::WorkChanged { id, .. }) if id.starts_with('#') => {
                        runtime.schedule(JudgmentSourceKind::Requirement, id).await
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Err(e) = runtime.scan_pending(false).await {
                            eprintln!("crewd: judgment pending scan failed: {e}");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }));
        Ok(Some(this))
    }
    async fn scan_pending(self: &Arc<Self>, recovery: bool) -> anyhow::Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        // Keyset pages bound both the DB actor turn and retained IDs. Historical
        // and settled rows advance the cursor too; they cannot trap a scan.
        for kind in [JudgmentSourceKind::Message, JudgmentSourceKind::Requirement] {
            let mut after = String::new();
            loop {
                if self.closed.load(Ordering::Acquire) {
                    return Ok(());
                }
                let cursor = after.clone();
                let page_kind = kind.clone();
                let (rows, pending) = state.db.call_result(move |s| {
                    let sql = match page_kind {
                        JudgmentSourceKind::Message => "SELECT id FROM messages WHERE id>?1 AND extra LIKE '%judgmentV1%' ORDER BY id LIMIT 64",
                        JudgmentSourceKind::Requirement => "SELECT '#'||r.number AS id FROM object_refs r JOIN todos t ON t.id=r.raw_todo_id WHERE r.kind='requirement' AND ('#'||r.number)>?1 AND t.extra LIKE '%judgmentV1%' ORDER BY id LIMIT 64",
                    };
                    let mut q = s.conn().prepare(sql)?;
                    let rows = q.query_map([cursor], |r| r.get::<_,String>(0))?
                        .collect::<std::result::Result<Vec<_>,_>>()?;
                    let mut pending = Vec::new();
                    for id in &rows {
                        if facts::record(s.conn(), &page_kind, id)?.is_some_and(|r|
                            matches!(r.phase, JudgmentPhase::Pending | JudgmentPhase::Dispatched)) {
                            pending.push(id.clone());
                        }
                    }
                    Ok::<_,crewd_collab::CollabError>((rows, pending))
                }).await??;
                let Some(last) = rows.last() else { break };
                after = last.clone();
                for id in pending {
                    if recovery {
                        self.run(kind.clone(), id, true).await;
                    } else {
                        self.schedule(kind.clone(), id).await;
                    }
                }
                if rows.len() < 64 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        Ok(())
    }
    async fn schedule(self: &Arc<Self>, kind: JudgmentSourceKind, id: String) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            self.run(kind, id, true).await;
            return;
        };
        let runtime = self.clone();
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let _slot = slot;
            runtime.run(kind, id, false).await;
        });
    }
    async fn current_provider(&self, settings: &JudgmentSettings) -> Option<Arc<DecisionProvider>> {
        let revision = settings.revision.as_ref()?;
        let config = settings.config.as_ref()?;
        if !config.enabled {
            return None;
        }
        let state = self.state.upgrade()?;
        configuration::validate_config(&state, config).ok()?;
        let epoch = state
            .db
            .call_result(|s| crewd_store::judgment_state::epoch(s.conn()))
            .await
            .ok()?
            .ok()?;
        let cache_key = format!("{revision}:{epoch}");
        let mut slot = self.provider.lock().await;
        if let Some((old, p)) = &*slot
            && old == &cache_key
        {
            return Some(p.clone());
        }
        if let Some((_, old)) = slot.take() {
            old.revoke();
        }
        let key = configuration::load_key(&state, self.data.clone(), settings).await;
        let provider = Arc::new(DecisionProvider::new(provider_config(config), key).ok()?);
        *slot = Some((cache_key, provider.clone()));
        Some(provider)
    }
    async fn run(&self, kind: JudgmentSourceKind, id: String, recovery: bool) {
        if let Err(e) = self.run_inner(kind, id, recovery).await {
            eprintln!("crewd: judgment failed closed: {e}");
        }
    }
    async fn run_inner(
        &self,
        kind: JudgmentSourceKind,
        id: String,
        recovery: bool,
    ) -> anyhow::Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        let key = kind.clone();
        let source = id.clone();
        let Some((record, snapshot, settings)) = state
            .collab(move |tx| {
                // Restart never re-dispatches a previously admitted external call.
                if recovery
                    && let Some(mut old) = facts::record(tx.conn(), &key, &source)?
                    && old.phase == JudgmentPhase::Dispatched
                {
                    old.phase = JudgmentPhase::Pending;
                    facts::restore_pending_for_settlement(tx, &key, &source, &old)?;
                }
                facts::dispatch(tx, &key, &source)
            })
            .await?
        else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let can_wake = if kind == JudgmentSourceKind::Message {
            let mid = id.clone();
            state
                .db
                .call_result(move |s| {
                    let m = crewd_collab::messages::get(s.conn(), &mid)?;
                    let now = Ms(crewd_store::now_ms());
                    let f = crate::wake_adapter::message_facts(s.conn(), &m, now)?;
                    Ok::<_, crewd_collab::CollabError>(
                        crate::wake_adapter::channel_context(s.conn(), &m.channel_id, now)?
                            .is_some_and(|ctx| plan::needs_arbiter(&f, &ctx, now)),
                    )
                })
                .await??
        } else {
            false
        };
        let request = batch(&snapshot, can_wake);
        let config = settings
            .config
            .as_ref()
            .map(provider_config)
            .unwrap_or_default();
        let elapsed = crewd_store::now_ms().saturating_sub(record.prepared_at_ms);
        let remaining =
            std::time::Duration::from_millis(2000u64.saturating_sub(elapsed.max(0) as u64));
        let result = if recovery
            || remaining.is_zero()
            || self.closed.load(Ordering::Acquire)
            || !state.admission.any()
        {
            JudgmentResult::Deferred {
                reason: if recovery {
                    "restart_unknown"
                } else if !state.admission.any() {
                    "awaiting_manual_resume"
                } else {
                    "deadline"
                }
                .into(),
                attempts: if recovery { None } else { Some(0) },
                latency_ms: 0,
            }
        } else {
            let deadline = std::time::Instant::now() + remaining;
            let eval = async {
                match self.current_provider(&settings).await {
                    Some(p) => p.evaluate_until(&request, deadline).await,
                    None => JudgmentResult::Deferred {
                        reason: "unconfigured".into(),
                        attempts: Some(0),
                        latency_ms: 0,
                    },
                }
            };
            tokio::time::timeout(remaining, eval)
                .await
                .unwrap_or(JudgmentResult::Deferred {
                    reason: "deadline".into(),
                    attempts: None,
                    latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                })
        };
        let chosen = selection(&snapshot, &config, &result);
        let key = kind.clone();
        let source = id.clone();
        let closed = self.closed.clone();
        state
            .collab(move |tx| {
                if closed.load(Ordering::Acquire) {
                    return Ok(());
                }
                let targets = if key == JudgmentSourceKind::Requirement {
                    chosen
                        .recipients
                        .iter()
                        .cloned()
                        .map(|id| (id, crewd_collab::wakes::WakePriority::Human))
                        .collect::<Vec<_>>()
                } else {
                    let m = crewd_collab::messages::get(tx.conn(), &source)?;
                    let now = Ms(tx.now_ms());
                    let facts = crate::wake_adapter::message_facts(tx.conn(), &m, now)?;
                    let Some(ctx) =
                        crate::wake_adapter::channel_context(tx.conn(), &m.channel_id, now)?
                    else {
                        return Ok(());
                    };
                    plan::plan(
                        &facts,
                        &ctx,
                        chosen.adopted.then_some(chosen.recipients.as_slice()),
                        now,
                    )
                    .into_iter()
                    .map(|t| {
                        (
                            t.session,
                            match t.priority {
                                crewd_wake::Priority::Human => {
                                    crewd_collab::wakes::WakePriority::Human
                                }
                                crewd_wake::Priority::System => {
                                    crewd_collab::wakes::WakePriority::System
                                }
                                crewd_wake::Priority::Auto => {
                                    crewd_collab::wakes::WakePriority::Auto
                                }
                            },
                        )
                    })
                    .collect()
                };
                let link = chosen
                    .link
                    .and_then(|n| snapshot.links.get(n))
                    .and_then(|l| {
                        Some(JudgmentLink {
                            kind: JudgmentObjectKind::from_key(&l.kind)?,
                            id: l.id.clone(),
                            revision: l.revision.clone(),
                        })
                    });
                let diagnostic =
                    serde_json::to_value(&result).map_err(crewd_store::StoreError::from)?;
                if !facts::settle(
                    tx,
                    &key,
                    &source,
                    &record.decision_id,
                    targets.iter().map(|(id, _)| id.clone()).collect(),
                    link,
                    diagnostic,
                )? {
                    return Ok(());
                }
                for (target, priority) in targets {
                    crewd_collab::wakes::enqueue(
                        tx,
                        crewd_collab::wakes::WakeRequest {
                            session: target,
                            kind: if key == JudgmentSourceKind::Requirement {
                                crewd_collab::wakes::WakeKind::System
                            } else {
                                crewd_collab::wakes::WakeKind::Message
                            },
                            source: if key == JudgmentSourceKind::Requirement {
                                format!("judgment-route:{source}")
                            } else {
                                crewd_collab::wakes::source::message(&source)
                            },
                            priority,
                            not_before_ms: None,
                            note: None,
                            origin: None,
                        },
                    )?;
                }
                Ok(())
            })
            .await?;
        Ok(())
    }
    async fn invalidate(&self) {
        if let Some((_, p)) = self.provider.lock().await.take() {
            p.revoke();
        }
    }
    pub(crate) async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some((_, p)) = self.provider.lock().await.take() {
            p.revoke();
        }
        if let Some(pump) = self.pump.lock().await.take() {
            pump.abort();
            let _ = pump.await;
        }
        let mut tasks = self.tasks.lock().await;
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
}

/// Public inspection is a closed projection: raw usage/error bodies and captured
/// authority floors remain private, never serialized into this view.
fn public_view(r: &JudgmentRecord) -> JudgmentView {
    let d = &r.diagnostic;
    let status = d
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let reason = match status {
        "answered" => JudgmentReason::Answered,
        "prepared" => JudgmentReason::Prepared,
        "stale" => JudgmentReason::Stale,
        "deferred" => match d.get("reason").and_then(serde_json::Value::as_str) {
            Some("unconfigured") => JudgmentReason::Unconfigured,
            Some("deadline") => JudgmentReason::Deadline,
            Some("provider_error") => JudgmentReason::ProviderError,
            Some("invalid_response" | "invalid_input") => JudgmentReason::InvalidResponse,
            Some("restart_unknown") => JudgmentReason::RestartUnknown,
            Some("busy") => JudgmentReason::Busy,
            Some("configuration_changed") => JudgmentReason::ConfigurationChanged,
            _ => JudgmentReason::Unknown,
        },
        _ => JudgmentReason::Unknown,
    };
    JudgmentView {
        decision_id: r.decision_id.clone(),
        phase: r.phase.clone(),
        reason,
        backend: JudgmentBackend::Typesafe,
        model: d
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|s| s.len() <= 256 && !s.chars().any(char::is_control))
            .map(str::to_owned),
        probability_source: match d
            .get("probability_source")
            .and_then(serde_json::Value::as_str)
        {
            Some("native") => Some(JudgmentProbabilitySource::Native),
            Some("generated_estimate") => Some(JudgmentProbabilitySource::GeneratedEstimate),
            _ => None,
        },
        calibrated: false,
        attempts: d
            .get("attempts")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| n.try_into().ok()),
        latency_ms: d.get("latency_ms").and_then(serde_json::Value::as_u64),
        usage: d.get("raw_usage").map(|u| JudgmentUsageView {
            input_tokens: u.get("input_tokens").and_then(serde_json::Value::as_u64),
            output_tokens: u.get("output_tokens").and_then(serde_json::Value::as_u64),
        }),
        recipients: r.recipients.clone(),
        attribution: r.attribution.clone(),
    }
}

#[cfg(test)]
mod tests;

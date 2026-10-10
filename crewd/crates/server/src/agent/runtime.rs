//! serve 里 agent 面的配置（serve 启动段由 M3c 填；缺省值见 Default）。整文件替换 M3a 的占位。

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// 一次注入的字符上限（官方 additionalContext 上限 10,000，留余量）。
    pub inject_cap_chars: usize,
    /// Post gate (S61): a session's bucket holds this many posts...
    pub post_burst: u32,
    /// ...and gets one back every this many seconds.
    pub post_refill_secs: u64,
    /// The same text (trimmed, whitespace collapsed) from the same session into the same channel
    /// within this window is refused as a duplicate.
    pub dup_window_secs: u64,
    /// At most this many posts with @human per session in any 30 minutes.
    pub human_mentions_per_30min: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            inject_cap_chars: 9_000,
            post_burst: 6,
            post_refill_secs: 20,
            dup_window_secs: 600,
            human_mentions_per_30min: 3,
        }
    }
}

/// Milliseconds since the epoch for the post gate: the real clock (`port::now_ms`) by default;
/// tests swap in one they move.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> i64 + Send + Sync>);

impl Clock {
    pub fn new(f: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
    pub fn now(&self) -> i64 {
        (self.0)()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new(crate::agent::port::now_ms)
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock")
    }
}

/// Why the post gate refused a post (the tool turns it into an `r.*` receipt).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRefusal {
    /// The bucket is empty: one post comes back in `retry_secs`.
    Throttled { retry_secs: u64 },
    /// The same text already went into the same channel as `#seq`.
    Duplicate { seq: i64 },
    /// Too many @human in the last 30 minutes.
    HumanMentions,
    /// Item `index` (1-based) of the batch repeats an earlier item's text.
    RepeatedInBatch { index: usize },
}

/// One `post` call as the gate sees it: all of its items are admitted or none.
pub struct PostAttempt<'a> {
    pub session: &'a str,
    /// The channel it goes to, when it already exists (a dm opened by this very post has none).
    pub channel: Option<&'a str>,
    pub texts: &'a [String],
    pub human_mention: bool,
}

const HUMAN_WINDOW_MS: i64 = 30 * 60 * 1000;

#[derive(Debug)]
struct Recent {
    channel: String,
    text: String,
    seq: i64,
    at: i64,
}

#[derive(Debug, Default)]
struct SenderGate {
    tokens: f64,
    /// When `tokens` was last brought up to date; `None` = never posted (a full bucket).
    refilled_at: Option<i64>,
    recent: VecDeque<Recent>,
    human_at: VecDeque<i64>,
}

/// Trimmed, whitespace runs collapsed to one space: what "the same text" means.
fn normalize(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Default)]
pub struct AgentRuntime {
    pub config: AgentConfig,
    clock: Clock,
    /// One async mutex per session: two post-tool-use hooks of the same session (parallel tool
    /// calls) must not read the same unread messages. Entries are tiny and live as long as serve.
    locks: std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Post gate state per session (S61). In memory only: a restart forgets it, which is fine.
    gates: Mutex<HashMap<String, SenderGate>>,
    /// One async mutex per session around a whole `post` call (target lookup, gate, dm, writes,
    /// duplicate record): two calls of one session cannot both pass the duplicate check. Separate
    /// from `locks` (the injection lock), so a post never waits for a hook or the other way round.
    post_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    #[cfg(any(test, feature = "testkit"))]
    pub(crate) hook_log: Mutex<Vec<crate::agent::testkit::HookObservation>>,
}

impl AgentRuntime {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            ..Default::default()
        }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// The injection lock of one session (held across `inject::prepare` + `commit`).
    pub fn session_lock(&self, session: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(session.to_string()).or_default().clone()
    }

    /// The post lock of one session (held across one whole `post` call).
    pub fn post_lock(&self, session: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.post_locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(session.to_string()).or_default().clone()
    }

    /// The post gate (S61), checked before anything is written: a batch repeating itself, duplicate
    /// text, then the @human cap, then the bucket. Admitting takes the tokens (one per item) and the @human slot;
    /// a refusal takes nothing.
    pub fn admit_post(&self, a: &PostAttempt<'_>) -> Result<(), GateRefusal> {
        let normalized: Vec<String> = a.texts.iter().map(|t| normalize(t)).collect();
        for (i, t) in normalized.iter().enumerate() {
            if normalized[..i].contains(t) {
                return Err(GateRefusal::RepeatedInBatch { index: i + 1 });
            }
        }
        let now = self.clock.now();
        let cfg = &self.config;
        let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
        let g = gates.entry(a.session.to_string()).or_default();
        let dup_ms = i64::try_from(cfg.dup_window_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
        while g.recent.front().is_some_and(|r| now - r.at >= dup_ms) {
            g.recent.pop_front();
        }
        if let Some(ch) = a.channel {
            for t in &normalized {
                if let Some(r) = g
                    .recent
                    .iter()
                    .rev()
                    .find(|r| r.channel == ch && r.text == *t)
                {
                    return Err(GateRefusal::Duplicate { seq: r.seq });
                }
            }
        }
        while g
            .human_at
            .front()
            .is_some_and(|t| now - t >= HUMAN_WINDOW_MS)
        {
            g.human_at.pop_front();
        }
        if a.human_mention && g.human_at.len() >= cfg.human_mentions_per_30min as usize {
            return Err(GateRefusal::HumanMentions);
        }
        let burst = f64::from(cfg.post_burst);
        let refill_ms = cfg.post_refill_secs.max(1) as f64 * 1000.0;
        let tokens = match g.refilled_at {
            None => burst,
            Some(t) => (g.tokens + (now - t).max(0) as f64 / refill_ms).min(burst),
        };
        let cost = a.texts.len() as f64;
        if tokens + 1e-9 < cost {
            let retry_secs = ((cost - tokens) * refill_ms / 1000.0).ceil().max(1.0) as u64;
            return Err(GateRefusal::Throttled { retry_secs });
        }
        g.tokens = tokens - cost;
        g.refilled_at = Some(now);
        if a.human_mention {
            g.human_at.push_back(now);
        }
        Ok(())
    }

    /// Give back what `admit_post` took for `items` messages that were not written after all; with
    /// `human` also the @human slot (only when the message carrying the @ was not written).
    pub fn refund_post(&self, session: &str, items: usize, human: bool) {
        let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(g) = gates.get_mut(session) {
            g.tokens = (g.tokens + items as f64).min(f64::from(self.config.post_burst));
            if human {
                g.human_at.pop_back();
            }
        }
    }

    /// Remember a posted text for the duplicate check.
    pub fn note_posted(&self, session: &str, channel: &str, text: &str, seq: i64) {
        let at = self.clock.now();
        let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
        gates
            .entry(session.to_string())
            .or_default()
            .recent
            .push_back(Recent {
                channel: channel.to_string(),
                text: normalize(text),
                seq,
                at,
            });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    fn rt() -> (AgentRuntime, Arc<AtomicI64>) {
        let now = Arc::new(AtomicI64::new(0));
        let n = now.clone();
        let rt = AgentRuntime::new(AgentConfig::default())
            .with_clock(Clock::new(move || n.load(Ordering::SeqCst)));
        (rt, now)
    }

    fn attempt<'a>(texts: &'a [String], human: bool) -> PostAttempt<'a> {
        PostAttempt {
            session: "s-1",
            channel: Some("ch-1"),
            texts,
            human_mention: human,
        }
    }

    #[test]
    fn the_bucket_refills_one_post_per_interval_and_a_refusal_takes_nothing() {
        let (rt, now) = rt();
        let six: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
        assert_eq!(rt.admit_post(&attempt(&six, false)), Ok(()));
        let one = vec!["x".to_string()];
        assert_eq!(
            rt.admit_post(&attempt(&one, false)),
            Err(GateRefusal::Throttled { retry_secs: 20 })
        );
        now.store(10_000, Ordering::SeqCst);
        assert_eq!(
            rt.admit_post(&attempt(&one, false)),
            Err(GateRefusal::Throttled { retry_secs: 10 }),
            "half a token back after 10 s"
        );
        now.store(20_000, Ordering::SeqCst);
        assert_eq!(rt.admit_post(&attempt(&one, false)), Ok(()));
        // A refund puts the tokens back.
        rt.refund_post("s-1", 1, false);
        assert_eq!(rt.admit_post(&attempt(&one, false)), Ok(()));
    }

    #[test]
    fn duplicates_are_per_channel_and_expire() {
        let (rt, now) = rt();
        rt.note_posted("s-1", "ch-1", " a   b ", 7);
        let same = vec!["a b".to_string()];
        assert_eq!(
            rt.admit_post(&attempt(&same, false)),
            Err(GateRefusal::Duplicate { seq: 7 })
        );
        let elsewhere = PostAttempt {
            channel: Some("ch-2"),
            ..attempt(&same, false)
        };
        assert_eq!(rt.admit_post(&elsewhere), Ok(()));
        let new_dm = PostAttempt {
            channel: None,
            ..attempt(&same, false)
        };
        assert_eq!(rt.admit_post(&new_dm), Ok(()));
        now.store(600_000, Ordering::SeqCst);
        assert_eq!(rt.admit_post(&attempt(&same, false)), Ok(()));
    }
}

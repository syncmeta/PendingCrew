//! 选批与退避（spec §22.4「队列」）。纯函数。
//! Swift 是一次一条（`CrewDeferredWakeQueue.swift:50`）；这里到期的最高一档合并成一次投递（≤10 条，渲染后 ≤8KB 由 IO 层裁），
//! 少转几轮空转。
use crate::model::{MIN, Ms, QueuedWake, SEC, WakeState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchLimits {
    pub max_items: usize,
    /// 渲染后的正文上限（IO 层渲染后从尾部减**非消息**条数直到放得下，至少留 1 条）。消息项只是建议值：
    /// 消息窗口由 M3a `build_batch` 给定、不能截半个（游标推到窗口末尾），只剩消息项时照原样投（R6）。
    pub max_bytes: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        Self {
            max_items: 10,
            max_bytes: 8 * 1024,
        }
    }
}

pub fn is_due(w: &QueuedWake, now: Ms) -> bool {
    w.state == WakeState::Pending && w.not_before <= now
}

/// 到期的 pending 里只取最高一档优先级，按 `(created_at, id)`，最多 `max_items`（至少 1）条。
/// `pending` 必须是**同一个 session** 的行：批是「一个 session 一次投递」，混进别的 session 的行会被一起选走。
pub fn select_batch(pending: &[QueuedWake], now: Ms, limits: &BatchLimits) -> Vec<QueuedWake> {
    let Some(top) = pending
        .iter()
        .filter(|w| is_due(w, now))
        .map(|w| w.priority)
        .max()
    else {
        return vec![];
    };
    let mut tier: Vec<QueuedWake> = pending
        .iter()
        .filter(|w| is_due(w, now) && w.priority == top)
        .cloned()
        .collect();
    tier.sort_by_key(|w| (w.created_at, w.id));
    tier.truncate(limits.max_items.max(1));
    tier
}

pub const RETRY_DELAYS_MS: [i64; 2] = [30 * SEC, 2 * MIN];
/// 总共试几次：第一次 + 每个退避间隔一次重试。从 `RETRY_DELAYS_MS` 推出来，两者不会漂。
pub const MAX_ATTEMPTS: u32 = RETRY_DELAYS_MS.len() as u32 + 1;
/// 重试时刻的抖动幅度：±20%（River 是 ±10%）。一次 API 故障里一起失败的几十个 session 不会在同一毫秒一起重试。
pub const RETRY_JITTER: f64 = 0.2;

/// 一次投递终局失败之后：`attempts` 是**库里**记的次数（claim 时 +1，含这一次）。`None` = 放弃（记 dropped）。
/// `jitter` ∈ [-[`RETRY_JITTER`], +[`RETRY_JITTER`]]（超出的按边界算）：退避 × (1 + jitter)。由调用方抽
/// （[`Jitter`]，种子注入），这里不碰随机数，测试照样确定。
pub fn after_failure(attempts: u32, now: Ms, jitter: f64) -> Option<Ms> {
    if attempts >= MAX_ATTEMPTS {
        return None;
    }
    let i = attempts.max(1) as usize - 1;
    let j = if jitter.is_finite() {
        jitter.clamp(-RETRY_JITTER, RETRY_JITTER)
    } else {
        0.0
    };
    RETRY_DELAYS_MS
        .get(i)
        .map(|d| now.add((*d as f64 * (1.0 + j)).round() as i64))
}

/// 退避抖动的来源：splitmix64，种子由调用方给（serve 用启动时刻，测试用固定值）——不引第三方 crate、不读时钟。
#[derive(Clone, Debug)]
pub struct Jitter {
    state: u64,
    spread: f64,
}

impl Jitter {
    /// `spread` = 抖动幅度（0 = 不抖；正常是 [`RETRY_JITTER`]）。
    pub fn new(seed: u64, spread: f64) -> Self {
        Self {
            state: seed,
            spread: spread.clamp(0.0, RETRY_JITTER),
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 下一个抖动值，均匀落在 [-spread, +spread]。
    pub fn draw(&mut self) -> f64 {
        // 高 53 位 → [0, 1)。
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        (unit * 2.0 - 1.0) * self.spread
    }
}

pub fn ids(batch: &[QueuedWake]) -> Vec<i64> {
    batch.iter().map(|w| w.id).collect()
}

/// 放弃时要不要在群里告警：任何优先级的批都告（控制者裁定 C19，含没有 origin 的 timer / system 唤醒——
/// 静默丢弃比告警更糟）；空批没有可告的。告警本身可以 @ 群主而入队一次唤醒，所以**不能**拿「告警不是唤醒」
/// 当成环安全的理由：安全来自 Q5 熔断（边 + 目标两级；system 来源的唤醒计入目标级，人发起的才永远放行），
/// 由发告警的一方保证它的 origin 是 system、不是人。
pub fn should_alert(batch: &[QueuedWake]) -> bool {
    !batch.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Priority, WakeKind};

    pub(crate) fn w(id: i64, priority: Priority, not_before: i64, created_at: i64) -> QueuedWake {
        QueuedWake {
            id,
            session: "s".into(),
            kind: WakeKind::Message,
            source: format!("msg:m-{id}"),
            priority,
            not_before: Ms(not_before),
            note: None,
            state: WakeState::Pending,
            attempts: 0,
            created_at: Ms(created_at),
            origin: "cap".into(),
            msg: None,
        }
    }

    #[test]
    fn batch_takes_only_the_highest_due_tier_in_creation_order() {
        let pending = vec![
            w(1, Priority::Auto, 0, 1),
            w(2, Priority::Human, 0, 5),
            w(3, Priority::Human, 0, 2),
            w(4, Priority::Human, 99, 0), // 没到点
            w(5, Priority::System, 0, 0),
        ];
        let got: Vec<i64> = select_batch(&pending, Ms(10), &BatchLimits::default())
            .iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(got, [3, 2]);
    }

    #[test]
    fn a_not_yet_due_human_row_does_not_shadow_a_due_auto_row() {
        let pending = vec![w(1, Priority::Human, 99, 0), w(2, Priority::Auto, 0, 1)];
        assert_eq!(
            ids(&select_batch(&pending, Ms(10), &BatchLimits::default())),
            [2]
        );
    }

    #[test]
    fn an_inflight_human_row_does_not_shadow_a_due_auto_row() {
        let mut h = w(1, Priority::Human, 0, 0);
        h.state = WakeState::Inflight;
        let pending = vec![h, w(2, Priority::Auto, 0, 1)];
        assert_eq!(
            ids(&select_batch(&pending, Ms(10), &BatchLimits::default())),
            [2]
        );
    }

    #[test]
    fn max_attempts_is_one_more_than_the_back_off_steps() {
        assert_eq!(MAX_ATTEMPTS as usize, RETRY_DELAYS_MS.len() + 1);
        assert!(after_failure(MAX_ATTEMPTS - 1, Ms(0), 0.0).is_some());
        assert_eq!(after_failure(MAX_ATTEMPTS, Ms(0), 0.0), None);
    }

    #[test]
    fn non_pending_rows_are_never_selected_and_ties_break_by_id() {
        let mut a = w(7, Priority::Auto, 0, 1);
        a.state = WakeState::Inflight;
        let pending = vec![a, w(9, Priority::Auto, 0, 1), w(8, Priority::Auto, 0, 1)];
        let got: Vec<i64> = select_batch(&pending, Ms(1), &BatchLimits::default())
            .iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(got, [8, 9]);
        assert!(select_batch(&[], Ms(1), &BatchLimits::default()).is_empty());
    }

    #[test]
    fn batch_is_capped_at_max_items_but_never_empty_when_something_is_due() {
        let pending: Vec<QueuedWake> = (1..=15).map(|i| w(i, Priority::Auto, 0, i)).collect();
        assert_eq!(
            select_batch(&pending, Ms(0), &BatchLimits::default()).len(),
            10
        );
        let one = BatchLimits {
            max_items: 0,
            max_bytes: 1,
        };
        assert_eq!(select_batch(&pending, Ms(0), &one).len(), 1);
    }

    #[test]
    fn failures_back_off_then_give_up_on_the_third() {
        assert_eq!(after_failure(1, Ms(0), 0.0), Some(Ms(30 * SEC)));
        assert_eq!(after_failure(2, Ms(0), 0.0), Some(Ms(2 * MIN)));
        assert_eq!(after_failure(3, Ms(0), 0.0), None);
        assert_eq!(after_failure(9, Ms(0), 0.0), None);
    }

    /// 抖动 = 退避 × (1 + jitter)，幅度封顶 ±20%（超出的、非数按边界 / 0 算）；放弃不受抖动影响。
    #[test]
    fn jitter_stretches_or_shrinks_the_delay_by_at_most_twenty_percent() {
        assert_eq!(after_failure(1, Ms(0), 0.2), Some(Ms(36 * SEC)));
        assert_eq!(after_failure(1, Ms(0), -0.2), Some(Ms(24 * SEC)));
        assert_eq!(after_failure(2, Ms(0), 0.1), Some(Ms(132 * SEC)));
        assert_eq!(after_failure(1, Ms(0), 5.0), Some(Ms(36 * SEC)), "封顶");
        assert_eq!(after_failure(1, Ms(0), f64::NAN), Some(Ms(30 * SEC)));
        assert_eq!(after_failure(MAX_ATTEMPTS, Ms(0), 0.2), None);
    }

    /// 同一个种子给同一串数（测试确定）；全都落在 ±spread 里；不是常数（真的会把一起失败的错开）；spread 0 = 不抖。
    #[test]
    fn jitter_draws_are_seeded_bounded_and_spread() {
        let draws = |seed| {
            let mut j = Jitter::new(seed, RETRY_JITTER);
            (0..200).map(|_| j.draw()).collect::<Vec<f64>>()
        };
        let a = draws(7);
        assert_eq!(a, draws(7), "同一个种子同一串");
        assert_ne!(a, draws(8));
        assert!(a.iter().all(|x| (-RETRY_JITTER..=RETRY_JITTER).contains(x)));
        assert!(
            a.iter().any(|x| *x > 0.1) && a.iter().any(|x| *x < -0.1),
            "两边都有"
        );
        let mut flat = Jitter::new(7, 0.0);
        assert!((0..20).all(|_| flat.draw() == 0.0));
        let mut wide = Jitter::new(7, 9.0);
        assert!(
            (0..200).all(|_| wide.draw().abs() <= RETRY_JITTER),
            "幅度封顶"
        );
    }

    #[test]
    fn every_priority_alerts_on_give_up_but_an_empty_batch_does_not() {
        assert!(
            should_alert(&[w(1, Priority::System, 0, 0)]),
            "C19: system-only batches alert too"
        );
        assert!(!should_alert(&[]));
        assert!(should_alert(&[
            w(1, Priority::System, 0, 0),
            w(2, Priority::Auto, 0, 0)
        ]));
        assert!(should_alert(&[w(1, Priority::Human, 0, 0)]));
        assert_eq!(
            ids(&[w(4, Priority::Auto, 0, 0), w(2, Priority::Auto, 0, 0)]),
            [4, 2]
        );
    }
}

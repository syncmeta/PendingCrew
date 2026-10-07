//! 会话生命周期的几条策略（纯函数）：额度重置唤醒的时刻、重启后的恢复名单顺序、启动闸（可选上限 + 错开）。
use crate::model::{MIN, Ms, QueuedWake, SEC, WakeKind, WakeState};
use crate::queue::is_due;
use crate::text::{Lang, get, render};

/// 重置后再缓 1 分钟（额度窗刚翻新，别掐点撞上仍受限的边缘；`AgentQuota.swift:342`）。
pub const QUOTA_RESET_BUFFER_MS: i64 = MIN;
/// 拿不到可信重置时刻时的退避（`AgentQuota.swift:344`）。
pub const QUOTA_FALLBACK_MS: i64 = 45 * MIN;
pub const QUOTA_SOURCE_PREFIX: &str = "quota:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaPlan {
    pub fire_at: Ms,
    /// true = 没拿到可信重置时刻，走的退避（群里要说清，Swift 的 fail-loud）。
    pub fallback: bool,
}

impl QuotaPlan {
    pub fn source(&self) -> String {
        crate::model::source::quota(self.fire_at.0)
    }
}

pub fn quota_plan(reset_at: Option<Ms>, now: Ms) -> QuotaPlan {
    match reset_at {
        Some(r) if r > now => QuotaPlan {
            fire_at: r.add(QUOTA_RESET_BUFFER_MS),
            fallback: false,
        },
        _ => QuotaPlan {
            fire_at: now.add(QUOTA_FALLBACK_MS),
            fallback: true,
        },
    }
}

fn is_quota(w: &QueuedWake) -> bool {
    w.source.starts_with(QUOTA_SOURCE_PREFIX)
}

/// 已挂着一条还没投完的额度唤醒（pending / inflight）。
pub fn quota_pending(pending: &[QueuedWake]) -> bool {
    pending
        .iter()
        .any(|w| is_quota(w) && matches!(w.state, WakeState::Pending | WakeState::Inflight))
}

pub fn quota_due(pending: &[QueuedWake], now: Ms) -> bool {
    pending.iter().any(|w| is_quota(w) && is_due(w, now))
}

/// 重启时「上次在跑」的一行（`desired=running`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartRow {
    pub session: String,
    pub name: String,
    pub crew: String,
    pub captain: bool,
    /// 上次停在干活中途（`was_working=1`）：恢复时补一条「被打断」附言。
    pub was_working: bool,
    pub last_active_at: Option<Ms>,
}

/// 恢复名单的顺序：机长在前，然后最近活跃的在前，最后按 id（§22.7 Q1：人答「恢复」后按这个顺序错开拉起）。
pub fn restart_order(mut rows: Vec<RestartRow>) -> Vec<RestartRow> {
    rows.sort_by(|a, b| {
        b.captain
            .cmp(&a.captain)
            .then(b.last_active_at.cmp(&a.last_active_at))
            .then(a.session.cmp(&b.session))
    });
    rows
}

/// 重启询问的恢复唤醒：`tag` = M3a `restart_prompts` 的 id。
pub fn restart_source(tag: &str) -> String {
    crate::model::source::restart(tag)
}

/// 两次拉起之间至少隔这么久（§22.7 Q1「错开时间逐个」）。
pub const DEFAULT_STAGGER_MS: i64 = 2 * SEC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Go,
    /// 可选上限已满（只挡自动拉起）。
    AtCap,
    /// 离上一次拉起太近，这一刻之后再试。
    WaitUntil(Ms),
}

/// 启动闸。**默认没有上限**（§22.7 Q5）；上限只挡自动拉起，人起的（`bypass_cap`）不挡。错开对谁都生效。
#[derive(Clone, Debug)]
pub struct LaunchGate {
    max_live: Option<usize>,
    stagger_ms: i64,
    last: Option<Ms>,
}

impl LaunchGate {
    pub fn new(max_live: Option<usize>, stagger_ms: i64) -> Self {
        Self {
            max_live,
            stagger_ms,
            last: None,
        }
    }

    /// `live` = 正在跑 + 正在起的会话数。
    pub fn check(&self, live: usize, bypass_cap: bool, now: Ms) -> Gate {
        if !bypass_cap && self.max_live.is_some_and(|m| live >= m) {
            return Gate::AtCap;
        }
        match self.last {
            Some(t) if now.since(t) < self.stagger_ms => Gate::WaitUntil(t.add(self.stagger_ms)),
            _ => Gate::Go,
        }
    }

    pub fn launched(&mut self, now: Ms) {
        self.last = Some(self.last.map_or(now, |t| t.max(now)));
    }

    pub fn set_max_live(&mut self, max: Option<usize>) {
        self.max_live = max;
    }

    pub fn max_live(&self) -> Option<usize> {
        self.max_live
    }
}

/// 休眠（C26）的默认期限（秒）：在干活的 session 最多等这么久到达安全点（这一轮结束），之后打断。0 = 立即休眠。
pub const DEFAULT_HIBERNATE_DEADLINE_SECS: u32 = 120;

/// 被休眠的 session 恢复时附言要的事实（C26）。`doing` 是外来文字（唤醒附言、群 id），调用方先过 `safe_name`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HibernateFacts<'a> {
    /// 休眠开始时它在干活（或正有一批在投）。
    pub was_working: bool,
    /// 到了期限它还没做完那一轮，被打断了。
    pub interrupted: bool,
    /// 停下之前在处理的那批唤醒（[`batch_summary`]）；不知道 = None。
    pub doing: Option<&'a str>,
}

/// 休眠恢复时给 agent 的附言：是休眠暂停的；停下时在做什么；做完了 / 被打断了（被打断的 → 被打断的命令要重跑）。
/// 休眠时空闲的（没在干活、没被打断）= None：没有要交代的，静默恢复（评审 I1，同崩溃重启时没在干活的）。
pub fn hibernate_note(lang: Lang, f: &HibernateFacts<'_>) -> Option<String> {
    if !f.was_working && !f.interrupted {
        return None;
    }
    let doing = match f.doing {
        Some(doing) => render(lang, "hibernate.doing", &[("doing", doing)]),
        None => get(lang, "hibernate.doing.unknown").to_string(),
    };
    let tail = if f.interrupted {
        "hibernate.interrupted"
    } else {
        "hibernate.finished"
    };
    Some(
        [
            get(lang, "hibernate.resume").to_string(),
            doing,
            get(lang, tail).to_string(),
        ]
        .join("\n\n"),
    )
}

/// [`batch_summary`] 至多多少个字符（含每条附言的截断）。
pub const SUMMARY_MAX_CHARS: usize = 400;
/// 摘要里每条附言至多多少个字符。
const NOTE_MAX_CHARS: usize = 80;

/// 一批唤醒的摘要（休眠记录里的「停下时在做什么」，C26）：每条 `种类 来源[: 附言]`，消息类写成 `message 群#序号`；
/// 恢复类的附言不再套进来（那是上一次的附言）。是数据、不是文案；插进附言前要过 `safe_name`。空批 = None。
pub fn batch_summary(wakes: &[QueuedWake]) -> Option<String> {
    if wakes.is_empty() {
        return None;
    }
    let items: Vec<String> = wakes
        .iter()
        .map(|w| {
            let head = match (&w.kind, &w.msg) {
                (WakeKind::Message, Some(r)) => format!("{} {}#{}", w.kind, r.channel, r.seq),
                _ => format!("{} {}", w.kind, w.source),
            };
            match &w.note {
                Some(n) if w.kind != WakeKind::Restart => {
                    let n: String = n.chars().take(NOTE_MAX_CHARS).collect();
                    format!("{head}: {n}")
                }
                _ => head,
            }
        })
        .collect();
    Some(items.join("; ").chars().take(SUMMARY_MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Priority;

    fn quota_wake(source: &str, state: WakeState, not_before: i64) -> QueuedWake {
        QueuedWake {
            id: 1,
            session: "s".into(),
            kind: WakeKind::System,
            source: source.into(),
            priority: Priority::System,
            not_before: Ms(not_before),
            note: None,
            state,
            attempts: 0,
            created_at: Ms(0),
            origin: "system".into(),
            msg: None,
            last_error: None,
        }
    }

    #[test]
    fn quota_wake_fires_a_minute_after_a_future_reset_else_backs_off() {
        let now = Ms(1_000 * MIN);
        let p = quota_plan(Some(now.add(30 * MIN)), now);
        assert_eq!(
            p,
            QuotaPlan {
                fire_at: now.add(31 * MIN),
                fallback: false
            }
        );
        assert_eq!(p.source(), format!("quota:{}", now.add(31 * MIN).0));
        for stale in [None, Some(now), Some(now.add(-MIN))] {
            assert_eq!(
                quota_plan(stale, now),
                QuotaPlan {
                    fire_at: now.add(45 * MIN),
                    fallback: true
                }
            );
        }
    }

    #[test]
    fn quota_pending_and_due_look_only_at_quota_sources() {
        let p = vec![
            quota_wake("quota:5", WakeState::Pending, 50),
            quota_wake("timer:x", WakeState::Pending, 0),
        ];
        assert!(quota_pending(&p));
        assert!(!quota_due(&p, Ms(49)));
        assert!(quota_due(&p, Ms(50)));
        assert!(!quota_pending(&[quota_wake(
            "quota:5",
            WakeState::Delivered,
            0
        )]));
        assert!(quota_pending(&[quota_wake(
            "quota:5",
            WakeState::Inflight,
            0
        )]));
    }

    fn row(session: &str, captain: bool, was_working: bool, last: Option<i64>) -> RestartRow {
        RestartRow {
            session: session.into(),
            name: session.into(),
            crew: "c".into(),
            captain,
            was_working,
            last_active_at: last.map(Ms),
        }
    }

    #[test]
    fn restart_order_puts_captains_first_then_most_recently_active() {
        let got: Vec<String> = restart_order(vec![
            row("w-old", false, false, Some(1)),
            row("w-never", false, false, None),
            row("cap", true, false, Some(0)),
            row("w-new", false, true, Some(9)),
        ])
        .into_iter()
        .map(|r| r.session)
        .collect();
        assert_eq!(got, ["cap", "w-new", "w-old", "w-never"]);
        assert_eq!(restart_source("b1"), "restart:b1");
    }

    #[test]
    fn restart_order_breaks_ties_by_session_id_and_ranks_none_below_some() {
        let ids = |rows: Vec<RestartRow>| -> Vec<String> {
            restart_order(rows).into_iter().map(|r| r.session).collect()
        };
        // 同样活跃时间 → 按 id 升序，与输入顺序无关。
        assert_eq!(
            ids(vec![
                row("b", false, false, Some(5)),
                row("a", false, false, Some(5))
            ]),
            ["a", "b"]
        );
        assert_eq!(
            ids(vec![
                row("a", false, false, Some(5)),
                row("b", false, false, Some(5))
            ]),
            ["a", "b"]
        );
        // 两个都没记过活跃时间 → 同样按 id；两个机长之间也一样。
        assert_eq!(
            ids(vec![
                row("z", false, false, None),
                row("m", false, false, None)
            ]),
            ["m", "z"]
        );
        assert_eq!(
            ids(vec![
                row("c2", true, false, None),
                row("c1", true, false, None)
            ]),
            ["c1", "c2"]
        );
        // None 排在任何 Some 之后（包括 Some(0)）。
        assert_eq!(
            ids(vec![
                row("a", false, false, None),
                row("b", false, false, Some(0))
            ]),
            ["b", "a"]
        );
    }

    fn wake(id: i64, kind: WakeKind, source: &str, note: Option<&str>) -> QueuedWake {
        QueuedWake {
            id,
            kind,
            source: source.into(),
            note: note.map(str::to_string),
            ..quota_wake("x", WakeState::Delivered, 0)
        }
    }

    /// C26：休眠恢复的附言——说清是休眠暂停的、停下时在做什么、被打断的要重跑；空闲的不给附言（评审 I1：静默恢复）。
    #[test]
    fn hibernate_note_says_it_was_suspended_what_it_was_doing_and_to_rerun_if_interrupted() {
        use crate::text::{Lang, get, render};
        for lang in [Lang::Zh, Lang::En] {
            let doing = "message c1/lobby#3";
            let cut = hibernate_note(
                lang,
                &HibernateFacts {
                    was_working: true,
                    interrupted: true,
                    doing: Some(doing),
                },
            )
            .expect("在干活的有附言");
            assert!(cut.starts_with(get(lang, "hibernate.resume")), "{cut}");
            assert!(cut.contains(&render(lang, "hibernate.doing", &[("doing", doing)])));
            assert!(cut.contains(get(lang, "hibernate.interrupted")), "{cut}");
            assert!(!cut.contains(get(lang, "hibernate.finished")));
            let done = hibernate_note(
                lang,
                &HibernateFacts {
                    was_working: true,
                    interrupted: false,
                    doing: None,
                },
            )
            .expect("在干活的有附言");
            assert!(
                done.contains(get(lang, "hibernate.doing.unknown")),
                "{done}"
            );
            assert!(done.contains(get(lang, "hibernate.finished")));
            assert!(!done.contains(get(lang, "hibernate.interrupted")));
            let idle = hibernate_note(
                lang,
                &HibernateFacts {
                    was_working: false,
                    interrupted: false,
                    doing: Some(doing),
                },
            );
            assert_eq!(idle, None, "空闲的：不给附言，静默恢复");
            // 被打断就一定是在干活（防御：调用方给错了也按「被打断」说，不漏掉「要重跑」）。
            let odd = hibernate_note(
                lang,
                &HibernateFacts {
                    was_working: false,
                    interrupted: true,
                    doing: None,
                },
            )
            .expect("被打断的一定有附言");
            assert!(odd.contains(get(lang, "hibernate.interrupted")));
        }
    }

    #[test]
    fn batch_summary_lists_each_wake_and_caps_long_notes() {
        assert_eq!(batch_summary(&[]), None);
        let mut m = wake(1, WakeKind::Message, "msg:m-1", None);
        m.msg = Some(crate::model::MessageRef {
            channel: "c1/lobby".into(),
            seq: 3,
        });
        let long = "x".repeat(500);
        let got = batch_summary(&[
            m,
            wake(2, WakeKind::Timer, "timer:t1", Some("check CI")),
            wake(
                3,
                WakeKind::Restart,
                "restart:7",
                Some("an older resume note"),
            ),
            wake(4, WakeKind::System, "system:s", Some(&long)),
        ])
        .unwrap();
        assert!(
            got.starts_with(
                "message c1/lobby#3; timer timer:t1: check CI; restart restart:7; system system:s: "
            ),
            "{got}"
        );
        assert!(
            !got.contains("an older resume note"),
            "恢复附言不再套进摘要"
        );
        assert!(got.chars().count() <= SUMMARY_MAX_CHARS, "{}", got.len());
    }

    #[test]
    fn launch_gate_has_no_cap_by_default_and_staggers_launches() {
        let mut g = LaunchGate::new(None, DEFAULT_STAGGER_MS);
        assert_eq!(
            g.check(10_000, false, Ms(0)),
            Gate::Go,
            "默认不设上限（§22.7 Q5）"
        );
        g.launched(Ms(0));
        assert_eq!(
            g.check(1, false, Ms(500)),
            Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS))
        );
        assert_eq!(
            g.check(1, true, Ms(500)),
            Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)),
            "错开对谁都生效"
        );
        assert_eq!(g.check(1, false, Ms(DEFAULT_STAGGER_MS)), Gate::Go);
        g.launched(Ms(-5));
        assert_eq!(
            g.check(1, false, Ms(500)),
            Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)),
            "launched 只往后推"
        );
    }

    #[test]
    fn optional_cap_blocks_only_automatic_launches() {
        let mut g = LaunchGate::new(None, 0);
        g.set_max_live(Some(3));
        assert_eq!(g.max_live(), Some(3));
        assert_eq!(g.check(2, false, Ms(0)), Gate::Go);
        assert_eq!(g.check(3, false, Ms(0)), Gate::AtCap);
        assert_eq!(g.check(3, true, Ms(0)), Gate::Go, "人起的不受上限");
        g.set_max_live(None);
        assert_eq!(g.check(3, false, Ms(0)), Gate::Go);
    }
}

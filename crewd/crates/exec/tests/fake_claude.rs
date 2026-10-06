//! 端到端：真 PTY + 假 claude（`src/bin/fake_claude.rs`）。投递、粘贴陷阱、信任框（回放
//! M2a 的真录制字节）、限流菜单自动应答、额度用尽退出。不碰真 claude（真机见 `live_claude.rs`）。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crewd_exec::driver::{ClaudeStart, DriverConfig, SessionHandle, start_claude};
use crewd_exec::health::HealthKind;
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::{BRIEF_DELIVERY_ID, DeliveryOutcome, ExitStatusKind, SessionEvent};
use crewd_term::DecisionEvent;
use tokio::sync::mpsc::UnboundedReceiver;

const FAKE: &str = env!("CARGO_BIN_EXE_fake_claude");

fn trust_fixture() -> String {
    let path = format!(
        "{}/../term/tests/fixtures/tui-claude-trust.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    assert!(std::path::Path::new(&path).exists(), "缺夹具 {path}");
    path
}

struct Run {
    h: SessionHandle,
    rx: UnboundedReceiver<SessionEvent>,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

/// 每条用例一个 crewd 会话号；claude 会话号由空账本现生成（小写 UUID）。同一进程里并行的
/// 用例不能共用会话号（驱动层对同号第二次启动报 `AlreadyRunning`）。
fn next_session_id() -> String {
    static N: AtomicU64 = AtomicU64::new(1);
    format!("worker-{}", N.fetch_add(1, Ordering::Relaxed))
}

async fn start(fake_env: &[(&str, &str)], brief: Option<&str>) -> Run {
    start_with(fake_env, brief, MemoryLedger::default()).await
}

async fn start_with(fake_env: &[(&str, &str)], brief: Option<&str>, ledger: MemoryLedger) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("submitted.log");
    let mut additional_env: BTreeMap<String, String> = fake_env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    additional_env.insert("FAKE_CLAUDE_LOG".into(), log.display().to_string());
    let mut config = DriverConfig::new(dir.path().join("run"));
    config.stop_grace = Duration::from_millis(500);
    // 缩短重试窗口，「投不进去」那条不用等 7.5s。
    config.core.delivery.verify_window = Duration::from_millis(800);
    config.core.delivery.submit_gap = Duration::from_millis(300);
    let start = ClaudeStart {
        session_id: next_session_id(),
        program: PathBuf::from(FAKE),
        child_path: "/usr/bin:/bin".into(),
        spec: ClaudeSpec {
            workdir: dir.path().to_path_buf(),
            initial_prompt: brief.map(str::to_string),
            additional_env,
            ..ClaudeSpec::default()
        },
        ledger: Arc::new(ledger),
    };
    let (h, rx) = start_claude(start, config).await.unwrap();
    Run {
        h,
        rx,
        log,
        _dir: dir,
    }
}

impl Run {
    async fn until(&mut self, pred: impl Fn(&SessionEvent) -> bool) -> Vec<SessionEvent> {
        let mut seen = Vec::new();
        loop {
            let e = tokio::time::timeout(Duration::from_secs(15), self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("15s 内没等到；已收到 {seen:?}"))
                .expect("事件流提前结束");
            let hit = pred(&e);
            seen.push(e);
            if hit {
                return seen;
            }
        }
    }

    fn submitted(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    async fn stop(mut self) {
        let _ = self.h.stop();
        self.until(|e| matches!(e, SessionEvent::Exited(_))).await;
    }
}

fn outcome(id: u64, pred: impl Fn(&DeliveryOutcome) -> bool) -> impl Fn(&SessionEvent) -> bool {
    move |e| matches!(e, SessionEvent::Delivery { id: i, outcome, .. } if *i == id && pred(outcome))
}

#[tokio::test]
async fn brief_waits_out_a_slow_first_screen_then_is_submitted_once() {
    let mut r = start(
        &[("FAKE_CLAUDE_READY_DELAY_MS", "1500")],
        Some("开场任务：只回答 OK"),
    )
    .await;
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(
        r.submitted(),
        ["开场任务：只回答 OK"],
        "只提交一次、一字不差"
    );
    assert!(r.h.screen_text(25).await.unwrap().contains("GOT: 开场任务"));
    r.stop().await;
}

#[tokio::test]
async fn long_brief_folded_into_a_paste_placeholder_still_lands() {
    let brief: String = (1..=40).map(|i| format!("第 {i} 行说明\n")).collect();
    let mut r = start(&[], Some(&brief)).await;
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.submitted(), [brief.replace('\n', "\\n")]);
    r.stop().await;
}

#[tokio::test]
async fn steers_are_queued_and_each_submitted_once_in_order() {
    let mut r = start(&[], None).await;
    let a = r.h.send("第一条").unwrap();
    let b = r.h.send("第二条").unwrap();
    r.until(outcome(a, |o| *o == DeliveryOutcome::Delivered))
        .await;
    r.until(outcome(b, |o| *o == DeliveryOutcome::Delivered))
        .await;
    assert_eq!(r.submitted(), ["第一条", "第二条"]);
    r.stop().await;
}

/// 器材自检：假 claude 真的模拟了粘贴陷阱——正文和回车同一笔到达，回车被当成换行；
/// 紧跟着单独发回车也只结束粘贴；先来一个普通按键才提交。没有这一条，上面几条绿了
/// 也证明不了「空格 + 退格 + 回车」三拍有必要。
#[tokio::test]
async fn fake_models_the_paste_trap() {
    let mut r = start(&[], None).await;
    r.until(|e| matches!(e, SessionEvent::Working(false))).await;
    let settle = || tokio::time::sleep(Duration::from_millis(300));
    r.h.raw(b"hello\r".to_vec()).unwrap();
    settle().await;
    r.h.raw(b"\r".to_vec()).unwrap();
    settle().await;
    assert!(
        r.submitted().is_empty(),
        "粘贴里的 CR、紧跟的单独回车都不该提交"
    );
    for key in [&b" "[..], &[0x7f], b"\r"] {
        r.h.raw(key.to_vec()).unwrap();
        settle().await;
    }
    assert_eq!(r.submitted(), ["hello\\n"], "普通按键之后的回车才提交");
    r.stop().await;
}

#[tokio::test]
async fn deaf_input_box_is_reported_undelivered() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "deaf")], Some("没人听")).await;
    let seen = r
        .until(outcome(BRIEF_DELIVERY_ID, |o| {
            matches!(o, DeliveryOutcome::Undelivered(_))
        }))
        .await;
    assert!(seen.iter().any(
        |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::BriefUndelivered)
    ));
    r.stop().await;
}

/// 回放真 claude 录下来的信任框首屏：不投、报待决策（无编号）与 Blocked；有人按「↓ 回车」代答后自己补投。
#[tokio::test]
async fn recorded_trust_dialog_blocks_until_answered_then_brief_lands() {
    let fixture = trust_fixture();
    let mut r = start(
        &[
            ("FAKE_CLAUDE_MODE", "trust"),
            ("FAKE_CLAUDE_REPLAY", fixture.as_str()),
        ],
        Some("信任之后再干活"),
    )
    .await;
    // Blocked（投递机 3s）与 Appeared（待决策跟踪器 3s，挂在 0.6s 节拍上）谁先到不一定。
    let blocked = outcome(BRIEF_DELIVERY_ID, |o| {
        matches!(o, DeliveryOutcome::Blocked(_))
    });
    let appeared = |e: &SessionEvent| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(d)) if !d.numbered);
    let mut seen = r.until(|e| blocked(e) || appeared(e)).await;
    if !seen.iter().any(&blocked) {
        seen.extend(r.until(&blocked).await);
    }
    if !seen.iter().any(appeared) {
        seen.extend(r.until(appeared).await);
    }
    assert!(r.submitted().is_empty());
    r.h.raw(b"\x1b[B".to_vec()).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    r.h.raw(b"\r".to_vec()).unwrap();
    let seen = r
        .until(outcome(BRIEF_DELIVERY_ID, |o| {
            *o == DeliveryOutcome::Delivered
        }))
        .await;
    assert!(
        seen.contains(&SessionEvent::Decision(DecisionEvent::Cleared)),
        "{seen:?}"
    );
    assert!(
        seen.contains(&SessionEvent::Health(None)),
        "补投成功后清掉 BriefUndelivered"
    );
    assert_eq!(r.submitted(), ["信任之后再干活"]);
    r.stop().await;
}

#[tokio::test]
async fn rate_limit_menu_is_answered_automatically() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "ratelimit")], None).await;
    r.h.send("触发限流").unwrap();
    r.until(|e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::RateLimited))
        .await;
    // 最多等 10s：满载的 check.sh 里「1」与回车之间、假 claude 落日志都可能慢。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while r.submitted().len() < 2 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(r.submitted(), ["触发限流", "RATE-ANSWER:1"]);
    r.stop().await;
}

#[tokio::test]
async fn usage_limit_then_exit_is_marked_hit_limit() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "usage")], Some("干活")).await;
    let seen = r.until(|e| matches!(e, SessionEvent::Exited(_))).await;
    assert!(
        seen.iter().any(
            |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::UsageLimit)
        )
    );
    let Some(SessionEvent::Exited(info)) = seen.last() else {
        unreachable!()
    };
    assert_eq!(info.status, ExitStatusKind::Code(1));
    assert!(info.hit_limit && !info.stopped_by_user);
}

/// Review Focus 2：子进程不读输入、我们却要写一大段——写任务卡住，但会话不能跟着卡死：
/// 事件照常、停得掉。
#[tokio::test]
async fn a_child_that_never_reads_cannot_wedge_the_session() {
    let r = start(&[("FAKE_CLAUDE_MODE", "noread")], None).await;
    r.h.send("x".repeat(1_000_000)).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let status = tokio::time::timeout(Duration::from_secs(5), r.h.status())
        .await
        .expect("写卡住时状态查询也必须能答")
        .unwrap();
    assert!(status.pid > 0);
    r.stop().await;
}

/// Review Focus 5：几十 KB 的开场任务（首轮白板）——只提交一次、一字不差，不因验证窗口
/// 超时重投出两份。
#[tokio::test]
async fn huge_brief_is_submitted_exactly_once() {
    let brief: String = (0..2000)
        .map(|i| format!("{i:05} 白板条目，带点中文\n"))
        .collect();
    assert!(brief.len() > 60_000);
    let mut r = start(&[], Some(&brief)).await;
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.submitted(), [brief.replace('\n', "\\n")]);
    r.stop().await;
}

/// 续跑被拒（claude 打印原话后退出）：换新号重起，开场任务搬到新进程、只提交一次，
/// 一路上不报「被丢下」（Abandoned）。
#[tokio::test]
async fn brief_carries_over_to_the_fresh_session_after_a_refused_resume() {
    let mut r = start_with(
        &[("FAKE_CLAUDE_MODE", "refuse")],
        Some("续跑被拒也要送到"),
        MemoryLedger::with(Some("gone-id")),
    )
    .await;
    let seen = r
        .until(outcome(BRIEF_DELIVERY_ID, |o| {
            *o == DeliveryOutcome::Delivered
        }))
        .await;
    assert!(
        seen.iter()
            .any(|e| matches!(e, SessionEvent::ResumeRefused { id, .. } if id == "gone-id")),
        "前提：续跑被拒：{seen:?}"
    );
    let fresh = seen
        .iter()
        .find_map(|e| match e {
            SessionEvent::Spawned {
                agent_session_id,
                resumed: false,
                ..
            } => Some(agent_session_id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("没有换新号重起：{seen:?}"));
    assert_ne!(fresh, "gone-id");
    assert!(
        !seen.iter().any(|e| matches!(
            e,
            SessionEvent::Delivery {
                outcome: DeliveryOutcome::Abandoned(_),
                ..
            }
        )),
        "开场任务不能在被拒的进程上被丢下：{seen:?}"
    );
    assert_eq!(r.submitted(), ["续跑被拒也要送到"], "只提交一次");
    let screen = r.h.screen_text(25).await.unwrap();
    assert!(
        screen.contains(&format!("session {fresh} resumed=false"))
            && screen.contains("GOT: 续跑被拒也要送到"),
        "投进的是新号那个进程：{screen}"
    );
    r.stop().await;
}

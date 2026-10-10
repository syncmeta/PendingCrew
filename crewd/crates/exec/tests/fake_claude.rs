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
use crewd_exec::session::{
    AnswerOutcome, AnswerStep, BRIEF_DELIVERY_ID, DeliveryOutcome, ExitStatusKind, SessionEvent,
};
use crewd_term::DecisionEvent;
use std::os::unix::fs::PermissionsExt;
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
    let mut seen = r
        .until(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Cleared)))
        .await;
    // Raw menu controls did not establish a visible input draft. An Empty frame cannot prove
    // their lifecycle, so the brief stays Unknown until explicit visible-draft / clear evidence.
    assert!(seen.iter().any(|e| matches!(
        e,
        SessionEvent::InputDraft(Some(crewd_exec::session::HeldDraft::Unknown))
    )));
    assert!(r.submitted().is_empty());
    r.h.raw(b"x".to_vec()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !r.h.screen_text(25).await.unwrap().contains("❯ x") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    r.h.raw(vec![0x15]).unwrap();
    seen.extend(
        r.until(outcome(BRIEF_DELIVERY_ID, |o| {
            *o == DeliveryOutcome::Delivered
        }))
        .await,
    );
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

/// T10d：经 `SessionHandle::answer` 代答信任框（↓ 回车，每一下之前会话自己回读屏幕）。指纹不对的回答一下都不按。
#[tokio::test]
async fn trust_dialog_answered_through_answer_lands_the_brief() {
    let mut r = start(&[("FAKE_CLAUDE_MODE", "trust")], Some("答完框再干活")).await;
    let seen = r
        .until(|e| matches!(e, SessionEvent::Decision(DecisionEvent::Appeared(_))))
        .await;
    let Some(SessionEvent::Decision(DecisionEvent::Appeared(d))) = seen.last() else {
        unreachable!()
    };
    assert_eq!(d.selected, Some(0), "{d:?}");
    let wrong =
        r.h.answer(
            "别的框|a|b".into(),
            vec![AnswerStep {
                bytes: b"\r".to_vec(),
                selected_before: None,
            }],
        )
        .await
        .unwrap();
    assert!(
        matches!(&wrong, AnswerOutcome::Conflict { now: Some(now) } if now.fingerprint() == d.fingerprint()),
        "{wrong:?}"
    );
    let steps = vec![
        AnswerStep {
            bytes: b"\x1b[B".to_vec(),
            selected_before: Some(0),
        },
        AnswerStep {
            bytes: b"\r".to_vec(),
            selected_before: Some(1),
        },
    ];
    let done = r.h.answer(d.fingerprint(), steps).await.unwrap();
    assert!(
        matches!(done, AnswerOutcome::Answered { cleared: true, .. }),
        "{done:?}"
    );
    r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.submitted(), ["答完框再干活"]);
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

struct SlowTrace {
    dir: tempfile::TempDir,
}

impl Drop for SlowTrace {
    fn drop(&mut self) {
        let path = self.dir.path().join("read-boundaries.log");
        eprintln!("SLOW-TRACE-BEGIN {}", path.display());
        match std::fs::read_to_string(&path) {
            Ok(trace) => eprint!("{trace}"),
            Err(error) => eprintln!("trace unavailable: {error}"),
        }
        eprintln!("SLOW-TRACE-END");
    }
}

/// A pending large paste can keep changing rows beyond the submit gap. Those echo
/// frames must not be mistaken for an actual submission. This deliberately slow
/// consumer may exhaust the unchanged delivery budget on some read boundaries.
#[tokio::test]
async fn slow_chunk_echo_never_reports_unconfirmed_delivery() {
    let brief: String = (0..2000)
        .map(|i| format!("{i:05} 白板条目，带点中文\n"))
        .collect();
    assert!(brief.len() > 60_000);
    // 68000 bytes require at least 17 chunks (reads may be smaller/more).
    // submit_gap remains 300ms and verify_window duration remains 800ms;
    // these duration values are not enlarged by this fixture.
    // Declared before EchoRun: its dump follows exact-child cleanup and occurs
    // before its independent TempDir is removed, including assertion/timeout panic.
    let trace = SlowTrace {
        dir: tempfile::tempdir().unwrap(),
    };
    let trace_path = trace.dir.path().join("read-boundaries.log");
    let trace_path = trace_path.to_str().unwrap();
    let r = start(
        &[
            ("FAKE_CLAUDE_CHUNK_DELAY_MS", "50"),
            ("FAKE_CLAUDE_TRACE", trace_path),
        ],
        Some(&brief),
    )
    .await;
    let pid = r.h.status().await.unwrap().pid;
    let mut r = EchoRun {
        r,
        pid,
        stopped: false,
    };
    let seen =
        r.r.until(outcome(BRIEF_DELIVERY_ID, |o| {
            matches!(
                o,
                DeliveryOutcome::Delivered | DeliveryOutcome::Unsubmitted(_)
            )
        }))
        .await;
    match seen.last().expect("terminal delivery event") {
        SessionEvent::Delivery {
            outcome: DeliveryOutcome::Delivered,
            ..
        } => {
            assert_eq!(
                r.r.submitted(),
                [brief.replace('\n', "\\n")],
                "Delivered requires one complete actual submission"
            );
        }
        SessionEvent::Delivery {
            outcome: DeliveryOutcome::Unsubmitted(_),
            ..
        } => {
            assert!(
                !seen
                    .iter()
                    .chain(r.events().iter())
                    .any(outcome(BRIEF_DELIVERY_ID, |o| {
                        *o == DeliveryOutcome::Delivered
                    })),
                "budget exhaustion must not report a confirmed delivery"
            );
            // Child may still consume previously queued controls. Unsubmitted is
            // an honest unconfirmed outcome, not proof of zero eventual submits.
        }
        unexpected => panic!("unexpected terminal event: {unexpected:?}"),
    }
    r.stop().await;
}

/// A smaller per-chunk echo delay retains the liveness contract: actual complete
/// submission once and Delivered, without enlarging any delivery timing duration.
#[tokio::test]
async fn huge_brief_with_delayed_chunk_echo_is_submitted_exactly_once() {
    let brief: String = (0..2000)
        .map(|i| format!("{i:05} 白板条目，带点中文\n"))
        .collect();
    assert!(brief.len() > 60_000);
    let r = start(&[("FAKE_CLAUDE_CHUNK_DELAY_MS", "10")], Some(&brief)).await;
    let pid = r.h.status().await.unwrap().pid;
    let mut r = EchoRun {
        r,
        pid,
        stopped: false,
    };
    r.r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.r.submitted(), [brief.replace('\n', "\\n")]);
    r.stop().await;
}

/// A real submit key can precede a late nonempty paste redraw. Success needs
/// fresh empty input evidence, then the actual complete single submission log.
#[tokio::test]
async fn late_nonempty_echo_cannot_acknowledge_a_submit_before_commit() {
    let gate = tempfile::tempdir().unwrap();
    let gate_path = gate.path().to_str().unwrap();
    let brief: String = (0..2000)
        .map(|i| format!("{i:05} 白板条目，带点中文\n"))
        .collect();
    let r = start(
        &[
            ("FAKE_CLAUDE_MODE", "late-submit-echo"),
            ("FAKE_CLAUDE_SUBMIT_GATE", gate_path),
        ],
        Some(&brief),
    )
    .await;
    let pid = r.h.status().await.unwrap().pid;
    let mut r = EchoRun {
        r,
        pid,
        stopped: false,
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while !gate.path().join("held-cr").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // screen_text asks the actual driver: this is not just a child receipt.
        while !r
            .r
            .h
            .screen_text(25)
            .await
            .unwrap()
            .contains("+2001 lines]")
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("real CR and delayed nonempty paste frame must be observed");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let held_events = r.events();
    assert!(
        !held_events.iter().any(outcome(BRIEF_DELIVERY_ID, |o| {
            *o == DeliveryOutcome::Delivered
        })),
        "late nonempty echo must not acknowledge the held CR: {held_events:?}"
    );
    assert!(
        r.r.submitted().is_empty(),
        "gate precedes actual submission"
    );
    std::fs::write(gate.path().join("release-submit"), b"go").unwrap();
    r.r.until(outcome(BRIEF_DELIVERY_ID, |o| {
        *o == DeliveryOutcome::Delivered
    }))
    .await;
    assert_eq!(r.r.submitted(), [brief.replace('\n', "\\n")]);
    r.stop().await;
}

// T10d R3: production SessionHandle/start_claude + real PTY, with a synthetic
// child acknowledging input consumption independently of its deliberately delayed echo.
struct EchoRun {
    r: Run,
    pid: u32,
    stopped: bool,
}

impl Drop for EchoRun {
    fn drop(&mut self) {
        if !self.stopped {
            // Panic/timeout cleanup addresses only this fixture's exact child PID.
            let _ = self.r.h.stop();
            if let Some(pid) = rustix::process::Pid::from_raw(self.pid as i32) {
                let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
            }
        }
    }
}

impl EchoRun {
    fn path(&self, name: &str) -> PathBuf {
        self.r._dir.path().join(name)
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.path(name)).unwrap_or_default()
    }

    async fn receipt(&self, name: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.path(name).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("child did not acknowledge {name}"));
    }

    async fn screen_has(&self, text: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self.r.h.screen_text(25).await.unwrap().contains(text) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("driver did not observe {text}"));
    }

    fn release(&self, name: &str) {
        std::fs::write(self.path(name), b"go").unwrap();
    }

    fn events(&mut self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.r.rx.try_recv() {
            events.push(event);
        }
        events
    }

    async fn stop(&mut self) {
        self.r.h.stop().unwrap();
        self.r.until(|e| matches!(e, SessionEvent::Exited(_))).await;
        self.stopped = true;
    }
}

enum EchoMode {
    Delayed,
    PauseBody,
    TimerRecovery,
}

async fn delayed_echo_child(mode: EchoMode) -> EchoRun {
    // Synthetic child source stays in Cargo's test artifact root, not the source tree.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let py = dir.path().join("synthetic_tui.py");
    std::fs::write(
        &py,
        r#"import os, pathlib, select, sys, termios, time
root = pathlib.Path(__file__).parent
pause_body = sys.argv[1] == 'pause'
hold_clear_echo = sys.argv[1] == 'timer'
clear_echoed = False
attrs = termios.tcgetattr(0)
attrs[0] &= ~termios.ICRNL
attrs[3] &= ~(termios.ICANON | termios.ECHO)
attrs[6][termios.VMIN] = 1
attrs[6][termios.VTIME] = 0
termios.tcsetattr(0, termios.TCSANOW, attrs)
value = b''
received = bytearray()
echoed = False
paused = False
def draw():
    body = value.decode(errors='replace') if value else '\x1b[2mTry x\x1b[22m'
    os.write(1, ('\x1b[2J\x1b[HClaude Code v9.9.9\r\n\r\n────────────────────\r\n❯ ' + body + '\r\n────────────────────\r\n\x1b[2m ? for shortcuts\x1b[22m').encode())
draw()
while True:
    if not pause_body and not echoed and (root / 'show-echo').exists():
        echoed = True
        draw()
        (root / 'echo-shown').write_bytes(value)
    if hold_clear_echo and not clear_echoed and (root / 'show-clear').exists():
        clear_echoed = True
        draw()
        (root / 'clear-shown').write_bytes(b'yes')
    ready, _, _ = select.select([0], [], [], 0.01)
    if not ready:
        continue
    chunk = os.read(0, 4096)
    received.extend(chunk)
    # Append-only evidence: no partial-file read races with the parent.
    with (root / 'received').open('ab') as f:
        f.write(chunk)
    suppress_draw = False
    for key in chunk:
        if key == 127:
            value = value[:-1]
        elif key == 21:
            value = b''
            (root / 'cleared').write_bytes(b'yes')
            suppress_draw = hold_clear_echo
        elif key == 13:
            with (root / 'submitted.log').open('ab') as f:
                f.write(value + b'\n')
            value = b''
        else:
            value += bytes([key])
    if pause_body and not paused:
        paused = True
        (root / 'body-prefix').write_bytes(chunk)
        while not (root / 'resume-reading').exists():
            time.sleep(0.01)
    if pause_body and b'x' in chunk:
        (root / 'raw-consumed').write_bytes(b'x')
    if not pause_body and not (root / 'human-consumed').exists():
        (root / 'human-consumed').write_bytes(chunk)
    if echoed and not suppress_draw:
        draw()
"#,
    )
    .unwrap();
    let program = dir.path().join("synthetic-tui");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nexec /usr/bin/python3 -I -u '{}' {}\n",
            py.display(),
            match mode {
                EchoMode::Delayed => "echo",
                EchoMode::PauseBody => "pause",
                EchoMode::TimerRecovery => "timer",
            }
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Default delivery, tick, submit and stop timings, not the shortened Run helper.
    let config = DriverConfig::new(dir.path().join("run"));
    let (h, rx) = start_claude(
        ClaudeStart {
            session_id: next_session_id(),
            program,
            child_path: "/usr/bin:/bin".into(),
            spec: ClaudeSpec {
                workdir: dir.path().to_path_buf(),
                ..ClaudeSpec::default()
            },
            ledger: Arc::new(MemoryLedger::default()),
        },
        config,
    )
    .await
    .unwrap();
    let mut r = Run {
        h,
        rx,
        log: dir.path().join("submitted.log"),
        _dir: dir,
    };
    let spawned = r.until(|e| matches!(e, SessionEvent::Spawned { .. })).await;
    let pid = match spawned.last().unwrap() {
        SessionEvent::Spawned { pid, .. } => *pid,
        _ => unreachable!(),
    };
    let r = EchoRun {
        r,
        pid,
        stopped: false,
    };
    r.screen_has("Try x").await;
    r
}

async fn assert_delayed_echo_hold(queue_first: bool) {
    use crewd_exec::session::{HeldDraft, INPUT_DRAFT};
    let mut r = delayed_echo_child(EchoMode::Delayed).await;
    let id = if queue_first {
        Some(r.r.h.send("BOT").unwrap())
    } else {
        None
    };
    r.r.h.raw(b"x".to_vec()).unwrap();
    r.receipt("human-consumed").await;
    assert_eq!(
        r.bytes("human-consumed"),
        b"x",
        "child consumed raw before enqueue"
    );
    assert!(r.r.h.screen_text(25).await.unwrap().contains("Try x"));
    let id = id.unwrap_or_else(|| r.r.h.send("BOT").unwrap());
    // More than two normal ticks and the entire normal body/submit timing.
    tokio::time::sleep(Duration::from_millis(1400)).await;
    let held_bytes = r.bytes("received");
    let held_submitted = r.r.submitted();
    let held_events = r.events();
    let safe = held_bytes == b"x"
        && held_submitted.is_empty()
        && !held_events
            .iter()
            .any(outcome(id, |o| *o == DeliveryOutcome::Delivered));
    if !safe {
        r.stop().await;
        panic!(
            "raw pending echo must hold all body/submit writes: received={held_bytes:?}, submitted={held_submitted:?}, events={held_events:?}"
        );
    }
    assert!(
        held_events.contains(&SessionEvent::InputDraft(Some(HeldDraft::Unknown))),
        "{held_events:?}"
    );
    assert!(
        held_events.iter().any(outcome(
            id,
            |o| matches!(o, DeliveryOutcome::Blocked(d) if d == INPUT_DRAFT)
        )),
        "{held_events:?}"
    );
    r.release("show-echo");
    r.receipt("echo-shown").await;
    r.screen_has("❯ x").await;
    let shown_events = r.events();
    assert!(
        shown_events.contains(&SessionEvent::InputDraft(Some(HeldDraft::Text("x".into())))),
        "{shown_events:?}"
    );
    r.r.h.raw(vec![0x15]).unwrap();
    r.receipt("cleared").await;
    let done =
        r.r.until(outcome(id, |o| *o == DeliveryOutcome::Delivered))
            .await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let later = r.events();
    let final_bytes = r.bytes("received");
    let submitted = r.r.submitted();
    r.stop().await;
    assert_eq!(
        submitted,
        ["BOT"],
        "human draft must never be submitted with BOT"
    );
    assert_eq!(
        final_bytes, b"x\x15BOT \x7f\r",
        "separate, exactly one body/submit"
    );
    assert_eq!(
        done.iter()
            .chain(&later)
            .filter(|e| outcome(id, |o| *o == DeliveryOutcome::Delivered)(e))
            .count(),
        1
    );
}

#[tokio::test]
async fn raw_consumed_before_echo_holds_the_production_delivery() {
    assert_delayed_echo_hold(false).await;
}

#[tokio::test]
async fn raw_after_enqueue_before_ready_holds_the_production_delivery() {
    assert_delayed_echo_hold(true).await;
}

/// Actual production SessionHandle/run()/PTY, default timings, synthetic child only.
/// Suspend this current-thread runtime while the child writes its sole clear echo:
/// both read and 600ms tick are ready when select resumes. This samples select order;
/// deterministic timer-call-path coverage lives in driver::output_drain_tests.
#[tokio::test(flavor = "current_thread")]
async fn timer_and_read_ready_together_recover_after_one_clear_echo_without_repaint() {
    use crewd_exec::session::HeldDraft;
    for sample in 0..8 {
        let mut r = delayed_echo_child(EchoMode::TimerRecovery).await;
        r.r.h.raw(b"x".to_vec()).unwrap();
        let id = r.r.h.send("BOT").unwrap();
        r.receipt("human-consumed").await;
        assert_eq!(r.bytes("received"), b"x");
        r.release("show-echo");
        // Four samples also make Text and a timer simultaneously ready.
        let text_co_ready = sample % 2 == 0;
        if text_co_ready {
            std::thread::sleep(Duration::from_millis(700));
            assert!(
                r.path("echo-shown").exists(),
                "child drew Text while paused"
            );
        }
        r.r.until(|e| matches!(e, SessionEvent::InputDraft(Some(HeldDraft::Text(t))) if t == "x"))
            .await;
        assert_eq!(r.bytes("received"), b"x", "BOT still held by human Text");
        r.r.h.raw(vec![0x15]).unwrap();
        r.receipt("cleared").await;
        assert_eq!(
            r.bytes("received"),
            b"x\x15",
            "child consumed explicit clear"
        );
        r.r.h.status().await.unwrap(); // FIFO barrier: raw handled, old Text still no evidence.
        assert!(r.r.h.screen_text(25).await.unwrap().contains("❯ x"));
        r.events();
        r.release("show-clear");
        std::thread::sleep(Duration::from_millis(700));
        assert!(
            r.path("clear-shown").exists(),
            "child drew fresh Empty while paused"
        );
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let events = r.events();
        let bytes = r.bytes("received");
        let submitted = r.r.submitted();
        let delivered = events
            .iter()
            .filter(|e| outcome(id, |o| *o == DeliveryOutcome::Delivered)(e))
            .count();
        println!(
            "sample={sample} text_co_ready={text_co_ready} clear_consumed=true clear_drawn_before_resume=true received={bytes:?} submitted={submitted:?} delivered={delivered} events={events:?}"
        );
        assert!(
            events.contains(&SessionEvent::InputDraft(None)),
            "fresh Empty must release the hold without a second clear repaint: {events:?}"
        );
        assert_eq!(delivered, 1, "one real Delivered outcome");
        assert_eq!(submitted, ["BOT"], "no human draft or duplicate submission");
        assert_eq!(
            bytes, b"x\x15BOT \x7f\r",
            "one body and separate submit beats"
        );
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(r.bytes("received"), bytes, "no late retry or cleanup");
        assert_eq!(r.r.submitted(), submitted);
        assert!(
            !r.events()
                .iter()
                .any(outcome(id, |o| *o == DeliveryOutcome::Delivered)),
            "no second Delivered event"
        );
        r.stop().await;
    }
    println!(
        "full_loop_samples=8 text_co_ready_samples=4 clear_co_ready_samples=8 recovered_without_extra_repaint=8 select_branch=not_instrumented"
    );
}

#[tokio::test]
async fn raw_during_a_backpressured_body_cancels_the_remaining_bytes_without_cleanup() {
    let mut r = delayed_echo_child(EchoMode::PauseBody).await;
    let body = "B".repeat(1_000_000);
    let id = r.r.h.send(body.clone()).unwrap();
    r.receipt("body-prefix").await;
    let prefix = r.bytes("body-prefix");
    assert!(!prefix.is_empty() && prefix.iter().all(|b| *b == b'B'));
    r.r.h.raw(b"x".to_vec()).unwrap();
    // FIFO barrier proves the raw command was handled while the child still wasn't reading.
    r.r.h.status().await.unwrap();
    r.release("resume-reading");
    r.receipt("raw-consumed").await;
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let bytes = r.bytes("received");
    let events = r.events();
    let submitted = r.r.submitted();
    r.stop().await;
    assert!(
        bytes.len() < body.len(),
        "partial body must not finish after raw"
    );
    assert_eq!(
        bytes.last(),
        Some(&b'x'),
        "raw must follow only the already-written prefix"
    );
    assert!(
        bytes[..bytes.len() - 1].iter().all(|b| *b == b'B'),
        "no Ctrl-U, submit, or retry: {bytes:?}"
    );
    assert!(submitted.is_empty());
    assert!(
        events.iter().any(outcome(id, |o| matches!(
            o,
            DeliveryOutcome::Unsubmitted(_)
        ))),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(outcome(id, |o| *o == DeliveryOutcome::Delivered)),
        "{events:?}"
    );
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

//! 真 claude 冒烟：只验「起得来、终端查询答得对（DA1 / OSC 11…）、画出输入框」，
//! **不发任何 prompt**（不花额度、不生成会话日志）。
//!
//! 两道闸，缺一不跑：`#[ignore]`（`check.sh` 不带 `--ignored`，永远不跑它）+
//! 环境变量 `CREWD_LIVE_CLAUDE=1`。工作目录取 `CREWD_LIVE_CLAUDE_WORKDIR`，缺省是本仓库根
//! （人平时就在这里用 claude，已信任，不会弹信任框；弹了也照样报出来，算失败）。
//!
//! 跑法：`CREWD_LIVE_CLAUDE=1 cargo test -p crewd-exec --test live_claude -- --ignored --nocapture`
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::AgentPaths;
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::SessionEvent;
use crewd_term::{blocking_dialog, input_row};

#[tokio::test]
#[ignore = "真 claude：CREWD_LIVE_CLAUDE=1 cargo test -p crewd-exec --test live_claude -- --ignored"]
async fn real_claude_starts_up_to_the_input_box() {
    if std::env::var("CREWD_LIVE_CLAUDE").as_deref() != Ok("1") {
        eprintln!("跳过：没设 CREWD_LIVE_CLAUDE=1");
        return;
    }
    let workdir = std::env::var_os("CREWD_LIVE_CLAUDE_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../..")));
    let paths = AgentPaths::discover().await;
    let program = paths.resolve("claude").expect("PATH 里没有 claude");
    let runtime = tempfile::tempdir().unwrap();
    let start = ClaudeStart {
        session_id: "live".into(),
        program,
        child_path: paths.child_path.clone(),
        spec: ClaudeSpec {
            workdir,
            ..ClaudeSpec::default()
        },
        ledger: Arc::new(MemoryLedger::default()),
    };
    let started = Instant::now();
    let (h, mut rx) = start_claude(start, DriverConfig::new(runtime.path().to_path_buf()))
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = String::new();
    // 循环里不 panic：先停掉 claude、等它退出，再断言（panic 会直接拆掉运行时，
    // 子进程没人收尸、留成孤儿）。
    let outcome: Result<(), &str> = 'wait: loop {
        if Instant::now() > deadline {
            break Err("60s 内没画出输入框（DA1 / OSC 回答不被接受时会卡在首屏）");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(e) = rx.try_recv() {
            eprintln!("{e:?}");
            if matches!(e, SessionEvent::Exited(_)) {
                break 'wait Err("claude 起来就退了");
            }
        }
        let Ok(text) = h.screen_text(50).await else {
            break Err("会话任务提前结束");
        };
        last = text;
        let rows: Vec<String> = last.lines().map(str::to_string).collect();
        if blocking_dialog(&rows).is_some() {
            break Err("弹了对话框（目录没信任？）");
        }
        if let Some(row) = input_row(&rows) {
            eprintln!("输入框（{}ms）：{row:?}", started.elapsed().as_millis());
            break Ok(());
        }
    };
    eprintln!("----- 最后一屏 -----\n{last}");
    let _ = h.stop();
    while let Some(e) = rx.recv().await {
        eprintln!("{e:?}");
        if matches!(e, SessionEvent::Exited(_)) {
            break;
        }
    }
    if let Err(why) = outcome {
        panic!("{why}");
    }
}

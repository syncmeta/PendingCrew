//! 真 claude 冒烟：只验「起得来、终端查询答得对（DA1 / OSC 11…）、画出输入框」，
//! **不发任何 prompt**（不花额度、不生成会话日志）。
//!
//! 两道闸，缺一不跑：`#[ignore]`（`check.sh` 不带 `--ignored`，永远不跑它）+
//! 环境变量 `CREWD_LIVE_CLAUDE=1`。带了 `--ignored` 却没设环境变量 = 直接失败并说明原因
//! （不静默「通过」：那样看不出它根本没跑）。
//!
//! 工作目录：缺省是 `$HOME` 下现建的临时空目录（跑完删掉）——claude 继承上级目录的信任，
//! 而 `$HOME` 通常已被信任，所以不会弹信任框；放在 `/tmp` 下没有受信任的祖先目录，会弹框。
//! 用 `CREWD_LIVE_CLAUDE_WORKDIR` 指定别的目录（必须是 claude 已信任的目录，测试不删它）。
//! 副作用：claude 会在 `~/.claude.json` 里给那个路径记一条 project 记录。
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
    assert!(
        std::env::var("CREWD_LIVE_CLAUDE").as_deref() == Ok("1"),
        "这条测试会起真 claude：带了 --ignored 还得设 CREWD_LIVE_CLAUDE=1 才跑（第二道闸）。\
         没设就算失败，免得一个根本没跑的「通过」被当成冒烟过了。"
    );
    // 缺省目录活到测试结束（drop 时删掉）；指定了目录就用它、不删。
    let (workdir, _temp): (PathBuf, Option<tempfile::TempDir>) =
        match std::env::var_os("CREWD_LIVE_CLAUDE_WORKDIR") {
            Some(dir) => (PathBuf::from(dir), None),
            None => {
                let home = std::env::var_os("HOME")
                    .expect("没有 HOME：用 CREWD_LIVE_CLAUDE_WORKDIR 指定一个已信任的目录");
                let temp = tempfile::Builder::new()
                    .prefix("crewd-live-claude-")
                    .tempdir_in(home)
                    .expect("在 $HOME 下建不了临时目录");
                (temp.path().to_path_buf(), Some(temp))
            }
        };
    let paths = AgentPaths::discover().await;
    let program = paths.resolve("claude").expect("PATH 里没有 claude");
    let runtime = tempfile::tempdir().unwrap();
    let start = ClaudeStart {
        session_id: "live".into(),
        program,
        child_path: paths.child_path.clone(),
        spec: ClaudeSpec {
            workdir: workdir.clone(),
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
    let outcome: Result<(), String> = 'wait: loop {
        if Instant::now() > deadline {
            break Err("60s 内没画出输入框（DA1 / OSC 回答不被接受时会卡在首屏）".to_string());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(e) = rx.try_recv() {
            eprintln!("{e:?}");
            if matches!(e, SessionEvent::Exited(_)) {
                break 'wait Err("claude 起来就退了".to_string());
            }
        }
        let Ok(text) = h.screen_text(50).await else {
            break Err("会话任务提前结束".to_string());
        };
        last = text;
        let rows: Vec<String> = last.lines().map(str::to_string).collect();
        if blocking_dialog(&rows).is_some() {
            break Err(format!(
                "弹了对话框（多半是信任框：{} 不在 claude 已信任的目录下）。\
                 把 CREWD_LIVE_CLAUDE_WORKDIR 设成一个已信任的目录再跑。",
                workdir.display()
            ));
        }
        if let Some(row) = input_row(&rows) {
            eprintln!("输入框（{}ms）：{row:?}", started.elapsed().as_millis());
            break Ok(());
        }
    };
    eprintln!("----- 最后一屏 -----\n{last}");
    let _ = h.stop();
    // 等退出也有上限：stop 之后 grace（2s）到点会 killpg，10s 还没有 Exited 就是驱动层的毛病。
    let exited = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = rx.recv().await {
            eprintln!("{e:?}");
            if matches!(e, SessionEvent::Exited(_)) {
                return true;
            }
        }
        false
    })
    .await;
    if let Err(why) = outcome {
        panic!("{why}");
    }
    assert!(
        matches!(exited, Ok(true)),
        "stop 之后 10s 内没等到 Exited：{exited:?}"
    );
}

//! 真 codex 冒烟。两道闸，缺一不跑：`#[ignore]`（`check.sh` 不带 `--ignored`，永远不跑它）+
//! 环境变量 `CREWD_LIVE_CODEX`。带了 `--ignored` 却没设（或设得不对）= 直接失败并说明原因
//! （不静默「通过」：那样看不出它根本没跑）。人已同意跑这两条（2026-10-06，回合那条会花额度）。
//!
//! - `CREWD_LIVE_CODEX=1`：只握手——initialize、thread/start、拿到 thread id，**不开回合**
//!   （不花额度）。验的是 schema 上核不到的那些：0.160 的 app-server 认不认我们的
//!   `initialize`（不带 params 的 `initialized`、`optOutNotificationMethods`）、`thread/start`
//!   带 `config.mcp_servers`（指向 `/usr/bin/true`：MCP 起不来，推测不挡握手，未核实）与
//!   `developerInstructions`。
//!   注意：thread/start 可能在 `$CODEX_HOME/sessions`（缺省 `~/.codex/sessions`）下留一条空
//!   thread 的记录（未核实）。
//! - `CREWD_LIVE_CODEX=turn`：两条都跑；回合那条再跑一个最小回合（花几百个 token）：白板经
//!   `additionalContext`（`kind: "untrusted"`）送进去，问它暗号——验 Swift 一直没真机验过的
//!   「additionalContext 真的被模型看见」。
//!
//! 工作目录取 `CREWD_LIVE_CODEX_WORKDIR`，缺省是一个现建、跑完即删的临时目录。
//! 等事件的循环里不 panic：先停掉 codex、等它退出（有上限），再断言——panic 会直接拆掉运行时，
//! codex 起的子进程没人收、留成孤儿。
//! 跑法（`=1` 时要带名字只跑握手那条，否则回合那条按第二道闸算失败）：
//! - `CREWD_LIVE_CODEX=1 cargo test -p crewd-exec --test live_codex live_codex_handshakes -- --ignored --nocapture`
//! - `CREWD_LIVE_CODEX=turn cargo test -p crewd-exec --test live_codex -- --ignored --nocapture`
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
use crewd_exec::driver::SessionHandle;
use crewd_exec::env::AgentPaths;
use crewd_exec::launch::CommsSpec;
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::{DerivedState, SessionEvent};
use tokio::sync::mpsc::UnboundedReceiver;

/// 第二道闸。`want_turn` = 这条要花额度，只认 `turn`。
fn gate(want_turn: bool) {
    let v = std::env::var("CREWD_LIVE_CODEX").unwrap_or_default();
    let ok = if want_turn {
        v == "turn"
    } else {
        v == "1" || v == "turn"
    };
    assert!(
        ok,
        "这条测试会起真 codex：带了 --ignored 还得设 CREWD_LIVE_CODEX={}（第二道闸；现在是 {v:?}）。\
         没设就算失败，免得一个根本没跑的「通过」被当成冒烟过了。",
        if want_turn { "turn" } else { "1 或 turn" }
    );
}

async fn start(
    brief: Option<&str>,
) -> (
    SessionHandle,
    UnboundedReceiver<SessionEvent>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = std::env::var_os("CREWD_LIVE_CODEX_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| tmp.path().to_path_buf());
    let paths = AgentPaths::discover().await;
    let program = paths
        .resolve("codex")
        .expect("登录 shell 的 PATH 里找不到 codex");
    eprintln!("codex = {}", program.display());
    let start = CodexStart {
        session_id: "live".into(),
        program,
        child_path: paths.child_path,
        spec: CodexSpec {
            workdir,
            developer_instructions: Some("你在一次自动化冒烟测试里。回答尽量短。".into()),
            initial_prompt: brief.map(str::to_string),
            initial_context: [(
                "crew_whiteboard".to_string(),
                "测试白板：今天的暗号是「青蛙七号」。".to_string(),
            )]
            .into_iter()
            .collect(),
            comms: Some(CommsSpec {
                helper: PathBuf::from("/usr/bin/true"),
                data_dir: tmp.path().to_path_buf(),
                crew_id: "live".into(),
                session_id: "live".into(),
                captain: false,
                structured_post: false,
                label: None,
                hooks: false,
            }),
            ..CodexSpec::default()
        },
        ledger: Arc::new(MemoryLedger::default()),
    };
    let (h, rx) = start_codex(start, CodexConfig::default())
        .await
        .expect("真 codex 握手失败");
    (h, rx, tmp)
}

/// 停掉并等 `Exited`（最多 10s：stop 之后 2s 宽限到点会 killpg，10s 还没有就是驱动层的毛病）。
async fn stop_and_wait(h: &SessionHandle, rx: &mut UnboundedReceiver<SessionEvent>) -> bool {
    let _ = h.stop();
    let exited = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(e) = rx.recv().await {
            eprintln!("{e:?}");
            if let SessionEvent::Exited(info) = e {
                return info.stopped_by_user;
            }
        }
        false
    })
    .await;
    matches!(exited, Ok(true))
}

#[tokio::test]
#[ignore = "真 codex：CREWD_LIVE_CODEX=1 cargo test -p crewd-exec --test live_codex live_codex_handshakes -- --ignored"]
async fn live_codex_handshakes_and_opens_a_thread_without_a_turn() {
    gate(false);
    let (h, mut rx, _tmp) = start(None).await;
    let first = rx.recv().await;
    eprintln!("{first:?}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    while let Ok(e) = rx.try_recv() {
        eprintln!("{e:?}");
    }
    let status = h.status().await;
    eprintln!("{status:?}");
    let stopped = stop_and_wait(&h, &mut rx).await;
    let Some(SessionEvent::Spawned {
        agent_session_id,
        resumed: false,
        ..
    }) = first
    else {
        panic!("第一条应是 Spawned：{first:?}");
    };
    assert!(!agent_session_id.is_empty());
    assert_eq!(status.unwrap().state, DerivedState::Idle);
    assert!(stopped, "stop 之后 10s 内没等到 stopped_by_user 的 Exited");
}

#[tokio::test]
#[ignore = "真 codex 回合（花额度）：CREWD_LIVE_CODEX=turn cargo test -p crewd-exec --test live_codex -- --ignored"]
async fn live_codex_turn_sees_the_untrusted_whiteboard_context() {
    gate(true);
    let (h, mut rx, _tmp) = start(Some("白板里的暗号是什么？只回答暗号本身。")).await;
    let answer = tokio::time::timeout(Duration::from_secs(180), async {
        while let Some(e) = rx.recv().await {
            eprintln!("{e:?}");
            if let SessionEvent::TurnEnded { last_agent_message } = e {
                return Some(last_agent_message);
            }
        }
        None
    })
    .await;
    let stopped = stop_and_wait(&h, &mut rx).await;
    let answer = match answer {
        Ok(Some(a)) => a,
        Ok(None) => panic!("事件流结束前没有 TurnEnded"),
        Err(_) => panic!("180s 内回合没结束"),
    };
    assert!(
        answer.contains("青蛙七号"),
        "模型没看到 additionalContext：{answer}"
    );
    assert!(stopped, "stop 之后 10s 内没等到 stopped_by_user 的 Exited");
}

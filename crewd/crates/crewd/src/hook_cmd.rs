//! `crewd hook <event>`：claude 的 hook（`sh -c` 起）。读 stdin JSON → `agent.hook` → 原样打印 stdout。
//! 永远 exit 0；绝不打印坏 JSON。连不上 / 超时：post-tool-use、stop 什么都不打（宁可漏一次）；
//! pre-tool-use 打 deny（它只在命中审批闸时才被调用，见 crewd-exec `gate_matcher`）。
use std::path::{Path, PathBuf};
use std::time::Duration;

use crewd_proto::agent::{HookKind, HookResult, methods};
use serde_json::{Value, json};
use std::io::Read;

use crate::agent_client::AgentClient;

/// 三种 hook 的总期限（读 stdin + 连 + attach + 调用）。claude 侧的超时是 15s（crewd-exec）。
const QUICK: Duration = Duration::from_secs(5);

#[derive(clap::Args, Clone)]
pub struct HookArgs {
    /// post-tool-use | pre-tool-use | stop
    event: String,
    #[arg(long)]
    session: String,
    #[arg(long)]
    token_file: PathBuf,
}

fn deny(reason: &str) -> String {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }})
    .to_string()
}

pub async fn run(data_dir: &Path, args: HookArgs) -> anyhow::Result<()> {
    // 三种 hook 同一个总期限（读 stdin + 连 + attach + 调用）：审批不阻塞（控制者裁定 C2），
    // pre-tool-use 也只是一次本机往返；它必须短于 claude 侧的 hook 超时（15s），否则 claude 的超时
    // 会替我们「放行」。读 stdin 也算在里面：stdin 一直不关，也不能拖过期限。
    let deadline = QUICK;
    let until = tokio::time::Instant::now() + deadline;
    // 先把 stdin 读完（不认识的事件也读）：claude 写完才关 stdin，提前退出会让它写到断管。
    // 在自己的线程里读：tokio 的 stdin 走 blocking 池，runtime 退出时会等它——stdin 不关就退不掉。
    // 这条线程不 join，进程退出时随之结束。
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut raw = Vec::new();
        let r = std::io::stdin().read_to_end(&mut raw).map(|_| raw);
        let _ = tx.send(r);
    });
    let read = tokio::time::timeout_at(until, rx).await;
    let Some(kind) = HookKind::parse(&args.event) else {
        eprintln!("crewd hook: unknown event {:?}, ignored", args.event);
        return Ok(());
    };
    let raw = match read {
        Ok(Ok(Ok(raw))) => raw,
        Ok(Ok(Err(e))) => {
            eprintln!("crewd hook {}: cannot read stdin: {e}", kind.as_str());
            Vec::new()
        }
        Ok(Err(_)) => unreachable!("the stdin thread always sends before it ends"),
        Err(_) => {
            eprintln!(
                "crewd hook {}: stdin not closed within {deadline:?}",
                kind.as_str()
            );
            if kind == HookKind::PreToolUse {
                println!("{}", deny("no approval decision in time; denied"));
            }
            return Ok(());
        }
    };
    let payload: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("crewd hook {}: stdin is not JSON: {e}", kind.as_str());
            if kind == HookKind::PreToolUse {
                println!(
                    "{}",
                    deny("crewd could not read this tool call; denied to be safe")
                );
            }
            return Ok(());
        }
    };
    let work = async {
        let (client, _) = AgentClient::connect(data_dir, &args.session, &args.token_file).await?;
        client
            .call::<HookResult>(methods::HOOK, json!({"event": kind, "payload": payload}))
            .await
    };
    match tokio::time::timeout_at(until, work).await {
        Ok(Ok(HookResult { stdout: Some(s) })) => println!("{s}"),
        Ok(Ok(HookResult { stdout: None })) => {}
        Ok(Err(e)) => {
            eprintln!("crewd hook {}: {e:#}", kind.as_str());
            if kind == HookKind::PreToolUse {
                println!(
                    "{}",
                    deny("crewd is not reachable; this gated tool needs approval, denied")
                );
            }
        }
        Err(_) => {
            eprintln!(
                "crewd hook {}: no answer within {deadline:?}",
                kind.as_str()
            );
            if kind == HookKind::PreToolUse {
                println!("{}", deny("no approval decision in time; denied"));
            }
        }
    }
    Ok(())
}

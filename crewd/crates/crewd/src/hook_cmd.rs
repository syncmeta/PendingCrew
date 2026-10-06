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

impl HookArgs {
    pub fn event(&self) -> &str {
        &self.event
    }
}

fn deny(reason: &str) -> String {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }})
    .to_string()
}

/// hook 出了任何岔子（命令行错、panic、`run` 返回 Err）时的唯一出口动作：pre-tool-use 打 deny，
/// 其余什么都不打。写 stdout 失败也不再 panic（这条路本身可能就是从 stdout 断管的 panic 来的）。
pub fn fail_closed(event: Option<&str>) {
    if event.and_then(HookKind::parse) == Some(HookKind::PreToolUse) {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = writeln!(
            out,
            "{}",
            deny("crewd hook failed; this gated tool needs approval, denied")
        );
        let _ = out.flush();
    }
}

/// 给 hook 子命令装 panic 钩子：panic → 写 stderr、`fail_closed`、exit 0。
/// 不 exit 0 的话 panic 是 101：claude 当非阻塞错误，命中闸的工具照样放行。
pub fn install_panic_guard(event: String) {
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("crewd hook {event}: panic: {info}");
        fail_closed(Some(&event));
        std::process::exit(0);
    }));
}

/// 命令行解析失败时，从原始 argv 里找出 `hook` 子命令与它的事件名（clap 已经给不出来了）。
/// 返回 None = 不是 hook 调用；Some(None) = 是 hook 但认不出事件。
pub fn hook_event_from_argv(argv: &[std::ffi::OsString]) -> Option<Option<String>> {
    // 带值的参数（`--x=v` 形式自带值，不吃下一个）。
    const WITH_VALUE: [&str; 3] = ["--data-dir", "--session", "--token-file"];
    let mut positional = Vec::new();
    let mut it = argv.iter().skip(1).map(|a| a.to_string_lossy());
    while let Some(a) = it.next() {
        if WITH_VALUE.contains(&a.as_ref()) {
            it.next();
        } else if !a.starts_with('-') {
            positional.push(a.into_owned());
        }
    }
    let mut p = positional.into_iter();
    if p.next().as_deref() != Some("hook") {
        return None;
    }
    Some(p.next())
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
    // 测试用的注入点（只在 debug 构建里）：验证 panic 也是「pre-tool-use deny、其余不出声、exit 0」。
    #[cfg(debug_assertions)]
    if std::env::var_os("CREWD_TEST_HOOK_PANIC").is_some() {
        panic!("injected test panic (CREWD_TEST_HOOK_PANIC)");
    }
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
        Ok::<_, anyhow::Error>(
            client
                .call::<HookResult>(methods::HOOK, json!({"event": kind, "payload": payload}))
                .await?,
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<std::ffi::OsString> {
        a.iter().map(|s| s.into()).collect()
    }

    #[test]
    fn hook_event_is_found_around_flags() {
        let ev = |a: &[&str]| hook_event_from_argv(&argv(a));
        assert_eq!(
            ev(&["crewd", "hook", "pre-tool-use", "--bogus"]),
            Some(Some("pre-tool-use".into()))
        );
        assert_eq!(
            ev(&[
                "crewd",
                "--data-dir",
                "hook",
                "hook",
                "--session",
                "stop",
                "post-tool-use"
            ]),
            Some(Some("post-tool-use".into())),
            "flag values are not positionals"
        );
        assert_eq!(
            ev(&["crewd", "--data-dir=/d", "hook", "--x", "stop"]),
            Some(Some("stop".into()))
        );
        assert_eq!(ev(&["crewd", "hook"]), Some(None));
        assert_eq!(ev(&["crewd", "mcp", "--session", "s"]), None);
        assert_eq!(ev(&["crewd", "--bogus"]), None);
    }
}

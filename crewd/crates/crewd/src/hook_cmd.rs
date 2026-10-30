//! `crewd hook <event>`：claude 的 hook（`sh -c` 起）。读 stdin JSON → `agent.hook` → 原样打印 stdout。
//! Observational hooks fail open with empty stdout/exit0. The managed
//! UserPromptSubmit guard instead requires an explicit durable allow ACK;
//! every parse/connection/timeout/panic failure exits2 before prompt processing.
//! Legacy `pre-tool-use` remains ignored; managed activity observes without
//! authorizing or blocking native tools.
use std::path::{Path, PathBuf};
use std::time::Duration;

use crewd_proto::agent::{HookKind, HookResult, methods};
use serde_json::{Value, json};
use std::io::Read;

use crate::agent_client::AgentClient;

/// hook 的总期限（读 stdin + 连 + attach + 调用）。claude 侧的超时是 15s（crewd-exec）。
const QUICK: Duration = Duration::from_secs(5);

#[derive(clap::Args, Clone)]
pub struct HookArgs {
    /// Hook wire event; managed user-prompt-submit requires explicit authorization.
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

/// Observation panic exits0; a managed prompt guard panic exits2.
pub fn install_panic_guard(event: String) {
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("crewd hook {event}: panic: {info}");
        std::process::exit(if event == "user-prompt-submit" { 2 } else { 0 });
    }));
}

/// 命令行解析失败时，从原始 argv 里找出它是不是一次 `hook` 调用（clap 已经给不出来了）：
/// 是 = 调用方写 stderr 后 exit 0（exit 2 对 Stop 是「挡住」）；不是 = clap 照常处理。
pub fn is_hook_argv(argv: &[std::ffi::OsString]) -> bool {
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
    positional.first().map(String::as_str) == Some("hook")
}

pub fn guarded_hook_argv(argv: &[std::ffi::OsString]) -> bool {
    is_hook_argv(argv) && argv.iter().any(|v| v == "user-prompt-submit")
}

pub async fn run(data_dir: &Path, args: HookArgs) -> anyhow::Result<()> {
    if args.event == "user-prompt-submit" {
        return guarded_run(data_dir, args).await;
    }
    // 同一个总期限（读 stdin + 连 + attach + 调用），短于 claude 侧的 hook 超时（15s）。
    // 读 stdin 也算在里面：stdin 一直不关，也不能拖过期限。
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
    // 测试用的注入点（只在 debug 构建里）：验证 panic 也是「不出声、exit 0」。
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
            return Ok(());
        }
    };
    let payload: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("crewd hook {}: stdin is not JSON: {e}", kind.as_str());
            return Ok(());
        }
    };
    let work = async {
        let (client, who) = AgentClient::connect(data_dir, &args.session, &args.token_file).await?;
        if let Some(warning) = who.skew_warning() {
            eprintln!("crewd hook: {warning}");
        }
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
        }
        Err(_) => {
            eprintln!(
                "crewd hook {}: no answer within {deadline:?}",
                kind.as_str()
            );
        }
    }
    Ok(())
}

/// Unlike observational hooks, missing/malformed input or ACK must never allow
/// a managed prompt. The caller exits2 on every error, including parse/panic.
async fn guarded_run(data_dir: &Path, args: HookArgs) -> anyhow::Result<()> {
    #[cfg(debug_assertions)]
    if std::env::var_os("CREWD_TEST_HOOK_PANIC").is_some() {
        panic!("injected guarded test panic");
    }
    let until = tokio::time::Instant::now() + QUICK;
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut raw = Vec::new();
        let result = std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut raw)
            .map(|_| raw);
        let _ = tx.send(result);
    });
    let raw = tokio::time::timeout_at(until, rx).await???;
    anyhow::ensure!(
        raw.len() <= 8 * 1024 * 1024,
        "managed hook input exceeds bound"
    );
    let payload: Value = serde_json::from_slice(&raw)?;
    anyhow::ensure!(payload.is_object(), "managed hook input must be an object");
    let result = tokio::time::timeout_at(until, async {
        let (client, who) = AgentClient::connect(data_dir, &args.session, &args.token_file).await?;
        anyhow::ensure!(
            who.skew_warning().is_none(),
            "managed hook helper build skew"
        );
        Ok::<_, anyhow::Error>(
            client
                .call::<HookResult>(
                    methods::HOOK,
                    json!({"event":HookKind::UserPromptSubmit,"payload":payload}),
                )
                .await?,
        )
    })
    .await??;
    let stdout = result
        .stdout
        .ok_or_else(|| anyhow::anyhow!("managed hook approval missing"))?;
    let value: Value = serde_json::from_str(&stdout)?;
    anyhow::ensure!(
        value == json!({"continue":true}),
        "managed hook approval refused or malformed"
    );
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<std::ffi::OsString> {
        a.iter().map(|s| s.into()).collect()
    }

    #[test]
    fn a_hook_call_is_recognised_around_flags() {
        let is = |a: &[&str]| is_hook_argv(&argv(a));
        assert!(is(&["crewd", "hook", "pre-tool-use", "--bogus"]));
        assert!(is(&[
            "crewd",
            "--data-dir",
            "hook",
            "hook",
            "--session",
            "stop",
            "post-tool-use"
        ]));
        assert!(is(&["crewd", "--data-dir=/d", "hook", "--x", "stop"]));
        assert!(is(&["crewd", "hook"]));
        assert!(!is(&["crewd", "mcp", "--session", "s"]));
        assert!(
            !is(&["crewd", "--data-dir", "hook", "mcp"]),
            "a flag value is not the subcommand"
        );
        assert!(!is(&["crewd", "--bogus"]));
    }
}

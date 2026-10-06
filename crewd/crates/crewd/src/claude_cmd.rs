//! `crewd claude`：前台起一个 claude 会话（M2b 的验收与调试入口；M3 起由 serve 拉起会话）。
//! 事件逐行打到 stdout；stdin 每一行当一条指令投进去；Ctrl-C / SIGTERM 停掉会话。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::AgentPaths;
use crewd_exec::health::AuthProbe;
use crewd_exec::launch::ClaudeSpec;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::SessionEvent;
use crewd_server::ledger::StoreLedger;
use tokio::io::AsyncBufReadExt;

#[derive(clap::Args, Clone)]
pub struct ClaudeArgs {
    /// 工作目录
    #[arg(long)]
    workdir: PathBuf,
    /// crewd session id：给了就用库里的会话号账本（行必须已存在，重跑会 --resume）；不给只在内存里记
    #[arg(long)]
    session: Option<String>,
    /// 开场任务（走 PTY 投递，绝不进 argv）
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    /// claude 可执行文件；缺省按登录 shell 的 PATH 找
    #[arg(long)]
    claude: Option<PathBuf>,
    /// 每次转入空闲时把当前屏幕打到 stderr
    #[arg(long)]
    screen: bool,
}

pub async fn run(data_dir: &Path, args: ClaudeArgs) -> anyhow::Result<()> {
    crewd_server::ensure_data_dir(data_dir)?;
    // 与 serve / import 同一把锁：账本写库、回收孤儿都要求没有别的 crewd 活着。
    let _lock = crewd_server::lock_data_dir(data_dir)?;
    let (registry, reports) = crewd_server::reap_orphans(data_dir)?;
    crewd_server::log_reap_reports(&reports);
    // 子进程的 cwd 是 workdir：相对路径在那边会被按 workdir 解析，先在这里转成绝对路径。
    let workdir = std::fs::canonicalize(&args.workdir)
        .with_context(|| format!("工作目录 {} 不可用", args.workdir.display()))?;
    let paths = AgentPaths::discover().await;
    let program = match &args.claude {
        Some(p) => std::fs::canonicalize(p).with_context(|| format!("找不到 {}", p.display()))?,
        None => paths
            .resolve("claude")
            .context("登录 shell 的 PATH 里找不到 claude")?,
    };
    let ledger: Arc<dyn AgentIdLedger> = match &args.session {
        Some(s) => Arc::new(StoreLedger::new(data_dir.join("crewd.db"), s.clone())),
        None => Arc::new(MemoryLedger::default()),
    };
    let mut config = DriverConfig::new(data_dir.join("run"));
    config.registry = Some(registry);
    config.claude_projects_dir = AuthProbe::projects_dir_for(
        std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        &workdir,
    );
    let start = ClaudeStart {
        session_id: args.session.unwrap_or_else(|| "cli".to_string()),
        program,
        child_path: paths.child_path,
        spec: ClaudeSpec {
            workdir,
            model: args.model,
            effort: args.effort,
            initial_prompt: args.prompt,
            ..ClaudeSpec::default()
        },
        ledger,
    };
    let (handle, mut events) = start_claude(start, config).await?;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_open = true;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                println!("{ev:?}");
                if args.screen
                    && ev == SessionEvent::Working(false)
                    && let Ok(text) = handle.screen_text(50).await
                {
                    eprintln!("----- screen -----\n{text}\n------------------");
                }
                if matches!(ev, SessionEvent::Exited(_)) {
                    break;
                }
            }
            line = lines.next_line(), if stdin_open => match line {
                Ok(Some(text)) if !text.is_empty() => { let _ = handle.send(text); }
                Ok(Some(_)) => {}
                _ => stdin_open = false,
            },
            _ = tokio::signal::ctrl_c() => { let _ = handle.stop(); }
            _ = term.recv() => { let _ = handle.stop(); }
        }
    }
    Ok(())
}

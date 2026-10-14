//! `crewd codex`：前台起一个 codex 会话（M2c 的验收与调试入口；M3 起由 serve 拉起会话）。
//! 事件逐行打到 stdout；stdin 每一行当一条指令投进去（`/interrupt` 打断、`/compact` 压缩）；
//! Ctrl-C / SIGTERM 停掉会话。开场任务只从文件或 stdin 读（`--prompt-file`，与 `crewd claude`
//! 同一套读法）：命令行参数在 `ps` 里对本机所有进程可见。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use crewd_exec::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
use crewd_exec::driver::Control;
use crewd_exec::env::AgentPaths;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::SessionEvent;
use crewd_server::ledger::StoreLedger;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::claude_cmd::{read_prompt, stdin_lines};

#[derive(clap::Args, Clone)]
pub struct CodexArgs {
    /// 工作目录
    #[arg(long)]
    workdir: PathBuf,
    /// crewd session id：给了就用库里的 thread 账本（行必须已存在且执行器是 codex，重跑会 thread/resume）；不给只在内存里记
    #[arg(long)]
    session: Option<String>,
    /// 开场任务所在的文件，`-` = 从 stdin 读到 EOF（此后 stdin 不再收指令）。末尾的空白不算正文。
    /// 走 turn/start（经 codex 的 stdin），绝不进 argv——也不收命令行上的正文：`ps` 对本机所有进程可见
    #[arg(long, value_name = "PATH")]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    /// 开 fast 档（不给 = 用 codex 自己的配置）
    #[arg(long)]
    fast: bool,
    /// codex 可执行文件；缺省按登录 shell 的 PATH 找
    #[arg(long)]
    codex: Option<PathBuf>,
    /// 每次转入空闲时把最近的结构化记录打到 stderr
    #[arg(long)]
    transcript: bool,
}

pub async fn run(data_dir: &Path, args: CodexArgs) -> anyhow::Result<()> {
    crewd_server::ensure_data_dir(data_dir)?;
    // 与 serve / import / claude 同一把锁：账本写库、回收孤儿都要求没有别的 crewd 活着。
    let _lock = crewd_server::lock_data_dir(data_dir)?;
    let (registry, reports) = crewd_server::reap_orphans(data_dir)?;
    crewd_server::log_reap_reports(&reports);
    crewd_server::recovery::require_settled(&registry)?;
    // thread/start 的 cwd 由 codex 解析：相对路径先在这里转成绝对路径。
    let workdir = std::fs::canonicalize(&args.workdir)
        .with_context(|| format!("工作目录 {} 不可用", args.workdir.display()))?;
    let paths = AgentPaths::discover().await;
    let program = match &args.codex {
        Some(p) => std::fs::canonicalize(p).with_context(|| format!("找不到 {}", p.display()))?,
        None => paths
            .resolve("codex")
            .context("登录 shell 的 PATH 里找不到 codex")?,
    };
    let prompt_from_stdin = args.prompt_file.as_deref() == Some(Path::new("-"));
    let initial_prompt = match &args.prompt_file {
        Some(path) => Some(read_prompt(path).await?),
        None => None,
    };
    let ledger: Arc<dyn AgentIdLedger> = match &args.session {
        Some(s) => {
            Arc::new(StoreLedger::new(data_dir.join("crewd.db"), s.clone()).expecting("codex"))
        }
        None => Arc::new(MemoryLedger::default()),
    };
    let config = CodexConfig {
        registry: Some(registry),
        ..CodexConfig::default()
    };
    let start = CodexStart {
        session_id: args.session.unwrap_or_else(|| "cli".to_string()),
        program,
        child_path: paths.child_path,
        spec: CodexSpec {
            workdir,
            model: args.model,
            effort: args.effort,
            fast_mode: args.fast.then_some(true),
            initial_prompt,
            ..CodexSpec::default()
        },
        ledger,
    };
    let (handle, mut events) = start_codex(start, config).await?;
    // stdin 已经被开场任务读到 EOF 了就不再收指令。stdin 在普通线程上读（见 `stdin_lines`）。
    let mut stdin_open = !prompt_from_stdin;
    let mut lines = if stdin_open {
        stdin_lines()
    } else {
        mpsc::unbounded_channel().1
    };
    // 信号流在循环外建好：每轮新建一个 `ctrl_c()`，两轮之间来的 Ctrl-C 没人接。
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match &ev {
                    // 条目原样太长，只打类型与 id。
                    SessionEvent::Item { turn_id, item } => println!(
                        "Item {{ turn_id: {turn_id:?}, type: {}, id: {} }}",
                        item["type"], item["id"]
                    ),
                    other => println!("{other:?}"),
                }
                if args.transcript
                    && ev == SessionEvent::Working(false)
                    && let Ok(text) = handle.screen_text(50).await
                {
                    eprintln!("----- transcript -----\n{text}\n----------------------");
                }
                if matches!(ev, SessionEvent::Exited(_)) {
                    break;
                }
            }
            line = lines.recv(), if stdin_open => match line {
                Some(text) if text.trim() == "/interrupt" => { let _ = handle.interrupt(); }
                Some(text) if text.trim() == "/compact" => {
                    if let Ok(outcome) = handle.control(Control::Compact).await {
                        println!("{outcome:?}");
                    }
                }
                // 空行被句柄拒掉（`SendError::EmptyText`），不用在这里挑。
                Some(text) => { let _ = handle.send(text); }
                None => stdin_open = false,
            },
            _ = int.recv() => { let _ = handle.stop(); }
            _ = term.recv() => { let _ = handle.stop(); }
        }
    }
    Ok(())
}

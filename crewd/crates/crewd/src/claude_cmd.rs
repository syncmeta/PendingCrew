//! `crewd claude`：前台起一个 claude 会话（M2b 的验收与调试入口；M3 起由 serve 拉起会话）。
//! 事件逐行打到 stdout；stdin 每一行当一条指令投进去；Ctrl-C / SIGTERM 停掉会话。
//! 开场任务只从文件或 stdin 读（`--prompt-file`）：命令行参数在 `ps` 里对本机所有进程可见。
use std::io::{BufRead, Read};
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
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

#[derive(clap::Args, Clone)]
pub struct ClaudeArgs {
    /// 工作目录
    #[arg(long)]
    workdir: PathBuf,
    /// crewd session id：给了就用库里的会话号账本（行必须已存在，重跑会 --resume）；不给只在内存里记
    #[arg(long)]
    session: Option<String>,
    /// 开场任务所在的文件，`-` = 从 stdin 读到 EOF（此后 stdin 不再收指令）。末尾的空白不算正文。
    /// 走 PTY 投递，绝不进 argv——也不收命令行上的正文：`ps` 对本机所有进程可见
    #[arg(long, value_name = "PATH")]
    prompt_file: Option<PathBuf>,
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

/// `--prompt-file`：文件或 stdin（`-`）。末尾的换行 / 空白去掉（文件末尾的换行不是正文的一部分）。
async fn read_prompt(path: &Path) -> anyhow::Result<String> {
    let text = if path == Path::new("-") {
        tokio::task::spawn_blocking(|| {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).map(|_| s)
        })
        .await?
        .context("从 stdin 读开场任务失败")?
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("读不了开场任务文件 {}", path.display()))?
    };
    Ok(text.trim_end().to_string())
}

/// stdin 逐行读进通道：放在一条普通线程上，**不**用 tokio 的 stdin——后者占着运行时的
/// 阻塞线程池，会话结束后运行时收尾要等它读完下一行（终端里就是「再按一次回车才退」）。
/// 普通线程不拦退出：`main` 返回时它随进程一起结束。
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    rx
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
    let prompt_from_stdin = args.prompt_file.as_deref() == Some(Path::new("-"));
    let initial_prompt = match &args.prompt_file {
        Some(path) => Some(read_prompt(path).await?),
        None => None,
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
            initial_prompt,
            ..ClaudeSpec::default()
        },
        ledger,
    };
    let (handle, mut events) = start_claude(start, config).await?;
    // stdin 已经被开场任务读到 EOF 了就不再收指令。
    let mut stdin_open = !prompt_from_stdin;
    let mut lines = if stdin_open {
        stdin_lines()
    } else {
        mpsc::unbounded_channel().1
    };
    // 信号流在循环外建好：每轮新建一个 `ctrl_c()`，两轮之间来的 Ctrl-C 没人接（处置已被
    // tokio 换掉，信号就这么丢了）。
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
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
            line = lines.recv(), if stdin_open => match line {
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

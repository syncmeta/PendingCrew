//! crewd：PendingCrew 的跨平台后端。
mod claude_cmd;
mod codex_cmd;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use crewd_proto::{Request, ServerFrame};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "crewd", version, about = "PendingCrew backend")]
struct Cli {
    /// 数据目录（库、socket、锁、附件都在这里）
    #[arg(long, global = true, env = "CREWD_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 前台运行服务
    Serve,
    /// 从旧版 JSON 数据只读导入到一个新库
    Import {
        /// 旧数据目录；缺省取 PENDINGCREW_DATA_DIR 或 ~/Library/Application Support/PendingCrew
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// 立即备份库到 <数据目录>/backups/crewd-manual-*.db（serve 在跑也行：走自己的只读连接）
    Backup,
    /// 从备份恢复（serve 必须停着）。原来的库挪成 crewd.db.replaced-*，永不覆盖
    Restore {
        /// 备份文件（通常在 <数据目录>/backups/ 里）
        #[arg(long)]
        from: PathBuf,
    },
    /// 检查库的完整性（integrity_check + foreign_key_check；只读，serve 在跑也行）。只报告，不修、不删
    Check {
        /// 只跑 quick_check（快，不核索引内容）
        #[arg(long)]
        quick: bool,
    },
    /// 调用一个 RPC 方法并打印结果
    Rpc {
        method: String,
        /// JSON 参数，缺省 null
        params: Option<String>,
    },
    /// 打印 API 契约
    Schema,
    /// 前台起一个 claude 会话（M2b 验收 / 调试；会话事件打到 stdout）
    Claude(claude_cmd::ClaudeArgs),
    /// 前台起一个 codex 会话（M2c 验收 / 调试；会话事件打到 stdout）
    Codex(codex_cmd::CodexArgs),
}

/// `--data-dir` / `CREWD_DATA_DIR`：与 PENDINGCREW_DATA_DIR 同规则——去首尾空白、展开 `~/`；
/// 空的视为没给。
fn data_dir(cli: &Cli) -> anyhow::Result<PathBuf> {
    if let Some(d) = &cli.data_dir {
        match d.to_str() {
            Some(s) if !s.trim().is_empty() => {
                return Ok(crewd_legacy::paths::expand_tilde(s.trim()));
            }
            Some(_) => {}
            None => return Ok(d.clone()),
        }
    }
    Ok(dirs::data_dir()
        .context("无法确定用户数据目录")?
        .join("crewd"))
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("crewd: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dir = data_dir(&cli)?;
    match &cli.cmd {
        Cmd::Serve => {
            crewd_server::serve(dir, shutdown_signal()).await?;
        }
        Cmd::Import { from } => {
            let src = from
                .clone()
                .unwrap_or_else(crewd_legacy::paths::default_legacy_root);
            // 先于任何写入（建目录、开库）：数据目录绝不能落在旧数据目录里，反之亦然。
            crewd_legacy::paths::check_disjoint(&src, &dir)?;
            crewd_server::ensure_data_dir(&dir)?;
            // 与 serve 同一把锁：正在服务的库不能被导入写。
            let _lock = crewd_server::lock_data_dir(&dir)?;
            let db = dir.join(crewd_store::DB_FILE);
            // 与 serve 同一道身份核对：库丢了 / 换了就拒绝，而不是往一个新建的空库里导（N4 / S24）。
            let mut store = crewd_store::Store::open_data_dir(&dir)?;
            // 只拒绝非空的库；上次导入失败留下的空库允许重试。
            if !store.is_empty()? {
                bail!(
                    "{} 已存在且非空。导入只写新库：先把它连同 {} 一起挪出数据目录（只挪库文件会被身份核对拦住），再导入。",
                    db.display(),
                    crewd_store::ID_FILE
                );
            }
            let report = crewd_legacy::import(&src, &mut store, &dir)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Cmd::Backup => {
            use crewd_store::backup;
            let db = dir.join(crewd_store::DB_FILE);
            let dest =
                backup::backups_dir_for(&db).join(backup::manual_name(crewd_store::now_ms()));
            backup::backup_db(&db, &dest)?;
            println!("{}", dest.display());
        }
        Cmd::Restore { from } => {
            crewd_server::ensure_data_dir(&dir)?;
            // 与 serve 同一把锁：serve 在跑就拒绝（它手里的库不能被换掉）。
            let _lock = crewd_server::lock_data_dir(&dir)?;
            let r = crewd_store::backup::restore(&dir, from)?;
            match r.replaced {
                Some(p) => println!("已恢复；原来的库挪到了 {}", p.display()),
                None => println!("已恢复（原来没有库）"),
            }
        }
        Cmd::Check { quick } => {
            use crewd_store::backup::{self, Check};
            let db = dir.join(crewd_store::DB_FILE);
            let kind = if *quick { Check::Quick } else { Check::Full };
            let problems = backup::check(&backup::open_for_check(&db)?, kind)?;
            if !problems.is_empty() {
                bail!(
                    "{} 没通过 {}（只报告，没有改动任何文件；可用 crewd restore 从 backups/ 恢复）：\n{}",
                    db.display(),
                    kind.name(),
                    problems.join("\n")
                );
            }
            println!("ok（{}）", kind.name());
        }
        Cmd::Rpc { method, params } => {
            let params: serde_json::Value = match params {
                Some(p) => serde_json::from_str(p).context("PARAMS 不是合法 JSON")?,
                None => serde_json::Value::Null,
            };
            let result = call(&dir, method, params)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Cmd::Claude(args) => claude_cmd::run(&dir, args.clone()).await?,
        Cmd::Codex(args) => codex_cmd::run(&dir, args.clone()).await?,
        Cmd::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(&crewd_proto::api_schema())?
            );
        }
    }
    Ok(())
}

fn call(
    dir: &std::path::Path,
    method: &str,
    params: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let sock = dir.join("crewd.sock");
    let mut stream = UnixStream::connect(&sock)
        .with_context(|| format!("连不上 {}（crewd serve 在跑吗？）", sock.display()))?;
    let req = Request {
        id: 1,
        method: method.to_string(),
        params,
    };
    let mut line = serde_json::to_vec(&req)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut buf = String::new();
        if reader.read_line(&mut buf)? == 0 {
            bail!("crewd 在回复前关闭了连接");
        }
        if let ServerFrame::Response(r) = serde_json::from_str::<ServerFrame>(&buf)? {
            if r.id != 1 {
                continue;
            }
            // `"result": null` 是成功（值为 null）；错误带码打出来，`crewd rpc` 非 0 退出。
            return r
                .into_result()
                .map_err(|e| anyhow::anyhow!("{} ({})", e.message, e.code));
        }
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("无法监听 SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

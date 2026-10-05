//! 没被建模导入、也不是缓存的文件，原样存进 legacy_archive（键 = 相对源目录的路径）。
use crate::Ctx;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 只在数据根第一层跳过的目录。
const SKIP_TOP_DIRS: &[&str] = &[
    "attachments",
    "crashes",
    "backups",
    "diagnostics",
    "cli-maintenance-logs",
    "welcome-crew",
    "crewd",
];
/// 运行时缓存/镜像/锁：导入后无意义。
const SKIP_FILES: &[&str] = &[
    "crew-sessions.json",
    "quota.json",
    "models.json",
    "expected-live-sessions.json",
    "automatic-wake-admission.json",
    "app.lastexit.json",
    "orchestrator.lock",
    ".DS_Store",
];
const SKIP_SUFFIXES: &[&str] = &[
    ".lock",
    ".cursor",
    ".turn",
    ".crewresp.json",
    ".crewcmd.json",
    ".captain-delegation-seen",
];

fn is_runtime_file(name: &str) -> bool {
    SKIP_FILES.contains(&name)
        || name.starts_with("daemon.")
        || SKIP_SUFFIXES.iter().any(|s| name.ends_with(s))
}

pub fn import_rest(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<()> {
    let root = cx.src.to_path_buf();
    walk(tx, cx, &root, true)
}

fn walk(tx: &Connection, cx: &mut Ctx, dir: &Path, top: bool) -> crewd_store::Result<()> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            cx.report
                .skip(cx.src, dir, None, format!("目录读不出: {e}"));
            return Ok(());
        }
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if p.is_dir() {
            if top && SKIP_TOP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(tx, cx, &p, false)?;
            continue;
        }
        if is_runtime_file(&name) || cx.consumed.contains(&p) {
            continue;
        }
        match std::fs::read(&p) {
            Ok(bytes) => {
                let key = p.strip_prefix(cx.src).unwrap_or(&p).display().to_string();
                crewd_store::archive::put(tx, &key, &bytes)?;
                cx.report.bump("archivedFiles");
            }
            Err(e) => cx.report.skip(cx.src, &p, None, format!("读取失败: {e}")),
        }
    }
    Ok(())
}

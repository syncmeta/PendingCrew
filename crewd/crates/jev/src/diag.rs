//! 本机诊断日志（spec §7.1）：每次判断一行 JSON，记输入摘要（脱敏后）、答案、置信度、耗时。
//! 不记 key，不上传。文件 0600；超过上限就把旧文件挪成 `<名字>.1`（只留一份旧的）。
use serde::Serialize;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const DEFAULT_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub const EXCERPT_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagRecord {
    pub at_unix_ms: u64,
    pub decision: &'static str,
    pub model: String,
    /// 脱敏后 state 的字符数与开头一段。
    pub state_chars: usize,
    pub state_excerpt: String,
    /// 按发出顺序的选项名 / 题号。
    pub options: Vec<String>,
    pub scrub_hits: usize,
    pub attempts: u32,
    pub latency_ms: u64,
    /// "act" | "defer"
    pub outcome: &'static str,
    pub reason: Option<String>,
    pub confidence: Option<f64>,
    pub answer: Value,
}

#[derive(Debug)]
pub struct DiagLog {
    path: PathBuf,
    max_bytes: u64,
    file: Mutex<Option<File>>,
}

fn open_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    // 早先以更宽权限建过的文件，收紧回 0600。
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

impl DiagLog {
    /// 父目录必须已存在（crewd 的数据目录，0700）。
    pub fn open(path: impl Into<PathBuf>) -> io::Result<DiagLog> {
        let path = path.into();
        let file = open_append(&path)?;
        Ok(DiagLog {
            path,
            max_bytes: DEFAULT_MAX_BYTES,
            file: Mutex::new(Some(file)),
        })
    }

    pub fn with_max_bytes(mut self, max_bytes: u64) -> DiagLog {
        self.max_bytes = max_bytes;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn rotated_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".1");
        self.path.with_file_name(name)
    }

    pub fn append(&self, record: &DiagRecord) -> io::Result<()> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let len = match guard.as_ref() {
            Some(f) => f.metadata()?.len(),
            None => 0,
        };
        if guard.is_none() || (len > 0 && len + line.len() as u64 > self.max_bytes) {
            *guard = None;
            if len > 0 {
                std::fs::rename(&self.path, self.rotated_path())?;
            }
            *guard = Some(open_append(&self.path)?);
        }
        let file = guard.as_mut().expect("刚打开");
        file.write_all(&line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(n: usize) -> DiagRecord {
        DiagRecord {
            at_unix_ms: 1,
            decision: "route_todo",
            model: "jev-1.13.0".into(),
            state_chars: n,
            state_excerpt: "{\"todo\":\"修滚动\"}".into(),
            options: vec!["captain".into(), "m1".into()],
            scrub_hits: 0,
            attempts: 1,
            latency_ms: 900,
            outcome: "act",
            reason: None,
            confidence: Some(0.9),
            answer: json!({"target": "member", "key": "m1"}),
        }
    }

    #[test]
    fn one_json_object_per_line_in_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagLog::open(dir.path().join("jev-diagnostics.jsonl")).unwrap();
        log.append(&record(1)).unwrap();
        log.append(&record(2)).unwrap();
        let text = std::fs::read_to_string(log.path()).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["state_chars"], 2);
        assert_eq!(lines[0]["answer"]["key"], "m1");
        let mode = std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn existing_loose_file_is_tightened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _log = DiagLog::open(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn rotates_into_a_single_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = DiagLog::open(dir.path().join("d.jsonl"))
            .unwrap()
            .with_max_bytes(600);
        for i in 0..10 {
            log.append(&record(i)).unwrap();
        }
        let current = std::fs::read_to_string(log.path()).unwrap();
        let old = std::fs::read_to_string(log.rotated_path()).unwrap();
        assert!(current.len() <= 600, "当前文件 {} 字节", current.len());
        assert!(!old.is_empty());
        let last: Value = serde_json::from_str(current.lines().last().unwrap()).unwrap();
        assert_eq!(last["state_chars"], 9, "最新一条在当前文件里");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 2, "只留一份旧文件");
    }
}

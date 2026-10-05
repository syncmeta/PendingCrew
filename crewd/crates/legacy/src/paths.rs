use std::fs;
use std::path::{Path, PathBuf};

/// 旧版数据根：`PENDINGCREW_DATA_DIR`（trim、展开 `~/`）> `~/Library/Application Support/PendingCrew`。
pub fn default_legacy_root() -> PathBuf {
    if let Ok(v) = std::env::var("PENDINGCREW_DATA_DIR") {
        let v = v.trim();
        if !v.is_empty() {
            return expand_tilde(v);
        }
    }
    dirs::data_dir()
        .expect("无法确定用户数据目录")
        .join("PendingCrew")
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// 旧数据里附件存的是绝对路径；导入时把源目录下的附件前缀换成新数据目录。
pub struct PathRewrite {
    pub from: PathBuf,
    pub to: PathBuf,
}

impl PathRewrite {
    pub fn apply(&self, p: &str) -> String {
        match Path::new(p).strip_prefix(&self.from) {
            Ok(rest) => self.to.join(rest).to_string_lossy().into_owned(),
            Err(_) => p.to_string(),
        }
    }
}

/// 递归复制，返回复制的文件数。源不存在返回 0。
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<usize> {
    if !from.is_dir() {
        return Ok(0);
    }
    fs::create_dir_all(to)?;
    let mut n = 0;
    for e in fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        let t = to.join(e.file_name());
        if p.is_dir() {
            n += copy_dir(&p, &t)?;
        } else {
            fs::copy(&p, &t)?;
            n += 1;
        }
    }
    Ok(n)
}

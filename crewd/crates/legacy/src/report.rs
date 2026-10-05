use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub counts: BTreeMap<String, usize>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Skipped {
    /// 相对源目录的路径。
    pub file: String,
    /// 数组里的第几行；`None` 表示整个文件。
    pub index: Option<usize>,
    pub reason: String,
}

impl ImportReport {
    pub fn add(&mut self, key: &str, n: usize) {
        *self.counts.entry(key.to_string()).or_default() += n;
    }
    pub fn bump(&mut self, key: &str) {
        self.add(key, 1);
    }
    pub fn skip(
        &mut self,
        root: &Path,
        file: &Path,
        index: Option<usize>,
        reason: impl Into<String>,
    ) {
        self.skipped.push(Skipped {
            file: file
                .strip_prefix(root)
                .unwrap_or(file)
                .display()
                .to_string(),
            index,
            reason: reason.into(),
        });
    }
    pub fn warn(&mut self, w: impl Into<String>) {
        self.warnings.push(w.into());
    }
}

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

/// 展开开头的 `~/`（其余原样）。用于环境变量里的路径：shell 不会替它们展开。
pub fn expand_tilde(s: &str) -> PathBuf {
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

/// 解析成绝对、规范的路径；路径可以还不存在：规范化最近的已存在祖先，再把其余部分按字面接回去
/// （`.` 丢掉，`..` 弹一级——不存在的部分不可能是符号链接，字面处理是准确的）。
pub fn resolve_lenient(p: &Path) -> std::io::Result<PathBuf> {
    use std::path::Component;
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let comps: Vec<Component> = abs.components().collect();
    for n in (1..=comps.len()).rev() {
        let prefix: PathBuf = comps[..n].iter().collect();
        match prefix.canonicalize() {
            Ok(mut out) => {
                for c in &comps[n..] {
                    match c {
                        Component::ParentDir => {
                            out.pop();
                        }
                        Component::Normal(s) => out.push(s),
                        _ => {}
                    }
                }
                return Ok(out);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(abs)
}

fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// `inner` 是 `outer` 本身或在它里面。路径比较之外再按 inode 比一遍祖先，
/// 挡住大小写不敏感文件系统上大小写不同的同一目录。
fn is_within(inner: &Path, outer: &Path) -> bool {
    inner.starts_with(outer) || inner.ancestors().any(|a| same_file(a, outer))
}

/// 旧数据目录与 crewd 数据目录必须互不包含：导入绝不往旧数据目录里写任何东西。
pub fn check_disjoint(src: &Path, dest: &Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let s = resolve_lenient(src).with_context(|| format!("无法解析路径 {}", src.display()))?;
    let d = resolve_lenient(dest).with_context(|| format!("无法解析路径 {}", dest.display()))?;
    if is_within(&d, &s) || is_within(&s, &d) {
        anyhow::bail!(
            "数据目录 {} 与旧数据目录 {} 重叠：导入绝不往旧数据目录里写，请换一个不在其中的数据目录",
            d.display(),
            s.display()
        );
    }
    Ok(())
}

/// 递归复制，返回复制的文件数。源不存在返回 0。目标是源本身或在源里面时拒绝
/// （自己复制到自己会把文件截断成空）。
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<usize> {
    if !from.is_dir() {
        return Ok(0);
    }
    let (f, t) = (resolve_lenient(from)?, resolve_lenient(to)?);
    if is_within(&t, &f) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("复制目标 {} 与源 {} 重叠", t.display(), f.display()),
        ));
    }
    copy_tree(from, to)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<usize> {
    fs::create_dir_all(to)?;
    let mut n = 0;
    for e in fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        let t = to.join(e.file_name());
        if p.is_dir() {
            n += copy_tree(&p, &t)?;
        } else {
            fs::copy(&p, &t)?;
            // 引用这些附件的事务提交之前，附件本身要已经落盘（N4 / S27）。
            fs::File::open(&t)?.sync_all()?;
            n += 1;
        }
    }
    fs::File::open(to)?.sync_all()?;
    Ok(n)
}

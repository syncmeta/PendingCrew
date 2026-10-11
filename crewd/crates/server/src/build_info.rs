//! 这个 crewd 是谁（U1 / S6 的 daemon 一半，`system.hello` 的 `build`）：git 提交、构建时干不干净、可执行文件的路径，
//! 以及那个文件此刻还是不是正在跑的这一个——更新把它换掉 / 删掉之后 = false，客户端据此知道「要重启 crewd 才用上新版」。
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// 构建时的 git 提交（12 位短号；发版流水线经 `CREWD_GIT_SHA` 给；拿不到 = `unknown`）。见 `build.rs`。
pub const GIT_SHA: &str = env!("CREWD_GIT_SHA");

/// 构建时工作区有没有改动（已跟踪的文件）。
pub fn git_dirty() -> bool {
    env!("CREWD_GIT_DIRTY") == "true"
}

/// 一个文件的身份：路径 + (设备号, inode)。路径还在、却换了 inode = 被换掉了（更新是「写新文件再改名过去」）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExeIdentity {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

impl ExeIdentity {
    pub fn capture(path: &Path) -> std::io::Result<Self> {
        let m = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            dev: m.dev(),
            ino: m.ino(),
        })
    }

    /// 路径上此刻还是不是记下的那个文件（没了 / 换了 = false）。
    pub fn still_on_disk(&self) -> bool {
        std::fs::metadata(&self.path).is_ok_and(|m| m.dev() == self.dev && m.ino() == self.ino)
    }
}

static EXE: LazyLock<Option<ExeIdentity>> = LazyLock::new(|| {
    let path = std::env::current_exe().ok()?;
    ExeIdentity::capture(&path).ok()
});

/// 正在跑的可执行文件，第一次调用时记下（T11 在 serve 一开始就调一次，免得更新在那之前就换掉了它）。
pub fn exe() -> Option<&'static ExeIdentity> {
    EXE.as_ref()
}

/// `system.hello` 的 `build`。
pub fn build_info() -> crewd_proto::BuildInfo {
    let exe = exe();
    crewd_proto::BuildInfo {
        git_sha: GIT_SHA.to_string(),
        dirty: git_dirty(),
        exe: exe
            .map(|e| e.path.display().to_string())
            .unwrap_or_default(),
        exe_still_on_disk: exe.is_some_and(ExeIdentity::still_on_disk),
    }
}

/// Compare declared compiled commits conservatively; copied paths/inodes are not equality.
/// Missing, unknown or dirty metadata cannot prove these artifacts are the same build.
/// This function never authorizes a caller, mutates state, retries, or restarts anything.
pub fn compare_helper_build(
    daemon: &crewd_proto::BuildInfo,
    helper: Option<&crewd_proto::BuildInfo>,
) -> crewd_proto::agent::HelperBuildStatus {
    use crewd_proto::agent::HelperBuildStatus;
    let known_clean = |build: &crewd_proto::BuildInfo| {
        !build.dirty && !build.git_sha.trim().is_empty() && build.git_sha != "unknown"
    };
    let Some(helper) = helper else {
        return HelperBuildStatus::Unknown;
    };
    if !known_clean(daemon) || !known_clean(helper) {
        return HelperBuildStatus::Unknown;
    }
    if daemon.git_sha == helper.git_sha {
        HelperBuildStatus::Same
    } else {
        HelperBuildStatus::Skew
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s6_build_comparison_is_conservative_and_independent_of_exe_path() {
        use crewd_proto::agent::HelperBuildStatus::{Same, Skew, Unknown};
        let daemon = crewd_proto::BuildInfo {
            git_sha: "a".into(),
            dirty: false,
            exe: "/synthetic/daemon".into(),
            exe_still_on_disk: true,
        };
        let mut helper = daemon.clone();
        helper.exe = "/different/copied/helper".into();
        helper.exe_still_on_disk = false;
        assert_eq!(compare_helper_build(&daemon, Some(&helper)), Same);
        helper.git_sha = "b".into();
        assert_eq!(compare_helper_build(&daemon, Some(&helper)), Skew);
        assert_eq!(compare_helper_build(&daemon, None), Unknown);
        for (sha, dirty) in [("unknown", false), ("", false), (" ", false), ("a", true)] {
            let unqualified = crewd_proto::BuildInfo {
                git_sha: sha.into(),
                dirty,
                ..daemon.clone()
            };
            assert_eq!(compare_helper_build(&unqualified, Some(&helper)), Unknown);
            assert_eq!(compare_helper_build(&daemon, Some(&unqualified)), Unknown);
        }
    }

    /// 测的是比对本身（临时文件，不碰真的可执行文件）：记下 → 还在；写一个新文件改名盖过去 → 不是它了；删了 → 不在了。
    #[test]
    fn a_replaced_or_deleted_file_is_no_longer_the_one_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd");
        std::fs::write(&path, b"old").unwrap();
        let id = ExeIdentity::capture(&path).unwrap();
        assert!(id.still_on_disk());
        std::fs::write(&path, b"old, rewritten in place").unwrap();
        assert!(id.still_on_disk(), "原地改写 inode 不变");
        let new = dir.path().join("crewd.new");
        std::fs::copy(&path, &new).unwrap();
        std::fs::rename(&new, &path).unwrap();
        assert!(!id.still_on_disk(), "复制一份再改名盖过去 = 换了文件");
        let again = ExeIdentity::capture(&path).unwrap();
        assert!(again.still_on_disk());
        std::fs::remove_file(&path).unwrap();
        assert!(!again.still_on_disk(), "删了");
        assert!(ExeIdentity::capture(&path).is_err());
    }

    #[test]
    fn the_running_binary_has_a_build_identity() {
        let b = build_info();
        assert!(!b.git_sha.is_empty());
        assert!(!b.exe.is_empty(), "测试二进制的路径");
        assert!(b.exe_still_on_disk, "测试跑着时它的二进制还在");
    }
}

//! 在 PTY 里起子进程（手写：rustix-openpty + tokio::process）。
//!
//! 子进程：`setsid`（自成会话与进程组，pgid == pid，stop 时 `killpg` 才收得干净）、
//! `TIOCSCTTY`（PTY 从端成为控制终端，`/dev/tty` 才打得开）、环境整份替换（不继承
//! crewd 的环境）、cwd 不存在即启动失败。SIGPIPE 由 std 在 exec 前复位成默认
//! （crewd 自己忽略 SIGPIPE，不复位的话子进程里 `yes | head` 会报 Broken pipe）。
//! 主端：非阻塞，挂在 tokio 的 `AsyncFd` 上读写。
use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::process::Stdio;

use rustix::termios::Winsize;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

/// 报给 PTY 的每格像素（与 `crewd_term::Screen` 对 `CSI 14 t` 的回答一致：16×16）。
pub const CELL_PIXELS: u16 = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// 子进程的**全部**环境。
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub cols: u16,
    pub lines: u16,
}

/// 起好的子进程。`pid` 也是它的进程组号。
pub struct PtyProcess {
    pub child: tokio::process::Child,
    pub pid: u32,
    pub master: PtyMaster,
}

/// PTY 主端。
pub struct PtyMaster {
    fd: AsyncFd<OwnedFd>,
    observer: Option<std::sync::Arc<dyn crate::acp::terminal::TerminalObserver>>,
}

fn winsize(cols: u16, lines: u16) -> Winsize {
    Winsize {
        ws_row: lines,
        ws_col: cols,
        ws_xpixel: cols.saturating_mul(CELL_PIXELS),
        ws_ypixel: lines.saturating_mul(CELL_PIXELS),
    }
}

/// 起进程。必须在 tokio 运行时里调用（主端要注册到 reactor）。
pub fn spawn_pty(spec: &SpawnSpec) -> io::Result<PtyProcess> {
    spawn_pty_gated(spec, None)
}
/// An optional controlled native birth barrier is completed before exec; default launches unchanged.
pub fn spawn_pty_gated(
    spec: &SpawnSpec,
    gate: Option<&crate::acp::birth::BirthGate>,
) -> io::Result<PtyProcess> {
    let birth = gate
        .map(crate::acp::birth::BirthGate::prepare)
        .transpose()?;
    let pty = rustix_openpty::openpty(None, Some(&winsize(spec.cols, spec.lines)))?;
    let (master, slave) = (pty.controller, pty.user);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(mut t) = rustix::termios::tcgetattr(&master) {
        // 与 alacritty 一致：行编辑模式下退格按 UTF-8 字符删。
        t.input_modes.insert(rustix::termios::InputModes::IUTF8);
        let _ = rustix::termios::tcsetattr(&master, rustix::termios::OptionalActions::Now, &t);
    }

    // 主端的非阻塞与 reactor 注册放在起子进程之前：这两步失败时还没有子进程，不会留下一个
    // 没人管、没人收尸的进程。（O_NONBLOCK 在主端那份打开文件上，不影响子进程拿到的从端。）
    rustix::io::ioctl_fionbio(&master, true)?;
    let fd = AsyncFd::with_interest(master, Interest::READABLE | Interest::WRITABLE)?;

    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: 闭包只做 async-signal-safe 的裸系统调用（setsid、ioctl），不分配内存。
    unsafe {
        cmd.pre_exec(move || {
            rustix::process::setsid()?;
            rustix::process::ioctl_tiocsctty(rustix::stdio::stdin())?;
            if let Some(gate) = &birth {
                gate.wait_before_exec()?;
            }
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    // `cmd` 里还攥着从端的三个副本；不 drop 掉，子进程退出后主端永远读不到 EOF。
    drop(cmd);
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("子进程刚起就已被回收"))?;
    Ok(PtyProcess {
        child,
        pid,
        master: PtyMaster { fd, observer: None },
    })
}

impl PtyMaster {
    /// Attach before the owning driver begins reading; every read path observes exactly once.
    pub fn observe_output(
        &mut self,
        observer: std::sync::Arc<dyn crate::acp::terminal::TerminalObserver>,
    ) {
        self.observer = Some(observer);
    }
    fn observed(&self, bytes: &[u8]) {
        if !bytes.is_empty()
            && let Some(observer) = &self.observer
        {
            observer.observe(bytes);
        }
    }

    /// 不等：此刻读得到就读一块（`Some(n)`，`Some(0)` = 从端全关了），读不到 = `None`。直接对 fd 做一次非阻塞读，
    /// 不看 tokio 的就绪缓存（缓存可能还没反映刚到的字节）；之后 `read` 遇到过期的就绪会自己清掉再等。
    pub fn try_read(&self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        loop {
            match rustix::io::read(self.fd.get_ref(), &mut *buf).map_err(io::Error::from) {
                Ok(n) => {
                    self.observed(&buf[..n]);
                    return Ok(Some(n));
                }
                Err(e) if e.raw_os_error() == Some(libc::EIO) => return Ok(Some(0)),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    /// 读一块。`Ok(0)` = 从端全关了（子进程退出；Linux 上这时 read 报 EIO，也映射成 0）。
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            match guard
                .try_io(|fd| rustix::io::read(fd.get_ref(), &mut *buf).map_err(io::Error::from))
            {
                Ok(Ok(n)) => {
                    self.observed(&buf[..n]);
                    return Ok(n);
                }
                Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => return Ok(0),
                Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => continue,
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => continue,
            }
        }
    }

    /// 驱动的单任务写队列：只等可写，不在另一个任务里继续整段 write_all。
    pub(crate) async fn writable(
        &self,
    ) -> io::Result<tokio::io::unix::AsyncFdReadyGuard<'_, OwnedFd>> {
        self.fd.writable().await
    }

    /// 一次非阻塞写。驱动在每一次系统调用之前核外来输入闸，并保留部分写入的 offset。
    pub(crate) fn try_write(&self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            match rustix::io::write(self.fd.get_ref(), bytes).map_err(io::Error::from) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }

    /// 写完整段。子进程不读时会一直等（PTY 缓冲满）。
    pub async fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(io::Error::from))
            {
                Ok(Ok(0)) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(Ok(n)) => bytes = &bytes[n..],
                Ok(Err(e)) if e.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(e)) => return Err(e),
                Err(_would_block) => {}
            }
        }
        Ok(())
    }

    /// 改窗口尺寸（内核随后给前台进程组发 SIGWINCH）。
    pub fn resize(&self, cols: u16, lines: u16) -> io::Result<()> {
        rustix::termios::tcsetwinsize(self.fd.get_ref().as_fd(), winsize(cols, lines))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn raw_tap_observes_real_pty_once_across_async_and_drain_reads() {
        use crate::acp::terminal::TerminalTap;
        let mut p = spawn_pty(&sh("printf '\\033[32m原生🙂\\033[0m'; exit 0")).unwrap();
        let tap = std::sync::Arc::new(TerminalTap::new(1, 64).unwrap());
        let mut observed = tap.subscribe();
        p.master.observe_output(tap);
        let mut actual = Vec::new();
        let mut buf = [0u8; 3];
        let n = p.master.read(&mut buf).await.unwrap();
        actual.extend_from_slice(&buf[..n]);
        loop {
            match p.master.try_read(&mut buf).unwrap() {
                Some(0) => break,
                Some(n) => actual.extend_from_slice(&buf[..n]),
                None => {
                    let n = p.master.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    actual.extend_from_slice(&buf[..n]);
                }
            }
        }
        assert!(p.child.wait().await.unwrap().success());
        let mut mirrored = Vec::new();
        while let Ok(frame) = observed.try_recv() {
            mirrored.extend_from_slice(&frame.bytes);
        }
        assert_eq!(actual, "\x1b[32m原生🙂\x1b[0m".as_bytes());
        assert_eq!(mirrored, actual);
    }

    fn sh(script: &str) -> SpawnSpec {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        env.insert("FOO".to_string(), "bar".to_string());
        SpawnSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".to_string(), script.to_string()],
            env,
            cwd: std::env::temp_dir(),
            cols: 80,
            lines: 25,
        }
    }

    async fn read_to_end(master: &PtyMaster) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(10), master.read(&mut buf))
                .await
                .expect("10s 内没读到 EOF")
                .unwrap();
            if n == 0 {
                return String::from_utf8_lossy(&out).into_owned();
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    #[tokio::test]
    async fn output_exit_code_and_eof() {
        let mut p = spawn_pty(&sh("printf hello; exit 3")).unwrap();
        assert!(read_to_end(&p.master).await.contains("hello"));
        assert_eq!(p.child.wait().await.unwrap().code(), Some(3));
    }

    #[tokio::test]
    async fn environment_is_replaced_not_inherited() {
        // SAFETY: 测试进程里设一个变量，确认它**不会**漏进子进程。
        unsafe { std::env::set_var("CREWD_EXEC_LEAK_PROBE", "leaked") };
        let mut p = spawn_pty(&sh("echo \"[$FOO|$HOME|$CREWD_EXEC_LEAK_PROBE]\"")).unwrap();
        let out = read_to_end(&p.master).await;
        assert!(out.contains("[bar||]"), "{out:?}");
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn child_is_session_leader_with_controlling_tty() {
        let mut p = spawn_pty(&sh("echo via-tty > /dev/tty && echo CTTY-OK; sleep 1")).unwrap();
        let pgid = rustix::process::getpgid(rustix::process::Pid::from_raw(p.pid as i32)).unwrap();
        assert_eq!(
            pgid.as_raw_nonzero().get() as u32,
            p.pid,
            "setsid 后 pgid == pid"
        );
        let out = read_to_end(&p.master).await;
        assert!(
            out.contains("via-tty") && out.contains("CTTY-OK"),
            "{out:?}"
        );
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn sigpipe_is_default_in_the_child() {
        let mut p = spawn_pty(&sh("yes | head -n 1; echo done")).unwrap();
        let out = read_to_end(&p.master).await;
        assert!(out.contains("done"), "{out:?}");
        assert!(
            !out.to_lowercase().contains("broken pipe"),
            "SIGPIPE 被继承成忽略了：{out:?}"
        );
        p.child.wait().await.unwrap();
    }

    /// 读到 `needle` 出现为止，返回读到的全部。
    async fn read_until(master: &PtyMaster, needle: &str) -> String {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while !String::from_utf8_lossy(&out).contains(needle) {
            let n = tokio::time::timeout(Duration::from_secs(10), master.read(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("10s 内没等到 {needle:?}"))
                .unwrap();
            assert!(n > 0, "没等到 {needle:?} 就 EOF 了");
            out.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn resize_reaches_the_child_and_input_is_written() {
        let mut p = spawn_pty(&sh("stty size; read line; echo \"got:$line\"; stty size")).unwrap();
        read_until(&p.master, "25 80").await;
        p.master.resize(101, 33).unwrap();
        p.master.write_all(b"ping\n").await.unwrap();
        let out = read_to_end(&p.master).await;
        assert!(out.contains("got:ping"), "{out:?}");
        assert!(out.contains("33 101"), "resize 后：{out:?}");
        p.child.wait().await.unwrap();
    }

    /// 下一条测试的探针，在单独的测试进程里跑（只在设了环境变量时干活）。整个进程忽略
    /// SIGHUP：panic 展开时主端被 drop，内核给会话首进程发挂断信号——不挡住它，漏出去的
    /// 子进程在干活之前就被带走了，看起来像「没起过」。
    #[test]
    fn master_setup_failure_probe() {
        let Some(marker) = std::env::var_os("CREWD_EXEC_PTY_SETUP_PROBE") else {
            return;
        };
        // SAFETY: 单独的探针进程，只此一条测试在跑；改信号处置不影响别的测试。
        unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
        let spec = sh(&format!(
            "touch '{}'",
            std::path::Path::new(&marker).display()
        ));
        // 运行时之外调：`AsyncFd` 注册 panic（tokio 的子进程收尸注册同样会 panic，但在 fork 之后）。
        let r = std::panic::catch_unwind(|| spawn_pty(&spec).map(|p| p.pid));
        assert!(!matches!(r, Ok(Ok(_))), "前提：运行时之外起不来");
        std::thread::sleep(Duration::from_millis(500));
        println!("PTY-SETUP-PROBE-RAN");
    }

    /// 主端的非阻塞 / reactor 注册失败时子进程不能已经起来了（没人管它、也没人收尸）：
    /// 这些设置都在 spawn 之前做。
    #[test]
    fn master_setup_failure_leaves_no_child_behind() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pty::tests::master_setup_failure_probe",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("CREWD_EXEC_PTY_SETUP_PROBE", &marker)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("PTY-SETUP-PROBE-RAN"),
            "前提：探针跑完了：{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!marker.exists(), "设置失败了，子进程却已经起来跑过了");
    }

    #[tokio::test]
    async fn missing_program_or_cwd_fails_to_spawn() {
        let mut spec = sh("true");
        spec.program = "/nonexistent/claude".into();
        assert!(spawn_pty(&spec).is_err());
        let mut spec = sh("true");
        spec.cwd = "/nonexistent/dir".into();
        assert!(spawn_pty(&spec).is_err());
    }
}

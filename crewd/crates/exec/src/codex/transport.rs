//! 起 `codex app-server` 子进程并接好三根管子（Swift `CodexAppServerConnection` 的移植）。
//!
//! - **新会话**（setsid）：自成进程组、没有控制终端——codex 跑的工具命令碰 `/dev/tty` 会立刻
//!   失败，而不是被 SIGTTIN 停住、把回合挂死；停止时 `killpg` 能连它起的子命令一起收掉。
//! - **环境**：白名单（`env::build_env_from_process`）再剥掉计费 key（[`BILLING_KEYS`]）——
//!   带着 `OPENAI_API_KEY` 起的 codex 会改走按量计费。
//! - **stderr 必须一直读**：codex 往 stderr 写日志，没人读的话 64KB 管道缓冲写满，它就卡在
//!   写上，整个 app-server 停摆（Swift 吃过：「回合永远不结束」只在长输出后出现）。这里一直读，
//!   只留最后一段（[`StderrTail`]）给握手失败的说明用。
//! - **stdout 按行读、有上限**（[`LineReader`]）：一行超过上限就丢掉这一行、报一次，读下一行，
//!   不会把内存吃光。读法是取消安全的（可以放进 `select!`）。
//! - 写 stdin 走单独的写任务（对端不读时写会阻塞，不能卡住驱动循环）。EPIPE 是普通错误：
//!   Rust 运行时启动时就忽略了 SIGPIPE（Swift 那次「写管道被信号打死」在这里不会发生），
//!   子进程那边由 std 在 exec 前把 SIGPIPE 复位成默认。
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// 子进程环境里绝不能有的变量：有了 codex 就不走订阅额度、改走 API key 按量计费
/// （名字取自 codex-cli 0.160.1 二进制里「auth is provided by environment」那组）。
pub const BILLING_KEYS: [&str; 3] = ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"];

/// 一行最多这么多字节（`item/completed` 带全量命令输出，几 MB 是正常的）。
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// stderr 留最后这么多字节。
pub const STDERR_TAIL_BYTES: usize = 4096;

/// 剥掉计费 key。
pub fn strip_billing_keys(env: &mut BTreeMap<String, String>) {
    for key in BILLING_KEYS {
        env.remove(key);
    }
}

/// 怎么起 app-server。
#[derive(Clone, Debug)]
pub struct CodexProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// 整份子进程环境（调用方已经过白名单；计费 key 由 `spawn_codex` 再剥一次，不依赖调用方）。
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

/// 起好的 app-server。
pub struct CodexProcess {
    pub child: Child,
    pub pid: u32,
    pub stdout: LineReader<tokio::io::BufReader<ChildStdout>>,
    /// 往 stdin 写一行（不含换行）。写任务按顺序写，写失败就停。
    pub writer: mpsc::UnboundedSender<String>,
    pub writer_task: JoinHandle<()>,
    pub stderr: StderrTail,
    pub stderr_task: JoinHandle<()>,
}

/// 起 app-server。工作目录不存在 = 失败（不让它静默落到别处）。
pub fn spawn_codex(spec: &CodexProcessSpec) -> io::Result<CodexProcess> {
    if !spec.cwd.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("工作目录 {} 不存在或不是目录", spec.cwd.display()),
        ));
    }
    // 结构性剥计费 key：对自己的那份环境副本再剥一次，调用方漏剥也不会漏到子进程。
    let mut env = spec.env.clone();
    strip_billing_keys(&mut env);
    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(&env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: pre_exec 在 fork 之后、exec 之前运行，只调 async-signal-safe 的 setsid(2)，
    // 不分配内存、不拿锁。
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(io::Error::from)
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("子进程刚起就没了"))?;
    let mut stdin = child.stdin.take().expect("stdin 是 piped");
    let stdout = child.stdout.take().expect("stdout 是 piped");
    let mut stderr = child.stderr.take().expect("stderr 是 piped");

    let (writer, mut lines) = mpsc::unbounded_channel::<String>();
    let writer_task = tokio::spawn(async move {
        while let Some(mut line) = lines.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });

    let tail = StderrTail::default();
    let sink = tail.clone();
    let stderr_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            sink.push(&buf[..n]);
        }
    });

    Ok(CodexProcess {
        child,
        pid,
        stdout: LineReader::new(tokio::io::BufReader::new(stdout), MAX_LINE_BYTES),
        writer,
        writer_task,
        stderr: tail,
        stderr_task,
    })
}

/// stderr 的最后一段（共享、有上限）。
#[derive(Clone, Debug, Default)]
pub struct StderrTail(Arc<Mutex<Vec<u8>>>);

impl StderrTail {
    pub fn push(&self, bytes: &[u8]) {
        let mut b = self.0.lock().expect("stderr 锁中毒");
        b.extend_from_slice(bytes);
        if b.len() > STDERR_TAIL_BYTES {
            let cut = b.len() - STDERR_TAIL_BYTES;
            b.drain(..cut);
        }
    }

    /// 最后几行（UTF-8 有损解码，去掉首尾空白）。
    pub fn text(&self) -> String {
        let b = self.0.lock().expect("stderr 锁中毒");
        String::from_utf8_lossy(&b).trim().to_string()
    }
}

/// 读出来的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Text(String),
    /// 超过上限的一行：已整行丢掉，只报长度（至少这么多字节）。
    TooLong {
        bytes: usize,
    },
}

/// 按 `\n` 切行、有上限的读取器。行尾的 `\r` 留给解析器去掉。
pub struct LineReader<R> {
    inner: R,
    buf: Vec<u8>,
    max: usize,
    /// 正在丢弃一行超长的：已丢了多少字节。
    discarding: Option<usize>,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            max,
            discarding: None,
        }
    }

    /// 下一行；EOF 返回 `None`（EOF 前没有换行的半行也交出来）。
    ///
    /// 取消安全：唯一的 `.await` 是 `fill_buf`，拿到数据后同步地搬进 `self.buf` 并 `consume`，
    /// 中途被 `select!` 丢掉不会丢字节。
    pub async fn next_line(&mut self) -> io::Result<Option<Line>> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                if let Some(bytes) = self.discarding.take() {
                    return Ok(Some(Line::TooLong { bytes }));
                }
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let bytes = std::mem::take(&mut self.buf);
                return Ok(Some(Line::Text(
                    String::from_utf8_lossy(&bytes).into_owned(),
                )));
            }
            let newline = available.iter().position(|b| *b == b'\n');
            let take = newline.unwrap_or(available.len());
            match &mut self.discarding {
                Some(dropped) => *dropped += take,
                None => {
                    if self.buf.len() + take > self.max {
                        self.discarding = Some(self.buf.len() + take);
                        self.buf.clear();
                    } else {
                        self.buf.extend_from_slice(&available[..take]);
                    }
                }
            }
            let consumed = newline.map_or(take, |i| i + 1);
            self.inner.consume(consumed);
            if newline.is_some() {
                if let Some(bytes) = self.discarding.take() {
                    return Ok(Some(Line::TooLong { bytes }));
                }
                let bytes = std::mem::take(&mut self.buf);
                return Ok(Some(Line::Text(
                    String::from_utf8_lossy(&bytes).into_owned(),
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    fn reader(bytes: &'static [u8], max: usize) -> LineReader<&'static [u8]> {
        LineReader::new(bytes, max)
    }

    #[tokio::test]
    async fn lines_are_split_on_newline_and_a_trailing_half_line_is_kept() {
        let mut r = reader(b"{\"a\":1}\r\n\n{\"b\":2}\npartial", 1024);
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"a\":1}\r".into()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text(String::new()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"b\":2}".into()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("partial".into()))
        );
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_overlong_line_is_dropped_whole_and_the_next_one_still_reads() {
        let mut r = reader(b"0123456789ABCDEF\nok\n0123456789", 8);
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::TooLong { bytes: 16 })
        );
        assert_eq!(r.next_line().await.unwrap(), Some(Line::Text("ok".into())));
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::TooLong { bytes: 10 })
        );
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_line_split_across_many_small_writes_comes_out_whole() {
        let (mut w, r) = tokio::io::duplex(8);
        let mut r = LineReader::new(tokio::io::BufReader::with_capacity(3, r), 1024);
        let writer = tokio::spawn(async move {
            for chunk in ["{\"me", "thod\":\"tu", "rn/started\"}\n{\"x\"", ":1}\n"] {
                w.write_all(chunk.as_bytes()).await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"method\":\"turn/started\"}".into()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"x\":1}".into()))
        );
        writer.await.unwrap();
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn being_cancelled_mid_line_loses_no_bytes() {
        let (mut w, r) = tokio::io::duplex(64);
        let mut r = LineReader::new(tokio::io::BufReader::new(r), 1024);
        w.write_all(b"{\"half\":").await.unwrap();
        // 读到半行时被取消（select! 里别的分支先到）。
        let cancelled = tokio::time::timeout(Duration::from_millis(50), r.next_line()).await;
        assert!(cancelled.is_err());
        w.write_all(b"true}\n").await.unwrap();
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"half\":true}".into()))
        );
    }

    #[test]
    fn billing_keys_are_stripped_even_when_passed_explicitly() {
        let mut extra = BTreeMap::new();
        extra.insert("OPENAI_API_KEY".to_string(), "sk-live".to_string());
        extra.insert("CODEX_API_KEY".to_string(), "x".to_string());
        extra.insert("CODEX_ACCESS_TOKEN".to_string(), "t".to_string());
        extra.insert("FAKE_CODEX_MODE".to_string(), "echo".to_string());
        let mut env = crate::env::build_env([("HOME", "/h")], "/usr/bin:/bin", &extra);
        strip_billing_keys(&mut env);
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert!(!env.contains_key("CODEX_API_KEY"));
        assert!(!env.contains_key("CODEX_ACCESS_TOKEN"));
        assert_eq!(env.get("FAKE_CODEX_MODE").map(String::as_str), Some("echo"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/h"));
    }

    fn sh(script: &str, cwd: PathBuf) -> CodexProcessSpec {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        CodexProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env,
            cwd,
        }
    }

    #[tokio::test]
    async fn the_child_leads_its_own_session_without_a_terminal() {
        let mut p = spawn_codex(&sh(
            "echo pid=$$; echo pgid=$(ps -o pgid= -p $$ | tr -d ' '); \
             echo tty=$(ps -o tty= -p $$ | tr -d ' ')",
            std::env::temp_dir(),
        ))
        .unwrap();
        let mut got = Vec::new();
        while let Some(Line::Text(l)) = p.stdout.next_line().await.unwrap() {
            got.push(l);
        }
        assert_eq!(got[0], format!("pid={}", p.pid));
        assert_eq!(got[1], format!("pgid={}", p.pid), "自成进程组");
        assert!(
            got[2] == "tty=??" || got[2] == "tty=?",
            "没有控制终端：{}",
            got[2]
        );
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn a_flood_on_stderr_cannot_stall_stdout_and_its_tail_is_kept() {
        // 1 MiB 写进 stderr：没人读的话子进程卡在第 64KB，stdout 那行永远不来。
        let mut p = spawn_codex(&sh(
            "head -c 1048576 /dev/zero | tr '\\0' 'x' >&2; echo 'last words' >&2; echo done",
            std::env::temp_dir(),
        ))
        .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(10), p.stdout.next_line())
            .await
            .expect("stderr 没被排空，子进程卡住了")
            .unwrap();
        assert_eq!(line, Some(Line::Text("done".into())));
        p.child.wait().await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut p.stderr_task).await;
        let tail = p.stderr.text();
        assert!(
            tail.ends_with("last words"),
            "{}",
            &tail[tail.len().saturating_sub(40)..]
        );
        assert!(tail.len() <= STDERR_TAIL_BYTES);
    }

    #[tokio::test]
    async fn lines_written_reach_stdin_in_order() {
        let mut p = spawn_codex(&sh("head -n 2", std::env::temp_dir())).unwrap();
        p.writer.send("{\"id\":0}".into()).unwrap();
        p.writer.send("{\"id\":1}".into()).unwrap();
        assert_eq!(
            p.stdout.next_line().await.unwrap(),
            Some(Line::Text("{\"id\":0}".into()))
        );
        assert_eq!(
            p.stdout.next_line().await.unwrap(),
            Some(Line::Text("{\"id\":1}".into()))
        );
        p.child.wait().await.unwrap();
        // 对端已经退出：继续写不会把我们打死（EPIPE 只是让写任务停下）。
        let _ = p.writer.send("{\"id\":2}".into());
        assert!(
            tokio::time::timeout(Duration::from_secs(2), &mut p.writer_task)
                .await
                .is_ok(),
            "写任务在对端退出后必须停下"
        );
    }

    /// 剥计费 key 是结构性的：调用方（哪怕忘了剥）把它们放进 `spec.env`，子进程也看不到。
    #[tokio::test]
    async fn spawn_codex_strips_billing_keys_itself() {
        let mut spec = sh(
            "echo \"openai=${OPENAI_API_KEY-unset} codex=${CODEX_API_KEY-unset} \
             token=${CODEX_ACCESS_TOKEN-unset} keep=${KEEP_ME-unset}\"",
            std::env::temp_dir(),
        );
        for key in BILLING_KEYS {
            spec.env.insert(key.to_string(), "sk-secret".to_string());
        }
        spec.env.insert("KEEP_ME".to_string(), "yes".to_string());
        let mut p = spawn_codex(&spec).unwrap();
        assert_eq!(
            p.stdout.next_line().await.unwrap(),
            Some(Line::Text(
                "openai=unset codex=unset token=unset keep=yes".into()
            ))
        );
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_workdir_is_a_spawn_error() {
        let err = spawn_codex(&sh("true", "/nonexistent/crewd-m2c".into()))
            .err()
            .expect("必须报错");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("工作目录"));
    }
}

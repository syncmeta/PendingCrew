//! 测试器材：一个只会画几屏的假 claude，跑在真 PTY 里（`tests/fake_claude.rs` 用）。
//! **不是产品代码**——只长到能让执行器的判据成立：输入框、粘贴折叠、信任框、限流菜单、额度报错。
//!
//! 独立 opt-in 参数：`--guarded-write-fixture DIR` 运行真实 PTY 背压器材；默认模式不变。
//! DIR 必须预先存在并含 marker.bin（8..128 字节唯一可打印 ASCII 标记）。器材 raw/no-echo，
//! 写 slave-ready；按顺序处理 screen-N.bin + screen-N.request（N 从 0 起），输出 flush 后写
//! screen-N.written（父需一边排空 PTY、一边等回执；输出非阻塞且限时）。release-input 写精确 filler 字节数后才开始读 stdin；丢完写 filler-drained，
//! 其余真实字节写 capture.bin，遇 marker 截去标记并写 capture.closed 后退出。协议总限时 30s，
//! 失败写 fixture.error 并 exit 2；正文和 control byte 都只是合成测试数据。
//!
//! 环境变量（执行器只放行调用方显式给的变量，所以测试经 `ClaudeSpec::additional_env` 传）：
//! - `FAKE_CLAUDE_MODE`：`ready`（缺省）/ `late-submit-echo`（CR 提交前的有限迟到回显 gate）/ `deaf`（画框但永不回显）/ `noread`（画框后再也不读
//!   stdin，PTY 输入缓冲会被写满）/ `decision`（普通编号菜单，等待真实人类 RPC 答案）/ `trust`（先过信任框）/ `ratelimit`（第一次提交弹限流菜单）/
//!   `usage`（第一次提交报额度用尽并退出）/ `refuse`（带 `--resume` 起时照真 claude 打印
//!   「No conversation found with session ID: <id>」后 exit 1；不带则同 `ready`）。
//! - `FAKE_CLAUDE_READY_DELAY_MS`：先只画横幅，这段时间收到的按键全部吞掉（真 claude 首屏重绘就这样吞字）。
//! - `FAKE_CLAUDE_REPLAY`：开场先原样回放这份录制字节（如 `tui-claude-trust.bin`）。
//! - `FAKE_CLAUDE_C13_GATE` + `FAKE_CLAUDE_C13_WITNESS`: opt-in ready-only 30s real heartbeat/release and per-generation actual-submit JSONL; synthetic owned paths only.
//! - `FAKE_CLAUDE_LOG`：每次提交往这个文件追加一行（正文里的换行写成 `\n`）。
//!
//! 粘贴判定照 claude（Swift 版在真 daemon 上实测到的行为）：同一次 read 里收到多个字节 = 粘贴，
//! 其中的 CR 当换行进输入框；粘贴之后单独来的回车只「结束粘贴」不提交，要先来一个普通按键
//! 才恢复按键语义——这就是执行器提交时要先发空格、退格再回车的原因。
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Trace {
    file: Arc<Mutex<std::fs::File>>,
    start: Instant,
}

impl Trace {
    fn record(&self, detail: std::fmt::Arguments<'_>) {
        let mut file = self.file.lock().expect("synthetic trace lock");
        writeln!(file, "t_us={} {detail}", self.start.elapsed().as_micros())
            .expect("write synthetic trace");
        file.flush().expect("flush synthetic trace");
    }
}

const HINT: &str = "Try \"write a test\"";

struct Fake {
    mode: String,
    session: String,
    resumed: bool,
    input: String,
    last: String,
    log: Option<String>,
    submits: usize,
    pasting: bool,
}

/// C13-only controller. A generation is a fixture identity, not a Claude conversation id.
/// Receipts follow completed real IO; default fake modes never consult these files.
struct C13 {
    start: Instant,
    gate: std::path::PathBuf,
    witness: std::path::PathBuf,
    session: String,
    agent: String,
    generation: String,
}
static C13_FIXTURE: std::sync::OnceLock<C13> = std::sync::OnceLock::new();
impl C13 {
    fn check(&self) {
        if self.start.elapsed() >= Duration::from_secs(30) {
            let _ = std::fs::write(self.gate.join("fixture.error"), b"C13 30s deadline");
            eprintln!("C13 owned fixture expired");
            std::process::exit(2);
        }
    }
    fn record(&self, mut value: serde_json::Value) {
        self.check();
        value["session_id"] = self.session.clone().into();
        value["agent_id"] = self.agent.clone().into();
        value["generation"] = self.generation.clone().into();
        value["pid"] = std::process::id().into();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.witness)
            .expect("C13 owned witness");
        let bytes = format!("{value}\n");
        assert!(bytes.len() <= 8 * 1024 * 1024, "bounded synthetic witness");
        file.write_all(bytes.as_bytes())
            .expect("C13 complete witness append");
        file.flush().expect("C13 witness flush");
    }
    fn write(&self, bytes: &[u8]) {
        // This flag may be shared with stdin: the opt-in reader handles EAGAIN below.
        let fd = libc::STDOUT_FILENO;
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0);
        assert!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0);
        let mut offset = 0;
        while offset < bytes.len() {
            self.check();
            let end = (offset + 4096).min(bytes.len());
            let n = unsafe { libc::write(fd, bytes[offset..end].as_ptr().cast(), end - offset) };
            if n > 0 {
                offset += n as usize;
                continue;
            }
            if n == 0 {
                panic!("C13 stdout made no progress");
            }
            let e = std::io::Error::last_os_error();
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) {
                std::thread::sleep(Duration::from_millis(2));
            } else {
                panic!("C13 actual stdout error: {e}");
            }
        }
    }
}
fn c13_init(fake: &Fake, args: &[String]) {
    let Ok(root) = std::env::var("FAKE_CLAUDE_C13_GATE") else {
        return;
    };
    assert_eq!(
        fake.mode, "ready",
        "C13 opt-in is independent of recorded/menu modes"
    );
    assert!(!fake.session.is_empty() && fake.session.len() <= 256);
    assert!(
        fake.session
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    );
    let config = args
        .windows(2)
        .find(|w| w[0] == "--mcp-config")
        .expect("actual generated mcp argv");
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config[1]).unwrap()).unwrap();
    let helper_args = mcp["mcpServers"]["crew"]["args"].as_array().unwrap();
    let session = helper_args
        .windows(2)
        .find(|w| w[0] == "--session")
        .and_then(|w| w[1].as_str())
        .expect("actual helper --session identity")
        .to_owned();
    let generation = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let gate = std::path::PathBuf::from(root)
        .join(&fake.session)
        .join(&generation);
    std::fs::create_dir_all(&gate).unwrap();
    let witness_root = std::path::PathBuf::from(
        std::env::var_os("FAKE_CLAUDE_C13_WITNESS").expect("C13 owned witness directory"),
    );
    std::fs::create_dir_all(&witness_root).unwrap();
    // One writer per generation: regular-file partial writes cannot interleave sessions.
    let witness = witness_root.join(format!("{generation}.jsonl"));
    C13_FIXTURE
        .set(C13 {
            start: Instant::now(),
            gate,
            witness,
            session,
            agent: fake.session.clone(),
            generation,
        })
        .ok()
        .expect("one fixture initialization");
    let c = C13_FIXTURE.get().unwrap();
    c.record(serde_json::json!({"event":"launch", "resumed":fake.resumed,
        "mcp":config[1], "gate":c.gate}));
}
fn c13_next(fake: &Fake, rx: &mpsc::Receiver<Vec<u8>>, released: &mut bool) -> Option<Vec<u8>> {
    let Some(c) = C13_FIXTURE.get() else {
        return rx.recv().ok();
    };
    loop {
        c.check();
        if c.gate.join("arm").is_file() && !*released {
            if c.gate.join("release").is_file() {
                fake.draw_box();
                std::fs::write(c.gate.join("released"), b"ready frame actually written").unwrap();
                *released = true;
            } else {
                fake.draw_box();
                if !c.gate.join("busy-written").exists() {
                    std::fs::write(c.gate.join("busy-written"), b"heartbeat actually written")
                        .unwrap();
                }
            }
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => return Some(chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn out(bytes: &[u8]) {
    if let Some(c) = C13_FIXTURE.get() {
        c.write(bytes);
        return;
    }
    let mut o = std::io::stdout();
    let _ = o.write_all(bytes);
    let _ = o.flush();
}

impl Fake {
    fn banner(&self) -> String {
        format!(
            "\x1b[2J\x1b[HClaude Code (fake) session {} resumed={}\r\n",
            self.session, self.resumed
        )
    }

    fn draw_box(&self) {
        // 占位提示与底下的提示条照真 claude 上暗淡样式（SGR 2）：执行器的草稿闸靠样式分辨占位提示和人打的字。
        let shown = if self.input.is_empty() {
            format!("\x1b[2m{HINT}\x1b[22m")
        } else if self.input.contains('\n') || self.input.chars().count() > 60 {
            format!("[Pasted text #1 +{} lines]", self.input.lines().count())
        } else {
            self.input.clone()
        };
        let rule = "─".repeat(40);
        out(format!(
            "{}\r\n{}\r\n\r\n{rule}\r\n\u{276f} {shown}\r\n{rule}\r\n\x1b[2m  ? for shortcuts\x1b[22m",
            self.banner(),
            self.last
        )
        .as_bytes());
    }

    fn draw_trust(&self, cursor: usize) {
        let mark = |i: usize| if i == cursor { "\u{276f}" } else { " " };
        out(format!(
            "\x1b[2J\x1b[HQuick safety check: Is this a project you created or one you trust?\r\n\r\n{}  No, exit\r\n{}  Yes, I trust this folder\r\n\r\nEnter to confirm \u{b7} Esc to cancel",
            mark(0),
            mark(1)
        )
        .as_bytes());
    }

    fn submit(&mut self) {
        let text = std::mem::take(&mut self.input);
        self.submits += 1;
        if let Some(c) = C13_FIXTURE.get() {
            c.record(serde_json::json!({"event":"submit", "submission":self.submits,"body":text}));
        }
        if let Some(path) = &self.log {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("打不开 FAKE_CLAUDE_LOG");
            let _ = writeln!(f, "{}", text.replace('\n', "\\n"));
        }
        let first: String = text.lines().next().unwrap_or("").chars().take(30).collect();
        self.last = format!("\u{23fa} GOT: {first}");
    }
}

/// Standalone deterministic queue-to-syscall fixture. Before release there is no
/// stdin reader (including a background reader), so PTY capacity is real kernel
/// backpressure. File receipts describe completed IO, never elapsed-time guesses.
fn guarded_write_fixture(dir: &std::path::Path) -> std::io::Result<()> {
    use std::fs::{File, OpenOptions};
    use std::io::{Error, ErrorKind};

    const LIMIT: usize = 8 * 1024 * 1024;
    const FRAME_LIMIT: u64 = 1024 * 1024;
    let invalid = |message: &str| Error::new(ErrorKind::InvalidData, message);
    let dir = dir.canonicalize()?;
    if !dir.is_dir() {
        return Err(invalid("guarded fixture DIR is not a directory"));
    }
    let marker = std::fs::read(dir.join("marker.bin"))?;
    if !(8..=128).contains(&marker.len()) || !marker.iter().all(|b| (0x21..=0x7e).contains(b)) {
        return Err(invalid(
            "marker.bin must contain 8..128 printable non-space ASCII bytes",
        ));
    }
    let start = Instant::now();
    let mut protocol = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(dir.join("protocol.log"))?;
    let receipt = |name: &str, text: &str| -> std::io::Result<()> {
        let path = dir.join(name);
        let temporary = dir.join(format!("{name}.tmp"));
        std::fs::write(&temporary, text)?;
        std::fs::rename(temporary, path)
    };
    let record = |log: &mut File, detail: &str| -> std::io::Result<()> {
        writeln!(log, "t_us={} {detail}", start.elapsed().as_micros())?;
        log.flush()
    };
    let stdin_fd = rustix::stdio::stdin();
    let mut term = rustix::termios::tcgetattr(stdin_fd)?;
    term.make_raw();
    rustix::termios::tcsetattr(stdin_fd, rustix::termios::OptionalActions::Now, &term)?;
    // PTY slave clones may share the open-file description: making stdout
    // nonblocking can also affect stdin, so its read path handles WouldBlock.
    // SAFETY: fcntl accesses this process's valid stdout fd; flags are preserved.
    let flags = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_GETFL) };
    if flags < 0 {
        return Err(Error::last_os_error());
    }
    // SAFETY: F_SETFL takes the existing flags plus O_NONBLOCK, no pointer.
    if unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Error::last_os_error());
    }
    // make_raw removes ECHO; require the actual configuration call to succeed.
    receipt("slave-ready", "raw_no_echo=1\nstdin_reader_started=0\n")?;
    record(&mut protocol, "slave-ready")?;

    let mut next_screen = 0u64;
    let mut filler_total = None;
    let mut filler_left = 0usize;
    let mut input = std::io::stdin();
    let mut capture = File::create(dir.join("capture.bin"))?;
    let mut captured = Vec::new();
    let mut read_seq = 0u64;
    loop {
        if start.elapsed() >= Duration::from_secs(30) {
            return Err(Error::new(
                ErrorKind::TimedOut,
                "guarded fixture 30s protocol deadline",
            ));
        }
        // This channel remains independent of stdin, also after release. Exactly
        // one sequential request per loop keeps input/drain and deadline fair.
        let request = dir.join(format!("screen-{next_screen}.request"));
        if request.exists() {
            let source = dir.join(format!("screen-{next_screen}.bin"));
            if source.metadata()?.len() > FRAME_LIMIT {
                return Err(invalid("guarded fixture screen exceeds 1MiB"));
            }
            let frame = std::fs::read(source)?;
            let mut offset = 0usize;
            while offset < frame.len() {
                if start.elapsed() >= Duration::from_secs(30) {
                    return Err(Error::new(
                        ErrorKind::TimedOut,
                        "guarded fixture screen output deadline",
                    ));
                }
                let end = (offset + 4096).min(frame.len());
                // SAFETY: the slice stays allocated throughout this nonblocking
                // syscall; stdout is valid and count is within the slice bounds.
                let written = unsafe {
                    libc::write(
                        libc::STDOUT_FILENO,
                        frame[offset..end].as_ptr().cast::<libc::c_void>(),
                        end - offset,
                    )
                };
                if written > 0 {
                    offset += written as usize;
                } else if written == 0 {
                    return Err(Error::new(
                        ErrorKind::WriteZero,
                        "guarded fixture stdout made no progress",
                    ));
                } else {
                    let error = Error::last_os_error();
                    match error.kind() {
                        ErrorKind::Interrupted => continue,
                        ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                        _ => return Err(error),
                    }
                }
            }
            // Receipt means the entire frame reached the PTY, not that the
            // parent consumed it. Parent must drain while waiting for this file.
            receipt(
                &format!("screen-{next_screen}.written"),
                &format!("bytes={}\n", frame.len()),
            )?;
            record(
                &mut protocol,
                &format!("screen-written seq={next_screen} bytes={}", frame.len()),
            )?;
            next_screen += 1;
        }
        if filler_total.is_none() && dir.join("release-input").exists() {
            let release = std::fs::read_to_string(dir.join("release-input"))?;
            let count = release
                .trim()
                .parse::<usize>()
                .map_err(|_| invalid("release-input is not a decimal byte count"))?;
            if count > LIMIT {
                return Err(invalid("filler exceeds 8MiB"));
            }
            filler_total = Some(count);
            filler_left = count;
            record(
                &mut protocol,
                &format!("release-input filler_bytes={count}"),
            )?;
            if count == 0 {
                receipt("filler-drained", "bytes=0\n")?;
            }
        }
        if filler_total.is_none() {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        // Single reader, raw VMIN=1, bounded poll. No extra process, shell,
        // provider, auth probe or background stdin consumer is involved.
        let mut poll = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll points to one initialized pollfd for this process's stdin.
        let ready = unsafe { libc::poll(&mut poll, 1, 2) };
        if ready < 0 {
            let error = Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready == 0 {
            continue;
        }
        let mut bytes = [0u8; 4096];
        let n = match input.read(&mut bytes) {
            Ok(0) => {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "stdin closed before capture marker",
                ));
            }
            Ok(n) => n,
            Err(error)
                if matches!(error.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        read_seq += 1;
        let discard = filler_left.min(n);
        filler_left -= discard;
        record(
            &mut protocol,
            &format!(
                "input-read seq={read_seq} bytes={n} filler_discard={discard} captured={}",
                n - discard
            ),
        )?;
        if discard > 0 && filler_left == 0 {
            receipt(
                "filler-drained",
                &format!("bytes={}\n", filler_total.unwrap()),
            )?;
            record(&mut protocol, "filler-drained")?;
        }
        let received = &bytes[discard..n];
        if captured.len().saturating_add(received.len()) > LIMIT {
            return Err(invalid("capture exceeds 8MiB"));
        }
        let search_from = captured.len().saturating_sub(marker.len() - 1);
        captured.extend_from_slice(received);
        capture.write_all(received)?;
        capture.flush()?;
        if let Some(relative) = captured[search_from..]
            .windows(marker.len())
            .position(|w| w == marker)
        {
            let end = search_from + relative;
            let trailing = captured.len() - end - marker.len();
            capture.set_len(end as u64)?;
            capture.flush()?;
            receipt(
                "capture.closed",
                &format!(
                    "captured_bytes={end}\nfiller_bytes={}\ntrailing_bytes={trailing}\n",
                    filler_total.unwrap()
                ),
            )?;
            record(
                &mut protocol,
                &format!("capture-closed bytes={end} trailing={trailing}"),
            )?;
            return Ok(());
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(dir) = value_of("--guarded-write-fixture") {
        let path = std::path::Path::new(&dir);
        if let Err(error) = guarded_write_fixture(path) {
            let _ = std::fs::write(path.join("fixture.error"), format!("{error}\n"));
            eprintln!("guarded-write-fixture: {error}");
            std::process::exit(2);
        }
        return;
    }
    let resumed = value_of("--resume");
    let mut fake = Fake {
        mode: std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "ready".into()),
        session: resumed
            .clone()
            .or_else(|| value_of("--session-id"))
            .unwrap_or_default(),
        resumed: resumed.is_some(),
        input: String::new(),
        last: String::new(),
        log: std::env::var("FAKE_CLAUDE_LOG").ok(),
        submits: 0,
        pasting: false,
    };

    c13_init(&fake, &args);

    if fake.mode == "refuse" && fake.resumed {
        out(format!(
            "No conversation found with session ID: {}\r\n",
            fake.session
        )
        .as_bytes());
        std::process::exit(1);
    }

    let stdin = rustix::stdio::stdin();
    if let Ok(mut t) = rustix::termios::tcgetattr(stdin) {
        t.make_raw();
        let _ = rustix::termios::tcsetattr(stdin, rustix::termios::OptionalActions::Now, &t);
    }
    if fake.mode == "noread" {
        fake.draw_box();
        std::thread::sleep(Duration::from_secs(600));
        return;
    }
    let trace = std::env::var("FAKE_CLAUDE_TRACE").ok().map(|path| Trace {
        file: Arc::new(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("open synthetic trace"),
        )),
        start: Instant::now(),
    });
    let reader_trace = trace.clone();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut input = std::io::stdin();
        let mut seq = 0u64;
        loop {
            if let Some(c) = C13_FIXTURE.get() {
                c.check();
            }
            let n = match input.read(&mut buf) {
                Ok(n) => n,
                Err(e)
                    if C13_FIXTURE.get().is_some()
                        && matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) =>
                {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(_) => return,
            };
            seq += 1;
            if let Some(trace) = &reader_trace {
                trace.record(format_args!(
                    "read seq={seq} len={n} cr={} bs={} space={}",
                    buf[..n].contains(&b'\r'),
                    buf[..n].contains(&0x7f),
                    buf[..n].contains(&b' '),
                ));
            }
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                return;
            }
        }
    });

    if let Ok(path) = std::env::var("FAKE_CLAUDE_REPLAY") {
        let bytes = std::fs::read(&path).expect("读不到 FAKE_CLAUDE_REPLAY");
        for chunk in bytes.chunks(64) {
            out(chunk);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    if fake.mode == "trust" {
        if std::env::var("FAKE_CLAUDE_REPLAY").is_err() {
            fake.draw_trust(0);
        }
        let mut cursor = 0;
        loop {
            let Ok(chunk) = rx.recv() else { return };
            if chunk.windows(3).any(|w| w == b"\x1b[B") {
                cursor = 1;
                fake.draw_trust(cursor);
            } else if chunk.windows(3).any(|w| w == b"\x1b[A") {
                cursor = 0;
                fake.draw_trust(cursor);
            } else if chunk.contains(&b'\r') {
                if cursor == 0 {
                    return;
                }
                break;
            }
        }
    }

    let delay: u64 = std::env::var("FAKE_CLAUDE_READY_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if delay > 0 {
        out(fake.banner().as_bytes());
        let until = Instant::now() + Duration::from_millis(delay);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            let _ = rx.recv_timeout(left);
        }
    }
    fake.draw_box();
    if fake.mode == "deaf" {
        loop {
            if rx.recv().is_err() {
                return;
            }
        }
    }

    // 一个 UTF-8 字符可能被切在两次 read 之间：没收全的那几个字节留到下一块。
    let chunk_delay = Duration::from_millis(
        std::env::var("FAKE_CLAUDE_CHUNK_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    );
    let mut partial: Vec<u8> = Vec::new();
    let mut consume_seq = 0u64;
    let mut c13_released = false;
    while let Some(chunk) = c13_next(&fake, &rx, &mut c13_released) {
        if !chunk_delay.is_zero() {
            std::thread::sleep(chunk_delay);
        }
        consume_seq += 1;
        let paste = chunk.len() > 1;
        if let Some(trace) = &trace {
            trace.record(format_args!(
                "consume seq={consume_seq} len={} paste={paste} pasting_before={} cr={} bs={} space={}",
                chunk.len(), fake.pasting, chunk.contains(&b'\r'),
                chunk.contains(&0x7f), chunk.contains(&b' '),
            ));
        }
        if paste {
            fake.pasting = true;
        } else if chunk != b"\r" {
            fake.pasting = false;
        }
        if let Some(trace) = &trace {
            trace.record(format_args!(
                "consume-state seq={consume_seq} pasting_after={}",
                fake.pasting,
            ));
        }
        partial.extend_from_slice(&chunk);
        let valid = match std::str::from_utf8(&partial) {
            Ok(_) => partial.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => partial.len(),
        };
        let rest = partial.split_off(valid);
        let text = String::from_utf8_lossy(&partial).into_owned();
        partial = rest;
        let mut submitted = false;
        for c in text.chars() {
            match c {
                '\r' if paste => fake.input.push('\n'),
                // 粘贴之后单独来的回车：只结束粘贴。
                '\r' if fake.pasting => {}
                '\r' => {
                    if fake.mode == "late-submit-echo" && fake.submits == 0 {
                        let gate = std::path::PathBuf::from(
                            std::env::var("FAKE_CLAUDE_SUBMIT_GATE")
                                .expect("late-submit-echo requires its synthetic gate"),
                        );
                        // CR was actually read as an ordinary submit key. Delay only
                        // this commit, not the reader or body consumption. Render an
                        // additional nonempty paste row without altering the payload.
                        fake.input.push('\n');
                        fake.draw_box();
                        fake.input.pop();
                        std::fs::write(gate.join("held-cr"), b"received; not submitted")
                            .expect("write synthetic CR receipt");
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while !gate.join("release-submit").exists() {
                            if Instant::now() >= deadline {
                                eprintln!("synthetic submit gate expired without release");
                                std::process::exit(2);
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    fake.submit();
                    submitted = true;
                }
                '\u{7f}' => {
                    fake.input.pop();
                }
                '\u{15}' => fake.input.clear(),
                c if c == '\n' || !c.is_control() => fake.input.push(c),
                _ => {}
            }
        }
        if submitted && fake.submits == 1 && fake.mode == "usage" {
            out(b"\r\nYou've reached your usage limit \xc2\xb7 resets 5pm\r\n");
            std::thread::sleep(Duration::from_millis(200));
            std::process::exit(1);
        }
        if submitted && fake.submits == 1 && fake.mode == "ratelimit" {
            out("\x1b[2J\x1b[HWhat do you want to do?\r\n\r\n\u{276f} 1. Stop and wait for limit to reset\r\n  2. Upgrade your plan\r\n\r\nEnter to confirm".as_bytes());
            let mut answer = String::new();
            while let Ok(chunk) = rx.recv() {
                answer.push_str(&String::from_utf8_lossy(&chunk));
                if answer.contains('\r') {
                    break;
                }
            }
            fake.input = format!("RATE-ANSWER:{}", answer.trim_end_matches('\r'));
            fake.submit();
        }
        if submitted && fake.submits == 1 && fake.mode == "decision" {
            out("\x1b[2J\x1b[HWhich synthetic option?\r\n\r\n\u{276f} 1. Synthetic first\r\n  2. Synthetic second\r\n\r\nEnter to confirm".as_bytes());
            let mut answer = String::new();
            while let Ok(chunk) = rx.recv() {
                answer.push_str(&String::from_utf8_lossy(&chunk));
                // Numbered menus accept the displayed digit immediately; the
                // production answer_plan deliberately sends no Enter for them.
                if answer.chars().any(|c| c == '1' || c == '2') {
                    break;
                }
            }
            fake.input = format!("DECISION-ANSWER:{}", answer.trim_end_matches('\r'));
            fake.submit();
        }
        fake.draw_box();
    }
}

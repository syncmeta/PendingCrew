//! 测试器材：一个只会画几屏的假 claude，跑在真 PTY 里（`tests/fake_claude.rs` 用）。
//! **不是产品代码**——只长到能让执行器的判据成立：输入框、粘贴折叠、信任框、限流菜单、额度报错。
//!
//! 环境变量（执行器只放行调用方显式给的变量，所以测试经 `ClaudeSpec::additional_env` 传）：
//! - `FAKE_CLAUDE_MODE`：`ready`（缺省）/ `deaf`（画框但永不回显）/ `noread`（画框后再也不读
//!   stdin，PTY 输入缓冲会被写满）/ `trust`（先过信任框）/ `ratelimit`（第一次提交弹限流菜单）/
//!   `usage`（第一次提交报额度用尽并退出）。
//! - `FAKE_CLAUDE_READY_DELAY_MS`：先只画横幅，这段时间收到的按键全部吞掉（真 claude 首屏重绘就这样吞字）。
//! - `FAKE_CLAUDE_REPLAY`：开场先原样回放这份录制字节（如 `tui-claude-trust.bin`）。
//! - `FAKE_CLAUDE_LOG`：每次提交往这个文件追加一行（正文里的换行写成 `\n`）。
//!
//! 粘贴判定照 claude（Swift 版在真 daemon 上实测到的行为）：同一次 read 里收到多个字节 = 粘贴，
//! 其中的 CR 当换行进输入框；粘贴之后单独来的回车只「结束粘贴」不提交，要先来一个普通按键
//! 才恢复按键语义——这就是执行器提交时要先发空格、退格再回车的原因。
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

fn out(bytes: &[u8]) {
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
        let shown = if self.input.is_empty() {
            HINT.to_string()
        } else if self.input.contains('\n') || self.input.chars().count() > 60 {
            format!("[Pasted text #1 +{} lines]", self.input.lines().count())
        } else {
            self.input.clone()
        };
        let rule = "─".repeat(40);
        out(format!(
            "{}\r\n{}\r\n\r\n{rule}\r\n\u{276f} {shown}\r\n{rule}\r\n  ? for shortcuts",
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value_of = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
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
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut input = std::io::stdin();
        while let Ok(n) = input.read(&mut buf) {
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
    let mut partial: Vec<u8> = Vec::new();
    while let Ok(chunk) = rx.recv() {
        let paste = chunk.len() > 1;
        if paste {
            fake.pasting = true;
        } else if chunk != b"\r" {
            fake.pasting = false;
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
        fake.draw_box();
    }
}

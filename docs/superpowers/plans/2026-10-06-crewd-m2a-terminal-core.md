# crewd M2a 终端核心 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建纯计算 crate `crewd-term`：alacritty_terminal 无头终端（终端回复、读屏、滚动区域、焦点上报）+ claude 对话框/输入框识别 + 待决策跟踪 + 快照编码，用仓库里已有的 4 个 claude TUI 夹具做回归。

**Architecture:** `crewd/crates/term`（包名 `crewd-term`，库名 `crewd_term`）不依赖其它 crewd crate，也暂不被它们依赖（M2b 接 PTY 时才接线）。`Screen` 包住 `Term<Replies>` + `Processor<NoSync>` + 一个旁路 vte 解析器（跟踪 alacritty 不公开的滚动区域、`?1004h`）；读屏规则在 `Screen` 上，菜单/输入框判据是作用在 `&[String]` 上的纯函数；跟踪器的时间由调用方传入；快照编码器只读 `Screen`。公开 API 除 `Rgb` 外不暴露 alacritty 类型——spec §13 Q12 要求执行器只经窄接口与其余部分交互，以后拆成独立进程只换传输层。

**Tech Stack:** Rust 1.94（edition 2024）、alacritty_terminal 0.26（`default-features = false`；vte 0.15 经它 re-export）。没有别的依赖：不用 tokio、不用 serde。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（§3.3 M2、§13 Q12、§14）；移植图与仿真器选型 `docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md`（§2、§3、§7、§8、§9）。Swift 对照：`Sources/Mac/LocalRunner/{TerminalScreenText,SessionPendingDecision,StartupPromptDelivery,SessionHealth,TerminalSnapshotEncoder}.swift`。

> 写计划时已把本计划的全部代码在一个只含 `crates/term` 的临时 workspace 里按任务顺序重放：每个任务结束 `check.sh` 全绿（单测数依次 16/25/34/57/67/67/88/93，另 11 条夹具测试），各「确认失败」步骤都真红过，Task 7/8 的变异步骤也逐条核过会红。执行时数字对不上就停下查，别改期望凑数。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（已加进 `~/.zshrc`）。若 `cargo` 找不到，先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`。
- 依赖一律用 `cargo add` 加，**不要手写版本号**；唯一的依赖用 `cargo add -p crewd-term alacritty_terminal@0.26 --no-default-features` 锁在 spike 验过的 0.26 线上。`Cargo.lock` 要提交。
- `crewd-term` 是纯计算：不起进程、不碰 tokio、不做 IO、不调 `Instant::now()`（测试除外；时间一律由调用方传入）。测试读夹具是唯一的文件 IO。
- 公开 API 不暴露 alacritty 类型（`Rgb` 是唯一的 re-export）。逐格比对之类要碰 alacritty 内部的测试放在 crate 内单测里。
- 本 crate 没有线协议类型，不加 serde；M1 的「线协议字段一律 camelCase」规矩到 M2d 定 attach 协议时再适用。
- 测试**只用合成数据**加 4 个已入库的夹具（`Tests/Fixtures/*.bin`，公开仓库里本来就有的合成录制，原样拷进 `crates/term/tests/fixtures/`）。夹具缺失必须失败，绝不静默跳过。
- 验收标准是 Swift `ClaudeInputBoxFixtureTests` 那 13 条夹具期望 + 快照逐格往返，**不是**与 SwiftTerm 逐字节一致（SwiftTerm 在 64 字节边界切开 OSC 标题时会把标题残片漏进网格，见移植图 §9）。
- 每个任务结束跑 `crewd/scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。提交只 `git add` 该任务列出的文件（路径相对 `crewd/`，git 认相对路径），提交信息末尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`。
- 在分支 `crewd-m2a` 上做（执行时用 worktree）。

## 范围

做：`Screen`（喂字节、终端回复发件箱、resize、光标、滚动区域）、读屏（TerminalScreenText 口径）、`AnsiPlainTextTail`、菜单解析（`PendingDecision`）、输入框判据（`input_row` / `body_tail_visible` / `blocking_dialog`）、`PendingDecisionTracker`、快照编码器、夹具回归。

不做（M2b–M2d）：PTY 启动、claude 启动参数与 hooks、开场投递状态机（含 `landed()`）、健康/限流扫描器（只移植它们的底座 `AnsiPlainTextTail`）、codex、生命周期与 resume、attach 的 socket 协议、群通知文案（`SessionDecisionNotice` 归 M3；它依赖的 `numbered` 事实字段在本计划里钉住）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **OSC 10/11 的回答**取 `ScreenConfig.foreground/background`，缺省白字黑底——与今天 SwiftTerm 报的黑底一致（claude 据此走暗色主题）。OSC 4 按 xterm 默认 256 色答，免得程序等一个永远不来的回复。
2. **焦点**：每见到一次 `?1004h` 就往发件箱补一个 `ESC[I`（SwiftTerm 的行为），不是只在「关→开」边沿补。实测 `tui-claude-ready.bin` 里 claude 开了两次。
3. **时钟注入** = `poll(now: Instant, screen)` 的参数，与 Swift 的 `poll(now:)` 同构；不引入 `Clock` trait。
4. **读屏**：alacritty 没写过的格本来就是空格（不是 NUL）；`\0`、`\t` 也读成空格；跳过 `WIDE_CHAR_SPACER`；`LEADING_WIDE_CHAR_SPACER` 按空格读。
5. **快照**：alacritty 里「没写过」与「默认属性的空格」是同一个 `Cell`，所以不需要 Swift 版那套 ECH/补位再擦掉的技巧——默认格直接写空格。软换行靠把整行写满让终端自己折，续行至少写一格；最底一行悬空的折行（竖向缩小后才有）用 2..lines 滚动区域折一次再 SD 滚回来（Task 8）。另外还原 Swift 版没管的：快照时刻的画笔（SGR + OSC 8 超链接）、INSERT、LNM、ORIGIN、APP_KEYPAD、关掉的 LINE_WRAP / ALTERNATE_SCROLL / URGENCY_HINTS。
6. **尾窗**：一个 UTF-8 字符被切在两次 `feed` 之间时拼起来（Swift 版会解成两个 U+FFFD）。
7. **resize** 忽略 `cols < 2 || lines < 1`（同 Swift）。
8. 快照往返要逐格比 alacritty 的 `Cell`，所以往返测试是 crate 内单测（`src/snapshot.rs` 的 `tests`、`src/snapshot_fixtures.rs`），夹具用 `include_bytes!`；13 条夹具期望只用公开 API，放 `tests/claude_fixtures.rs`。

## Review Focus

1. PTY 输出在任意字节处被切开（UTF-8 字符中间、转义序列中间、OSC 标题中间）→ 屏幕、对话框判据、尾窗与整块喂完全一致。（Task 1 `chunking_anywhere_gives_the_same_screen`、Task 3 `utf8_split_across_feeds_is_reassembled`、Task 6 `every_fixture_renders_the_same_whatever_the_chunking`）
2. 从更矮的窗口 attach（权威终端被竖向缩小，最底一行的折行续行被挤出屏幕）→ 快照照样还原折行标志，之后再 resize 两边 reflow 一致。（Task 8 `dangling_wrap_on_the_bottom_row_survives`、`fixtures_round_trip_after_resizes`）
3. 快照时画笔还带着底色 → 快照里换行引起的上滚不把底色染进新行。（Task 7 `background_does_not_bleed_into_scrolled_lines`）
4. 调用方传进来的 `now` 比上一次还早（时钟调整、乱序调用）→ 不提前报待决策。（Task 5 `clock_going_backwards_does_not_fire_early`）
5. 拍完快照程序接着输出 → 客户端那台终端的结果与权威终端逐格一致（画笔、模式、光标、pending-wrap 都还原了）。（Task 8 `stream_continues_identically_after_a_snapshot`）

## 文件结构

```
crewd/crates/term/
  Cargo.toml                 cargo new + cargo add 生成
  src/lib.rs                 mod 声明 + 公开 re-export（每个任务追加）
  src/screen.rs              Screen / ScreenConfig / CursorPos、终端回复发件箱、读屏      (T1, T2, T7)
  src/side.rs                旁路 vte 解析：滚动区域、?1004h                             (T2)
  src/tail.rs                AnsiPlainTextTail                                          (T3)
  src/menu.rs                PendingDecision、parse_menu 及其零件                        (T4)
  src/input_box.rs           blocking_dialog / input_row / body_tail_visible             (T4)
  src/tracker.rs             PendingDecisionTracker / DecisionEvent / STABLE_WINDOW      (T5)
  src/snapshot.rs            Snapshot 编码器 + 合成往返测试                               (T7, T8)
  src/snapshot_fixtures.rs   #[cfg(test)] 真语料往返                                     (T8)
  tests/claude_fixtures.rs   13 条夹具期望 + 待决策出口 + 切块无关                         (T6)
  tests/fixtures/*.bin       从仓库根 Tests/Fixtures/ 原样拷来的 4 个夹具                 (T6)
crewd/README.md              加一行 crate 说明                                           (T1)
docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md  加 §15 交付记录 (T8)
```

---

### Task 1: crate 骨架 + `Screen`（终端回复、resize、光标、读屏）

**Files:**
- Create: `crewd/crates/term/Cargo.toml`（`cargo new` + `cargo add`）、`crewd/crates/term/src/lib.rs`、`crewd/crates/term/src/screen.rs`
- Modify: `crewd/Cargo.lock`、`crewd/README.md`

**Interfaces:**
- Consumes: 无（alacritty_terminal 0.26）。
- Produces（crate `crewd_term`）:
  - `pub use alacritty_terminal::vte::ansi::Rgb;`（`Rgb { r: u8, g: u8, b: u8 }`）
  - `pub struct ScreenConfig { pub cols: usize, pub lines: usize, pub scrollback: usize, pub foreground: Rgb, pub background: Rgb }`，`Default` = 80×25、10_000、白字黑底
  - `pub struct CursorPos { pub line: usize, pub col: usize, pub pending_wrap: bool, pub visible: bool }`（`Copy + Eq + Debug`）
  - `impl Screen`：`new(ScreenConfig) -> Screen`、`feed(&mut self, &[u8])`、`take_replies(&mut self) -> Vec<u8>`、`resize(&mut self, cols: usize, lines: usize) -> bool`、`cols(&self) -> usize`、`lines(&self) -> usize`、`history_size(&self) -> usize`、`cursor(&self) -> CursorPos`、`screen_rows(&self) -> Vec<String>`、`screen_text(&self, max_lines: usize) -> String`、`history_rows(&self) -> Vec<String>`
  - crate 内：`pub(crate) struct Replies`（alacritty 的 `EventListener`）、`pub(crate) struct NoSync`、`pub(crate) fn color_for_index(usize, Rgb, Rgb) -> Rgb`

- [ ] **Step 1: 建 crate、加依赖**

```bash
cargo new --lib crates/term --name crewd-term --vcs none
cargo add -p crewd-term alacritty_terminal@0.26 --no-default-features
cat crates/term/Cargo.toml
```

Expected（cargo 在 workspace 里会自动用 workspace 继承的字段；若不是，手工改成下面这样）：

```toml
[package]
name = "crewd-term"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
alacritty_terminal = { version = "0.26", default-features = false }
```

- [ ] **Step 2: 写失败的测试**

`crates/term/src/lib.rs`（整个文件）：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod screen;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use screen::{CursorPos, Screen, ScreenConfig};
```

`crates/term/src/screen.rs`：先只放测试模块（下一步实现时把实现写在它**上面**）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Screen {
        Screen::new(ScreenConfig::default())
    }

    #[test]
    fn default_config_is_80x25_with_10k_scrollback() {
        let s = screen();
        assert_eq!((s.cols(), s.lines()), (80, 25));
        let mut s = screen();
        for n in 0..10_100 {
            s.feed(format!("line {n}\r\n").as_bytes());
        }
        assert_eq!(s.history_size(), 10_000);
        assert_eq!(s.screen_rows()[0], "line 10076");
    }

    #[test]
    fn answers_device_queries_into_the_outbox() {
        let mut s = screen();
        s.feed(b"\x1b[c");
        assert_eq!(s.take_replies(), b"\x1b[?6c");
        assert!(s.take_replies().is_empty(), "取完即空");
        s.feed(b"\x1b[3;7H\x1b[6n");
        assert_eq!(s.take_replies(), b"\x1b[3;7R");
        s.feed(b"\x1b[18t");
        assert_eq!(s.take_replies(), b"\x1b[8;25;80t");
        s.feed(b"\x1b[14t");
        assert_eq!(s.take_replies(), b"\x1b[4;400;1280t");
    }

    #[test]
    fn osc_color_queries_use_configured_colors() {
        let mut s = Screen::new(ScreenConfig {
            background: Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            },
            ..ScreenConfig::default()
        });
        s.feed(b"\x1b]11;?\x07");
        assert_eq!(s.take_replies(), b"\x1b]11;rgb:1212/3434/5656\x07");
        s.feed(b"\x1b]10;?\x1b\\");
        assert_eq!(s.take_replies(), b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\");
        s.feed(b"\x1b]4;1;?\x07");
        assert_eq!(s.take_replies(), b"\x1b]4;1;rgb:cdcd/0000/0000\x07");
    }

    #[test]
    fn palette_matches_xterm_defaults() {
        let fg = Rgb { r: 1, g: 2, b: 3 };
        let bg = Rgb { r: 4, g: 5, b: 6 };
        assert_eq!(color_for_index(16, fg, bg), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(
            color_for_index(196, fg, bg),
            Rgb {
                r: 0xff,
                g: 0,
                b: 0
            }
        );
        assert_eq!(
            color_for_index(231, fg, bg),
            Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff
            }
        );
        assert_eq!(color_for_index(232, fg, bg), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            color_for_index(255, fg, bg),
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
        assert_eq!(color_for_index(256, fg, bg), fg);
        assert_eq!(color_for_index(257, fg, bg), bg);
        assert_eq!(color_for_index(258, fg, bg), fg);
    }

    #[test]
    fn synchronized_output_is_drawn_immediately() {
        let mut s = screen();
        s.feed(b"\x1b[?2026hHELLO");
        assert_eq!(s.screen_rows(), vec!["HELLO"]);
    }

    #[test]
    fn chunking_anywhere_gives_the_same_screen() {
        let bytes = "\x1b[1;31m我在\x1b[0m，\x1b]0;标题\x07attach\x1b[2;5Hok".as_bytes();
        let mut whole = screen();
        whole.feed(bytes);
        let mut bytewise = screen();
        for b in bytes {
            bytewise.feed(std::slice::from_ref(b));
        }
        assert_eq!(whole.screen_rows(), bytewise.screen_rows());
        assert_eq!(whole.screen_rows(), vec!["我在，attach", "    ok"]);
    }

    #[test]
    fn resize_reflows_and_updates_size_replies() {
        let mut s = screen();
        s.feed(format!("{}END", "x".repeat(100)).as_bytes());
        assert!(s.resize(160, 30));
        assert_eq!(s.screen_rows(), vec![format!("{}END", "x".repeat(100))]);
        s.feed(b"\x1b[18t");
        assert_eq!(s.take_replies(), b"\x1b[8;30;160t");
    }

    #[test]
    fn degenerate_resize_is_ignored() {
        let mut s = screen();
        assert!(!s.resize(1, 25));
        assert!(!s.resize(80, 0));
        assert_eq!((s.cols(), s.lines()), (80, 25));
    }

    #[test]
    fn cursor_reports_pending_wrap() {
        let mut s = screen();
        s.feed("x".repeat(80).as_bytes());
        let c = s.cursor();
        assert_eq!(
            (c.line, c.col, c.pending_wrap, c.visible),
            (0, 79, true, true)
        );
        s.feed(b"\x1b[?25l");
        assert!(!s.cursor().visible);
    }

    // ---- 读屏口径（TerminalScreenText 移植）----

    #[test]
    fn skipped_cells_read_as_spaces() {
        let mut s = screen();
        s.feed(b"\x1b[2J\x1b[1;1HClaude\x1b[1;13Hv2.1.260");
        assert_eq!(s.screen_rows(), vec!["Claude      v2.1.260"]);
    }

    #[test]
    fn wide_chars_are_not_split() {
        let mut s = screen();
        s.feed("\x1b[2J\x1b[1;1H我在，attach 探针测试中。".as_bytes());
        assert_eq!(s.screen_rows(), vec!["我在，attach 探针测试中。"]);
    }

    #[test]
    fn wide_chars_and_gaps_mix_on_one_row() {
        let mut s = screen();
        s.feed("\x1b[2J\x1b[1;1H我在\x1b[1;9Hattach 中".as_bytes());
        assert_eq!(s.screen_rows(), vec!["我在    attach 中"]);
    }

    #[test]
    fn trailing_blanks_and_blank_rows_are_dropped() {
        let mut s = screen();
        s.feed(b"\x1b[2J\x1b[1;1Hhi\x1b[3;1H   ");
        assert_eq!(s.screen_rows(), vec!["hi"]);
        s.feed(b"\x1b[2;1H\tx");
        assert_eq!(s.screen_rows(), vec!["hi", "        x"]);
    }

    #[test]
    fn combining_marks_stay_with_their_base() {
        let mut s = screen();
        s.feed("e\u{301}x".as_bytes());
        assert_eq!(s.screen_rows(), vec!["e\u{301}x"]);
    }

    #[test]
    fn screen_text_takes_the_tail() {
        let mut s = screen();
        s.feed(b"a\r\nb\r\nc");
        assert_eq!(s.screen_text(2), "b\nc");
        assert_eq!(s.screen_text(0), "");
    }

    #[test]
    fn history_rows_are_oldest_first() {
        let mut s = screen();
        for n in 0..30 {
            s.feed(format!("line {n}\r\n").as_bytes());
        }
        let history = s.history_rows();
        assert_eq!(history.len(), 6);
        assert_eq!(history[0], "line 0");
        assert_eq!(history[5], "line 5");
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-term`
Expected: 编译失败，``cannot find type `Screen` in this scope`` 之类。

- [ ] **Step 4: 写实现**

把下面这段放在 `crates/term/src/screen.rs` 的测试模块**之前**：

```rust
//! 权威终端：alacritty_terminal 的薄封装。只做三件事——喂字节、把终端自己的
//! 回复（DA1/DSR/OSC 11/XTWINOPS…）收进发件箱让调用方写回 PTY、读屏。
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb, Timeout};

/// 屏幕参数。缺省值 = 今天 Swift 版的 PTY：80×25、回滚 10000 行、黑底白字。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenConfig {
    pub cols: usize,
    pub lines: usize,
    pub scrollback: usize,
    /// OSC 10 的回答。
    pub foreground: Rgb,
    /// OSC 11 的回答——claude 据此选明暗主题。黑 = 与 Swift 版（SwiftTerm 报黑）一致。
    pub background: Rgb,
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            lines: 25,
            scrollback: 10_000,
            foreground: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            background: Rgb { r: 0, g: 0, b: 0 },
        }
    }
}

/// 光标。`pending_wrap` = 刚写满最后一列、下一个字符才真正换行（alacritty 此时报
/// 列 = cols-1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorPos {
    pub line: usize,
    pub col: usize,
    pub pending_wrap: bool,
    pub visible: bool,
}

/// 同步输出（`?2026`）不缓冲：alacritty 默认把 `?2026h` 之后最多 150ms 的字节攒着不画，
/// 那段时间里读屏/拍快照会漏。no-op 超时 = 立即处理（spike 已验证）。
#[derive(Default)]
pub(crate) struct NoSync;

impl Timeout for NoSync {
    fn set_timeout(&mut self, _: Duration) {}
    fn clear_timeout(&mut self) {}
    fn pending_timeout(&self) -> bool {
        false
    }
}

struct Outbox {
    bytes: Vec<u8>,
    size: WindowSize,
    foreground: Rgb,
    background: Rgb,
}

/// alacritty 的事件出口。只收「要写回 PTY」的三种，其余（标题、响铃…）丢掉。
#[derive(Clone)]
pub(crate) struct Replies(Arc<Mutex<Outbox>>);

impl Replies {
    fn lock(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        let mut outbox = self.lock();
        match event {
            Event::PtyWrite(text) => outbox.bytes.extend_from_slice(text.as_bytes()),
            Event::ColorRequest(index, format) => {
                let rgb = color_for_index(index, outbox.foreground, outbox.background);
                outbox.bytes.extend_from_slice(format(rgb).as_bytes());
            }
            Event::TextAreaSizeRequest(format) => {
                let text = format(outbox.size);
                outbox.bytes.extend_from_slice(text.as_bytes());
            }
            _ => {}
        }
    }
}

fn window_size(cols: usize, lines: usize) -> WindowSize {
    WindowSize {
        num_lines: lines as u16,
        num_cols: cols as u16,
        cell_width: 16,
        cell_height: 16,
    }
}

/// OSC 4/10/11/12 的回答。0..=255 用 xterm 默认调色板；256 前景、257 背景、258 光标。
pub(crate) fn color_for_index(index: usize, foreground: Rgb, background: Rgb) -> Rgb {
    const BASE16: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    if index == NamedColor::Background as usize {
        return background;
    }
    if index >= NamedColor::Foreground as usize {
        return foreground;
    }
    if index < 16 {
        let (r, g, b) = BASE16[index];
        return Rgb { r, g, b };
    }
    if index < 232 {
        let step = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
        let i = index - 16;
        return Rgb {
            r: step(i / 36),
            g: step((i / 6) % 6),
            b: step(i % 6),
        };
    }
    let level = (8 + (index - 232) * 10) as u8;
    Rgb {
        r: level,
        g: level,
        b: level,
    }
}

struct Size {
    cols: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// 一台无头终端。纯计算：不起进程、不读时钟、不碰 IO。
pub struct Screen {
    term: Term<Replies>,
    parser: Processor<NoSync>,
    replies: Replies,
}

impl Screen {
    pub fn new(config: ScreenConfig) -> Self {
        let replies = Replies(Arc::new(Mutex::new(Outbox {
            bytes: Vec::new(),
            size: window_size(config.cols, config.lines),
            foreground: config.foreground,
            background: config.background,
        })));
        let term_config = Config {
            scrolling_history: config.scrollback,
            ..Config::default()
        };
        let size = Size {
            cols: config.cols,
            lines: config.lines,
        };
        let term = Term::new(term_config, &size, replies.clone());
        Self {
            term,
            parser: Processor::new(),
            replies,
        }
    }

    /// 喂一段 PTY 输出。可以在任意字节处切块（转义序列、UTF-8 中间都行）。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// 取走终端攒下的回复字节（调用方原样写回 PTY）。取完即空。
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies.lock().bytes)
    }

    /// 改尺寸（alacritty 会 reflow）。`cols < 2 || lines < 1` 忽略并返回 false。
    pub fn resize(&mut self, cols: usize, lines: usize) -> bool {
        if cols < 2 || lines < 1 {
            return false;
        }
        self.term.resize(Size { cols, lines });
        self.replies.lock().size = window_size(cols, lines);
        true
    }

    pub fn cols(&self) -> usize {
        self.term.columns()
    }

    pub fn lines(&self) -> usize {
        self.term.screen_lines()
    }

    /// 回滚区里有多少行（不含当前屏）。
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn cursor(&self) -> CursorPos {
        let cursor = &self.term.grid().cursor;
        CursorPos {
            line: cursor.point.line.0.max(0) as usize,
            col: cursor.point.column.0,
            pending_wrap: cursor.input_needs_wrap,
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        }
    }

    /// 当前屏逐行文本（TerminalScreenText 口径），尾部空行去掉。
    pub fn screen_rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = (0..self.lines())
            .map(|line| self.line_text(line as i32))
            .collect();
        while rows.last().is_some_and(|row| row.trim().is_empty()) {
            rows.pop();
        }
        rows
    }

    /// 当前屏文本，最多 `max_lines` 行（取尾部），`\n` 连接。
    pub fn screen_text(&self, max_lines: usize) -> String {
        let rows = self.screen_rows();
        let start = rows.len().saturating_sub(max_lines);
        rows[start..].join("\n")
    }

    /// 回滚区逐行文本，最旧的在前。
    pub fn history_rows(&self) -> Vec<String> {
        let history = self.history_size() as i32;
        (1..=history).rev().map(|i| self.line_text(-i)).collect()
    }

    /// 一行格子 → 一行文本：宽字符后半格丢掉；`\0`/`\t` 当空格；组合字符跟在主字符后；
    /// 行尾空格去掉、行首保留。`line` 为负 = 回滚区。
    fn line_text(&self, line: i32) -> String {
        let grid = self.term.grid();
        let row = &grid[Line(line)];
        let mut out = String::with_capacity(grid.columns());
        for col in 0..grid.columns() {
            let cell: &Cell = &row[Column(col)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            out.push(match cell.c {
                '\0' | '\t' => ' ',
                c => c,
            });
            if let Some(zerowidth) = cell.zerowidth() {
                out.extend(zerowidth.iter());
            }
        }
        let trimmed = out.trim_end_matches(' ').len();
        out.truncate(trimmed);
        out
    }
}
```

要点：`Processor<NoSync>` 让 `?2026h` 之后的字节立刻生效（默认的 `StdSyncHandler` 会攒最多 150ms，读屏/快照会漏）；DA1/DSR/CPR/18t 由 alacritty 自己经 `Event::PtyWrite` 答，14t 走 `TextAreaSizeRequest`（16×16 像素格，同 Swift），OSC 颜色查询走 `ColorRequest`，由我们按裁定 1 回答。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `16 passed`。

- [ ] **Step 6: README 加一行**

在 `crewd/README.md` 的 `` - 检查：`scripts/check.sh` `` 那一行**下面**插入：

```markdown
- 终端核心：`crates/term`（alacritty_terminal 无头终端：读屏、claude 对话框/输入框识别、待决策跟踪、快照；纯计算，不起进程、不用 tokio）
```

- [ ] **Step 7: 全量检查并提交**

Run: `./scripts/check.sh` → 退出码 0。

```bash
git add Cargo.lock README.md crates/term/Cargo.toml crates/term/src/lib.rs crates/term/src/screen.rs
git commit -m "crewd-term: headless Screen over alacritty_terminal with reply outbox and screen text

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: 旁路解析——滚动区域与焦点上报

alacritty 的 `Term::scroll_region` 是私有字段，快照又必须还原它；alacritty 也不像 SwiftTerm 那样在 `?1004h` 时立刻报 focus-in，而 claude 会开焦点上报。用同一个 vte 状态机再过一遍字节，只认 `CSI t;b r`、`ESC c`、`CSI ? … 1004 … h`，规则逐条照抄 alacritty 0.26（`set_scrolling_region`：`top` 缺省/0 当 1，`bottom` 缺省/0 当行数，`top >= bottom` 忽略，两端夹到行数以内；`reset_state` 与 `resize` 复位成整屏）。

**Files:**
- Create: `crewd/crates/term/src/side.rs`
- Modify: `crewd/crates/term/src/lib.rs`、`crewd/crates/term/src/screen.rs`

**Interfaces:**
- Consumes: Task 1 的 `Screen`（私有字段 `term`/`parser`/`replies`、`Replies::lock()`、`Outbox.bytes`）。
- Produces:
  - `Screen::scroll_region(&self) -> (usize, usize)`：0 起，上界含、下界不含，整屏 = `(0, lines)`
  - `Screen::feed` 每见一次 `?1004h` 往发件箱追加 `b"\x1b[I"`（排在本块其它回复之后）
  - crate 内：`pub(crate) struct SideTracker { new(lines) / advance(&[u8]) / resize(lines) / region() -> (usize, usize) / take_focus_requests() -> usize }`

- [ ] **Step 1: 写失败的测试（旁路解析器本身）**

`crates/term/src/side.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(bytes: &[u8]) -> SideTracker {
        let mut t = SideTracker::new(25);
        t.advance(bytes);
        t
    }

    #[test]
    fn full_screen_by_default() {
        assert_eq!(tracked(b"").region(), (0, 25));
    }

    #[test]
    fn decstbm_sets_and_resets_region() {
        assert_eq!(tracked(b"\x1b[3;20r").region(), (2, 20));
        assert_eq!(tracked(b"\x1b[3;20r\x1b[r").region(), (0, 25));
        assert_eq!(tracked(b"\x1b[5r").region(), (4, 25));
        assert_eq!(tracked(b"\x1b[0;10r").region(), (0, 10));
        assert_eq!(tracked(b"\x1b[3;99r").region(), (2, 25));
    }

    #[test]
    fn invalid_region_is_ignored() {
        assert_eq!(tracked(b"\x1b[3;20r\x1b[9;9r").region(), (2, 20));
        assert_eq!(tracked(b"\x1b[3;20r\x1b[10;2r").region(), (2, 20));
    }

    #[test]
    fn private_r_is_not_decstbm() {
        assert_eq!(tracked(b"\x1b[?3;20r").region(), (0, 25));
    }

    #[test]
    fn ris_and_resize_reset_region() {
        assert_eq!(tracked(b"\x1b[3;20r\x1bc").region(), (0, 25));
        let mut t = tracked(b"\x1b[3;20r");
        t.resize(40);
        assert_eq!(t.region(), (0, 40));
    }

    #[test]
    fn split_sequences_are_tracked() {
        let mut t = SideTracker::new(25);
        t.advance(b"\x1b[");
        t.advance(b"3;2");
        t.advance(b"0r");
        assert_eq!(t.region(), (2, 20));
    }

    #[test]
    fn counts_focus_requests() {
        let mut t = tracked(b"\x1b[?1004h\x1b[?2004;1004h\x1b[?1004l");
        assert_eq!(t.take_focus_requests(), 2);
        assert_eq!(t.take_focus_requests(), 0);
    }
}
```

`crates/term/src/lib.rs` 整个换成：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod screen;
mod side;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use screen::{CursorPos, Screen, ScreenConfig};
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-term`
Expected: 编译失败，``cannot find type `SideTracker` ``。

- [ ] **Step 3: 实现 `SideTracker`**

放在 `crates/term/src/side.rs` 的测试模块之前：

```rust
//! 旁路解析：alacritty 不公开、但我们必须知道的两件事。
//! - 滚动区域（`Term::scroll_region` 是私有字段）——快照要还原它。
//! - `?1004h`——SwiftTerm 打开焦点上报时立刻发 `ESC[I`，alacritty 不发，claude 在等它。
//!
//! 用同一个 vte 状态机再过一遍字节，规则逐条照抄 alacritty 0.26 的处理
//! （`set_scrolling_region`、`reset_state`、`resize`），保证两边对滚动区域的看法一致。
use alacritty_terminal::vte::{Params, Parser, Perform};

pub(crate) struct SideTracker {
    parser: Parser,
    state: SideState,
}

struct SideState {
    lines: usize,
    /// 0 起，`top` 含、`bottom` 不含——与 alacritty 的 `Line(top)..Line(bottom)` 同义。
    region: (usize, usize),
    focus_requests: usize,
}

impl SideTracker {
    pub(crate) fn new(lines: usize) -> Self {
        Self {
            parser: Parser::new(),
            state: SideState {
                lines,
                region: (0, lines),
                focus_requests: 0,
            },
        }
    }

    pub(crate) fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    /// alacritty 的 `Term::resize` 无条件把滚动区域复位成整屏。
    pub(crate) fn resize(&mut self, lines: usize) {
        self.state.lines = lines;
        self.state.region = (0, lines);
    }

    pub(crate) fn region(&self) -> (usize, usize) {
        self.state.region
    }

    /// 自上次调用以来见到几次 `?1004h`。取完清零。
    pub(crate) fn take_focus_requests(&mut self) -> usize {
        std::mem::take(&mut self.state.focus_requests)
    }
}

impl Perform for SideState {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        // 与 alacritty 相同的门槛：被截断的序列一律不处理。
        if ignore || intermediates.len() > 2 {
            return;
        }
        match (action, intermediates) {
            ('r', []) => {
                let mut iter = params.iter();
                let top = match iter.next() {
                    Some(&[p, ..]) if p != 0 => p as usize,
                    _ => 1,
                };
                let bottom = iter
                    .next()
                    .map(|p| p[0] as usize)
                    .filter(|&p| p != 0)
                    .unwrap_or(self.lines);
                if top >= bottom {
                    return;
                }
                self.region = ((top - 1).min(self.lines), bottom.min(self.lines));
            }
            ('h', [b'?']) => {
                for param in params.iter() {
                    if param[0] == 1004 {
                        self.focus_requests += 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        // RIS（`ESC c`）复位滚动区域。
        if !ignore && intermediates.is_empty() && byte == b'c' {
            self.region = (0, self.lines);
        }
    }
}
```

- [ ] **Step 4: 写失败的测试（接到 `Screen` 上）**

在 `crates/term/src/screen.rs` 的测试模块里、`// ---- 读屏口径（TerminalScreenText 移植）----` 那一行**之前**插入：

```rust
    #[test]
    fn focus_reporting_gets_an_immediate_focus_in() {
        let mut s = screen();
        s.feed(b"\x1b[?1004h");
        assert_eq!(s.take_replies(), b"\x1b[I");
        s.feed(b"\x1b[?1004l\x1b[c");
        assert_eq!(s.take_replies(), b"\x1b[?6c");
    }

    /// 旁路记下的滚动区域必须和 alacritty 实际用的那个一致：在区域底部换行，
    /// 只有区域内的行上滚，区域外的行纹丝不动。
    #[test]
    fn tracked_region_matches_alacritty_behaviour() {
        let mut s = screen();
        for n in 0..25 {
            s.feed(format!("\x1b[{};1Hrow{n}", n + 1).as_bytes());
        }
        s.feed(b"\x1b[3;5r");
        assert_eq!(s.scroll_region(), (2, 5));
        s.feed(b"\x1b[5;1H\n");
        let rows = s.screen_rows();
        assert_eq!(&rows[0..6], ["row0", "row1", "row3", "row4", "", "row5"]);
        assert!(s.resize(80, 30));
        assert_eq!(s.scroll_region(), (0, 30));
    }
```

Run: `cargo test -p crewd-term`
Expected: 编译失败，``no method named `scroll_region` found``。

- [ ] **Step 5: 把旁路解析器接进 `Screen`**

在 `crates/term/src/screen.rs` 里做下面 5 处替换（每处旧文本在文件里都只出现一次）：

**替换 1/5（引入旁路解析器）**——把

```rust
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb, Timeout};
```

换成

```rust
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb, Timeout};

use crate::side::SideTracker;
```

**替换 2/5（`Screen` 加字段）**——把

```rust
    replies: Replies,
}

impl Screen {
```

换成

```rust
    replies: Replies,
    side: SideTracker,
}

impl Screen {
```

**替换 3/5（`new` 里建旁路解析器）**——把

```rust
        let term = Term::new(term_config, &size, replies.clone());
        Self {
            term,
            parser: Processor::new(),
            replies,
        }
```

换成

```rust
        let term = Term::new(term_config, &size, replies.clone());
        let side = SideTracker::new(config.lines);
        Self {
            term,
            parser: Processor::new(),
            replies,
            side,
        }
```

**替换 4/5（`feed` 也喂旁路解析器并补 focus-in）**——把

```rust
        self.parser.advance(&mut self.term, bytes);
    }
```

换成

```rust
        self.parser.advance(&mut self.term, bytes);
        self.side.advance(bytes);
        let focus_requests = self.side.take_focus_requests();
        if focus_requests > 0 {
            let mut outbox = self.replies.lock();
            for _ in 0..focus_requests {
                // 我们就是「有焦点的那个窗口」：SwiftTerm 在 `?1004h` 时立刻报 focus-in。
                outbox.bytes.extend_from_slice(b"\x1b[I");
            }
        }
    }
```

**替换 5/5（`resize` 复位滚动区域，并加 `scroll_region()`）**——把

```rust
        self.term.resize(Size { cols, lines });
        self.replies.lock().size = window_size(cols, lines);
        true
    }
```

换成

```rust
        self.term.resize(Size { cols, lines });
        self.side.resize(lines);
        self.replies.lock().size = window_size(cols, lines);
        true
    }

    /// 滚动区域：0 起，上界含、下界不含。整屏 = `(0, lines)`。
    pub fn scroll_region(&self) -> (usize, usize) {
        self.side.region()
    }
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `25 passed`。

- [ ] **Step 7: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/src/side.rs crates/term/src/lib.rs crates/term/src/screen.rs
git commit -m "crewd-term: side-track scroll region and answer ?1004h with focus-in

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: `AnsiPlainTextTail`（去 ANSI 的字节尾窗）

Swift `SessionHealth.swift:137-230` 的移植：状态机 normal / ESC / CSI / OSC（BEL 或 `ESC \` 结束）/ 两字节 ESC（`( ) * + # %`），可见字节（含 UTF-8 ≥0x80）入窗，CR/LF → `\n`，其余控制字符丢掉；窗口按**字符**计，超过 2×limit 才裁回 limit；ASCII 小写字节镜像用来匹配短语。唯一差别见裁定 6。

**Files:**
- Create: `crewd/crates/term/src/tail.rs`
- Modify: `crewd/crates/term/src/lib.rs`

**Interfaces:**
- Consumes: 无。
- Produces: `pub struct AnsiPlainTextTail`（`Clone + Debug + Default`）：`pub const DEFAULT_LIMIT: usize = 4096`、`new() -> Self`、`with_limit(usize) -> Self`、`feed(&mut self, &[u8]) -> bool`（true = 有新明文入窗）、`tail(&self) -> &str`、`contains_lowered_ascii(&self, needle: &[u8]) -> bool`、`lowered_needle(&str) -> Vec<u8>`（关联函数）、`clear(&mut self)`（保留 ANSI 状态与未完的 UTF-8 半截）。

- [ ] **Step 1: 写失败的测试**

`crates/term/src/tail.rs` 先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fed(chunks: &[&[u8]]) -> AnsiPlainTextTail {
        let mut t = AnsiPlainTextTail::new();
        for c in chunks {
            t.feed(c);
        }
        t
    }

    #[test]
    fn strips_csi_osc_and_two_byte_escapes() {
        let t = fed(&[b"\x1b[1;31mred\x1b[0m \x1b]0;title\x07ok\x1b]8;;x\x1b\\!\x1b(Bz\x1b7y"]);
        assert_eq!(t.tail(), "red ok!zy");
    }

    #[test]
    fn cr_and_lf_become_newlines_and_controls_vanish() {
        let t = fed(&[b"a\r\nb\x07\x08\x7fc"]);
        assert_eq!(t.tail(), "a\n\nbc");
    }

    #[test]
    fn sequences_split_across_feeds_are_stripped() {
        let t = fed(&[
            b"he\x1b[3",
            b"8;5;200ml",
            b"lo\x1b]0;ti",
            b"tle\x1b",
            b"\\!",
        ]);
        assert_eq!(t.tail(), "hello!");
    }

    #[test]
    fn returns_false_when_only_control_bytes_arrive() {
        let mut t = AnsiPlainTextTail::new();
        assert!(!t.feed(b"\x1b[2J\x1b[H"));
        assert!(t.feed(b"x"));
    }

    #[test]
    fn utf8_split_across_feeds_is_reassembled() {
        let bytes = "我在".as_bytes();
        let mut t = AnsiPlainTextTail::new();
        assert!(!t.feed(&bytes[..2]));
        assert!(t.feed(&bytes[2..4]));
        assert!(t.feed(&bytes[4..]));
        assert_eq!(t.tail(), "我在");
    }

    #[test]
    fn window_is_trimmed_back_to_limit_after_doubling() {
        let mut t = AnsiPlainTextTail::with_limit(4);
        t.feed(b"abcdefgh");
        assert_eq!(t.tail(), "abcdefgh");
        t.feed("ij我".as_bytes());
        assert_eq!(t.tail(), "hij我");
        assert!(t.contains_lowered_ascii(b"hij"));
        assert!(!t.contains_lowered_ascii(b"abc"));
    }

    #[test]
    fn lowered_mirror_matches_case_insensitively() {
        let t = fed(&[b"Stop And WAIT for limit to reset"]);
        let needle = AnsiPlainTextTail::lowered_needle("stop and wait for limit to reset");
        assert!(t.contains_lowered_ascii(&needle));
        assert!(!t.contains_lowered_ascii(b""));
        assert!(!t.contains_lowered_ascii(b"Stop"));
    }

    #[test]
    fn clear_keeps_escape_state() {
        let mut t = fed(&[b"abc\x1b[3"]);
        t.clear();
        assert_eq!(t.tail(), "");
        t.feed(b"1mx");
        assert_eq!(t.tail(), "x");
    }

    #[test]
    fn incomplete_suffix_detection() {
        assert_eq!(incomplete_utf8_suffix(b"abc"), 0);
        assert_eq!(incomplete_utf8_suffix(&"我".as_bytes()[..1]), 1);
        assert_eq!(incomplete_utf8_suffix(&"我".as_bytes()[..2]), 2);
        assert_eq!(incomplete_utf8_suffix("我".as_bytes()), 0);
        assert_eq!(incomplete_utf8_suffix(&"😀".as_bytes()[..3]), 3);
        assert_eq!(incomplete_utf8_suffix(b"\x80\x80"), 0);
    }
}
```

`crates/term/src/lib.rs` 整个换成：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod screen;
mod side;
mod tail;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use screen::{CursorPos, Screen, ScreenConfig};
pub use tail::AnsiPlainTextTail;
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-term`
Expected: 编译失败，``cannot find type `AnsiPlainTextTail` ``。

- [ ] **Step 3: 写实现**

放在 `crates/term/src/tail.rs` 的测试模块之前：

```rust
//! 去 ANSI 的滚动明文尾窗（Swift `AnsiPlainTextTail` 的移植）：喂 PTY 原始字节，
//! 剥 CSI / OSC / 两字节 ESC 序列与控制字符，CR/LF 一律变 `\n`，保留最近一段明文。
//! 匹配走 ASCII 小写字节镜像（短语表都是 ASCII，按字节比又快又准）。
//!
//! 与 Swift 版的一处差别：一个 UTF-8 字符被切在两次 `feed` 之间时，这里把前半截
//! 留到下一次拼起来；Swift 版会把它解成两个 U+FFFD。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnsiState {
    Normal,
    Esc,
    SkipOne,
    Csi,
    Osc,
    OscEsc,
}

#[derive(Clone, Debug)]
pub struct AnsiPlainTextTail {
    state: AnsiState,
    tail: String,
    tail_chars: usize,
    lowered: Vec<u8>,
    pending_utf8: Vec<u8>,
    limit: usize,
}

impl Default for AnsiPlainTextTail {
    fn default() -> Self {
        Self::with_limit(Self::DEFAULT_LIMIT)
    }
}

impl AnsiPlainTextTail {
    /// 尾窗上限（字符）。超过 2 倍才裁回 `limit`，保证不会切在刚到、还没匹配过的短语中间。
    pub const DEFAULT_LIMIT: usize = 4096;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limit(limit: usize) -> Self {
        Self {
            state: AnsiState::Normal,
            tail: String::new(),
            tail_chars: 0,
            lowered: Vec::new(),
            pending_utf8: Vec::new(),
            limit,
        }
    }

    /// 喂一段原始字节；返回是否有新明文入窗（false = 全是控制序列）。
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        let mut plain = std::mem::take(&mut self.pending_utf8);
        let had_pending = !plain.is_empty();
        let mut added = false;
        for &b in bytes {
            match self.state {
                AnsiState::Normal => {
                    if b == 0x1b {
                        self.state = AnsiState::Esc;
                    } else if b >= 0x20 && b != 0x7f {
                        plain.push(b);
                        added = true;
                    } else if b == b'\n' || b == b'\r' {
                        plain.push(b'\n');
                        added = true;
                    }
                }
                AnsiState::Esc => {
                    self.state = match b {
                        b'[' => AnsiState::Csi,
                        b']' => AnsiState::Osc,
                        b'(' | b')' | b'*' | b'+' | b'#' | b'%' => AnsiState::SkipOne,
                        _ => AnsiState::Normal,
                    };
                }
                AnsiState::SkipOne => self.state = AnsiState::Normal,
                AnsiState::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.state = AnsiState::Normal;
                    }
                }
                AnsiState::Osc => {
                    if b == 0x07 {
                        self.state = AnsiState::Normal;
                    } else if b == 0x1b {
                        self.state = AnsiState::OscEsc;
                    }
                }
                AnsiState::OscEsc => {
                    self.state = if b == b'\\' {
                        AnsiState::Normal
                    } else {
                        AnsiState::Osc
                    };
                }
            }
        }
        if !added && !had_pending {
            return false;
        }
        let keep = incomplete_utf8_suffix(&plain);
        self.pending_utf8 = plain.split_off(plain.len() - keep);
        if plain.is_empty() {
            return false;
        }
        let text = String::from_utf8_lossy(&plain);
        self.tail_chars += text.chars().count();
        self.tail.push_str(&text);
        self.lowered
            .extend(text.bytes().map(|b| b.to_ascii_lowercase()));
        if self.tail_chars > self.limit * 2 {
            let cut = self
                .tail
                .char_indices()
                .nth(self.tail_chars - self.limit)
                .map_or(self.tail.len(), |(i, _)| i);
            self.tail.drain(..cut);
            self.tail_chars = self.limit;
            self.lowered = self.tail.bytes().map(|b| b.to_ascii_lowercase()).collect();
        }
        true
    }

    /// 当前明文尾窗。
    pub fn tail(&self) -> &str {
        &self.tail
    }

    /// 在 ASCII 小写镜像里找子串。`needle` 必须已小写（用 [`Self::lowered_needle`] 转）。
    pub fn contains_lowered_ascii(&self, needle: &[u8]) -> bool {
        !needle.is_empty() && self.lowered.windows(needle.len()).any(|w| w == needle)
    }

    /// 把短语转成 `contains_lowered_ascii` 用的字节（A–Z → a–z，其余原样）。
    pub fn lowered_needle(phrase: &str) -> Vec<u8> {
        phrase.bytes().map(|b| b.to_ascii_lowercase()).collect()
    }

    /// 清空尾窗。ANSI 状态与未完的 UTF-8 半截保留——序列可能正跨在清空点上。
    pub fn clear(&mut self) {
        self.tail.clear();
        self.tail_chars = 0;
        self.lowered.clear();
    }
}

/// 末尾那个没写完的 UTF-8 字符有几个字节（0 = 末尾是完整的）。
fn incomplete_utf8_suffix(bytes: &[u8]) -> usize {
    let start = bytes.len().saturating_sub(3);
    for i in (start..bytes.len()).rev() {
        let b = bytes[i];
        if b & 0xc0 == 0x80 {
            continue;
        }
        let need = match b {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => return 0,
        };
        let have = bytes.len() - i;
        return if have < need { have } else { 0 };
    }
    0
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `34 passed`。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/src/tail.rs crates/term/src/lib.rs
git commit -m "crewd-term: port AnsiPlainTextTail with cross-feed UTF-8 reassembly

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 菜单解析 + 输入框判据

`SessionPendingDecision.swift:64-247`（`TerminalMenuParser`）与 `StartupPromptDelivery.swift:267-340`（`ClaudeInputBox`）的移植，判据逐条照搬（见各函数注释）。两处只是 Rust 化：序号只认 ASCII 数字（Swift 的 `isNumber` 收进来的非 ASCII 数字随后 `Int()` 也解析不了，结果相同）；`strip` 返回借用的 `&str`。

**Files:**
- Create: `crewd/crates/term/src/menu.rs`、`crewd/crates/term/src/input_box.rs`
- Modify: `crewd/crates/term/src/lib.rs`

**Interfaces:**
- Consumes: 无（纯字符串函数；输入就是 `Screen::screen_rows()` 的结果）。
- Produces:
  - `pub struct PendingDecision { pub prompt: String, pub options: Vec<String>, pub numbered: bool }`（`Clone + Debug + Eq`），`fingerprint(&self) -> String`（`prompt|opt1|opt2…`，不含 `numbered`）
  - `pub fn parse_menu(plain: &str) -> Option<PendingDecision>`、`pub fn strip(&str) -> &str`、`pub fn is_confirm_footer(&str) -> bool`
  - 常量：`CURSOR_MARKERS: [char; 4]`、`BOX_CHARS: [char; 20]`、`TRAILING_SLACK = 2`、`PROMPT_SCAN_DEPTH = 12`、`RATE_LIMIT_MENU_PHRASES: [&str; 2]`、`INPUT_MARKERS: [char; 3]`、`INPUT_SCAN_DEPTH = 15`、`BODY_TAIL_CHARS = 64`
  - `pub fn blocking_dialog(rows: &[String]) -> Option<PendingDecision>`、`pub fn input_row(rows: &[String]) -> Option<String>`、`pub fn body_tail_visible(prompt: &str, rows: &[String]) -> bool`
  - crate 内：`pub(crate) struct MenuOption { index, text, selected }`、`pub(crate) fn parse_option(&str) -> Option<MenuOption>`

- [ ] **Step 1: 写失败的测试**

`crates/term/src/menu.rs` 先只放测试模块（菜单形状取自 Swift `SessionPendingDecisionTests`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn permission_menu() -> String {
        [
            "╭──────────────────────────────────────────────╮",
            "│ Bash command                                 │",
            "│                                              │",
            "│   rm -rf build                               │",
            "│   Remove the build directory                 │",
            "│                                              │",
            "│ Do you want to proceed?                      │",
            "│ ❯ 1. Yes                                     │",
            "│   2. Yes, and don't ask again for rm commands │",
            "│   3. No, and tell Claude what to do (esc)    │",
            "╰──────────────────────────────────────────────╯",
        ]
        .join("\n")
    }

    fn unnumbered_trust_dialog() -> String {
        [
            "Quick safety check: Is this a project you created or one you trust? (Like your",
            "own code, a well-known open source project, or work from your team). If not,",
            "take a moment to review what's in this folder first.",
            "",
            "Claude Code'll be able to read, edit, and execute files here.",
            "",
            "Security guide",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]
        .join("\n")
    }

    #[test]
    fn parses_permission_menu() {
        let d = parse_menu(&permission_menu()).unwrap();
        assert_eq!(d.prompt, "Do you want to proceed?");
        assert_eq!(
            d.options,
            [
                "Yes",
                "Yes, and don't ask again for rm commands",
                "No, and tell Claude what to do (esc)"
            ]
        );
        assert!(d.numbered);
    }

    #[test]
    fn parses_plan_menu() {
        let menu = [
            "Would you like to proceed?",
            "❯ 1. Yes, and auto-accept edits",
            "  2. Yes, and manually approve edits",
            "  3. No, keep planning",
        ]
        .join("\n");
        let d = parse_menu(&menu).unwrap();
        assert_eq!(d.prompt, "Would you like to proceed?");
        assert_eq!(d.options.len(), 3);
    }

    #[test]
    fn parses_trust_folder_menu() {
        let menu = "Do you trust the files in this folder?\n❯ 1. Yes, proceed\n  2. No, exit";
        let d = parse_menu(menu).unwrap();
        assert_eq!(d.prompt, "Do you trust the files in this folder?");
        assert_eq!(d.options, ["Yes, proceed", "No, exit"]);
    }

    #[test]
    fn marker_on_second_option_still_parses() {
        let menu = "Do you want to proceed?\n  1. Yes\n❯ 2. No";
        assert_eq!(parse_menu(menu).unwrap().options, ["Yes", "No"]);
    }

    #[test]
    fn parses_unnumbered_trust_dialog() {
        let d = parse_menu(&unnumbered_trust_dialog()).unwrap();
        assert_eq!(d.options, ["No, exit", "Yes, I trust this folder"]);
        assert!(!d.numbered);
        assert!(
            d.prompt.contains("trust?") || d.prompt.contains("safety check"),
            "{}",
            d.prompt
        );
    }

    #[test]
    fn ready_input_box_is_not_a_dialog() {
        let ready = [
            "╭──────────────────────────────────────────────╮",
            "│ ❯ Try \"how do I log an error?\"               │",
            "╰──────────────────────────────────────────────╯",
            "  auto mode on · ? for shortcuts",
        ]
        .join("\n");
        assert_eq!(parse_menu(&ready), None);
    }

    #[test]
    fn multi_line_input_box_is_not_a_dialog() {
        let typing = [
            "╭──────────────────────────────────────────────╮",
            "│ ❯ 先看一下现状                                │",
            "│   再决定怎么改                                │",
            "╰──────────────────────────────────────────────╯",
            "  auto mode on · ? for shortcuts",
        ]
        .join("\n");
        assert_eq!(parse_menu(&typing), None);
    }

    #[test]
    fn prose_numbered_list_is_not_a_menu() {
        assert_eq!(
            parse_menu("我打算这么做：\n1. 先查现状\n2. 再写单测\n3. 最后接线"),
            None
        );
    }

    #[test]
    fn answered_menu_followed_by_output_is_not_pending() {
        let after = permission_menu()
            + "\n⏺ Bash(rm -rf build)\n  ⎿  (No content)\n⏺ 删好了，继续下一步。\n正在读取 src/main.swift …";
        assert_eq!(parse_menu(&after), None);
    }

    #[test]
    fn rate_limit_menu_is_excluded() {
        let menu = "What do you want to do?\n❯ 1. Stop and wait for limit to reset\n  2. Upgrade your plan";
        assert_eq!(parse_menu(menu), None);
    }

    #[test]
    fn single_option_is_not_a_menu() {
        assert_eq!(parse_menu("Continue?\n❯ 1. Yes\n"), None);
    }

    #[test]
    fn non_consecutive_numbers_are_rejected() {
        assert_eq!(parse_menu("见下：\n❯ 1. 甲\n  3. 丙\n"), None);
    }

    #[test]
    fn cursor_list_needs_exactly_one_cursor() {
        let two = "Pick one\n\n❯ a\n❯ b\n\nEnter to confirm · Esc to cancel";
        assert_eq!(parse_menu(two), None);
        let one = "Pick one\n\n❯ a\n  b\n\nEnter to confirm · Esc to cancel";
        let d = parse_menu(one).unwrap();
        assert_eq!(d.options, ["a", "b"]);
        assert_eq!(d.prompt, "Pick one");
    }

    #[test]
    fn footer_matches_without_spaces() {
        assert!(is_confirm_footer("Entertoconfirm·Esctocancel"));
        assert!(is_confirm_footer("Enter to confirm · Esc to cancel"));
        assert!(!is_confirm_footer("Enter to confirm"));
    }

    #[test]
    fn option_lines_need_a_space_after_the_dot() {
        assert_eq!(parse_option("1.Yes"), None);
        assert_eq!(parse_option("1. "), None);
        assert_eq!(
            parse_option("❯  2. No"),
            Some(MenuOption {
                index: 2,
                text: "No".into(),
                selected: true
            })
        );
        assert_eq!(parse_option("99999999999999999999999. x"), None);
    }

    #[test]
    fn fingerprint_ignores_numbered() {
        let a = PendingDecision {
            prompt: "q".into(),
            options: vec!["a".into()],
            numbered: true,
        };
        let b = PendingDecision {
            numbered: false,
            ..a.clone()
        };
        assert_eq!(a.fingerprint(), "q|a");
        assert_eq!(a.fingerprint(), b.fingerprint());
    }
}
```

`crates/term/src/input_box.rs` 先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn finds_the_input_row_inside_the_box() {
        let screen = rows(&[
            "╭────────────────────╮",
            "│ ❯ Try \"how do I log an error?\" │",
            "╰────────────────────╯",
            "  auto mode on · ? for shortcuts",
        ]);
        assert_eq!(
            input_row(&screen).as_deref(),
            Some("Try \"how do I log an error?\"")
        );
    }

    #[test]
    fn empty_box_gives_empty_row_not_none() {
        let screen = rows(&["╭────╮", "│ >  │", "╰────╯"]);
        assert_eq!(input_row(&screen).as_deref(), Some(""));
    }

    #[test]
    fn numbered_option_under_the_marker_is_not_an_input_row() {
        let screen = rows(&["Pick", "❯ 1. Yes"]);
        assert_eq!(input_row(&screen), None);
    }

    #[test]
    fn dialog_on_screen_hides_the_input_row() {
        let screen = rows(&[
            "Quick safety check: do you trust this folder?",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]);
        assert!(blocking_dialog(&screen).is_some());
        assert_eq!(input_row(&screen), None);
    }

    #[test]
    fn marker_beyond_scan_depth_is_not_found() {
        let mut screen = rows(&["❯ hello"]);
        screen.extend((0..INPUT_SCAN_DEPTH).map(|i| format!("line {i}")));
        assert_eq!(input_row(&screen), None);
        screen.remove(1);
        assert_eq!(input_row(&screen).as_deref(), Some("hello"));
    }

    #[test]
    fn nul_cells_count_as_spaces() {
        let screen = vec!["\0\0❯\0hi".to_string()];
        assert_eq!(input_row(&screen).as_deref(), Some("hi"));
    }

    #[test]
    fn body_tail_ignores_wrapping_and_whitespace() {
        let prompt = format!("{}末尾这句", "a".repeat(100));
        let screen = rows(&[&"a".repeat(80), &format!("{}末尾 这句", "a".repeat(20))]);
        assert!(body_tail_visible(&prompt, &screen));
        assert!(!body_tail_visible(&prompt, &rows(&["something else"])));
        assert!(!body_tail_visible("   ", &screen));
    }
}
```

`crates/term/src/lib.rs` 整个换成：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod input_box;
mod menu;
mod screen;
mod side;
mod tail;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use input_box::{
    BODY_TAIL_CHARS, INPUT_MARKERS, INPUT_SCAN_DEPTH, blocking_dialog, body_tail_visible, input_row,
};
pub use menu::{
    BOX_CHARS, CURSOR_MARKERS, PROMPT_SCAN_DEPTH, PendingDecision, RATE_LIMIT_MENU_PHRASES,
    TRAILING_SLACK, is_confirm_footer, parse_menu, strip,
};
pub use screen::{CursorPos, Screen, ScreenConfig};
pub use tail::AnsiPlainTextTail;
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-term`
Expected: 编译失败，``cannot find function `parse_menu` `` 等。

- [ ] **Step 3: 实现菜单解析**

放在 `crates/term/src/menu.rs` 的测试模块之前：

```rust
//! 从终端明文里认出「交互式选择菜单」（Swift `TerminalMenuParser` 的移植）。
//!
//! 判据缺一不可，全为了**不误报**（误报比漏报更伤：群里一刷噪音，人就不再看通知）：
//! - 编号式：末尾附近一段连续 `N. 文本` 行，序号从 1 连续，≥2 项，至少一行带光标。
//! - 光标列表式（2026-09 起的 claude 信任框）：底部有 `Enter to confirm · Esc to cancel`
//!   脚注，其上 ≥2 行连续非空、无编号的行，恰好一行带光标。
//! - 选项块后最多跟 [`TRAILING_SLACK`] 行正文，再多说明菜单已被答掉。
//! - 限流菜单交给自动应答，这里排除。

/// 选择光标字形。**不含裸 `>`**——markdown 引用、shell 提示符都用它。
pub const CURSOR_MARKERS: [char; 4] = ['❯', '›', '▸', '▶'];

/// TUI 画框字符：逐行剥掉两端的框线。
pub const BOX_CHARS: [char; 20] = [
    '│', '┃', '║', '╭', '╮', '╰', '╯', '─', '━', '═', '┌', '┐', '└', '┘', '├', '┤', '╔', '╗', '╚',
    '╝',
];

/// 选项块后还能跟几行正文（给框下沿的提示行留位）。宁小勿大：决定答完多久状态能回正。
pub const TRAILING_SLACK: usize = 2;

/// 问句往上找多少行（信任框的真问句离选项块有 6～7 行）。
pub const PROMPT_SCAN_DEPTH: usize = 12;

/// 限流菜单的特征短语（小写）。命中的菜单不报给人。
pub const RATE_LIMIT_MENU_PHRASES: [&str; 2] =
    ["/rate-limit-options", "stop and wait for limit to reset"];

/// 一个「在等人选」的现场。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDecision {
    /// 问句原文；认不出时为空串。
    pub prompt: String,
    /// 选项正文（已剥掉序号与光标），按屏幕顺序。
    pub options: Vec<String>,
    /// 屏幕上本来有没有编号。事实字段，每个生产者如实填，没有缺省。
    pub numbered: bool,
}

impl PendingDecision {
    /// 去重指纹：同一菜单反复重绘 → 同一指纹。故意不含 `numbered`。
    pub fn fingerprint(&self) -> String {
        std::iter::once(self.prompt.as_str())
            .chain(self.options.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// 剥掉一行两端的框线与空白。
pub fn strip(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || BOX_CHARS.contains(&c))
}

/// 一个编号选项行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuOption {
    pub(crate) index: usize,
    pub(crate) text: String,
    pub(crate) selected: bool,
}

/// 剥干净的一行是不是 `[光标] N. 文本`。
pub(crate) fn parse_option(stripped: &str) -> Option<MenuOption> {
    let mut s = stripped;
    let mut selected = false;
    if let Some(first) = s.chars().next()
        && CURSOR_MARKERS.contains(&first)
    {
        selected = true;
        s = s[first.len_utf8()..].trim_start();
    }
    let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let index: usize = s[..digits].parse().ok()?;
    let rest = s[digits..].strip_prefix('.')?;
    if !rest.chars().next()?.is_whitespace() {
        return None;
    }
    let text = rest.trim_start();
    if text.is_empty() {
        return None;
    }
    Some(MenuOption {
        index,
        text: text.to_string(),
        selected,
    })
}

/// `Enter to confirm · Esc to cancel`。比对前去掉全部空白——字节尾窗里同一句是
/// `Entertoconfirm·Esctocancel`（claude 用光标定位摆词）。
pub fn is_confirm_footer(stripped: &str) -> bool {
    let compact: String = stripped
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    compact.contains("toconfirm") && compact.contains("tocancel")
}

/// 从一段明文里认菜单。`plain` 是 `\n` 分隔的若干行（屏幕行或字节尾窗）。
pub fn parse_menu(plain: &str) -> Option<PendingDecision> {
    let lines: Vec<&str> = plain.split('\n').map(strip).collect();
    let decision = parse_numbered(&lines).or_else(|| parse_cursor_list(&lines))?;
    let hay = std::iter::once(decision.prompt.as_str())
        .chain(decision.options.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if RATE_LIMIT_MENU_PHRASES.iter().any(|p| hay.contains(p)) {
        return None;
    }
    Some(decision)
}

/// 选项块上方的问句：优先带问号的那行，否则最近一行正文。
fn prompt_above(lines: &[&str], block_start: usize) -> String {
    let mut nearest = "";
    for line in lines[..block_start].iter().rev().take(PROMPT_SCAN_DEPTH) {
        if line.is_empty() {
            continue;
        }
        if nearest.is_empty() {
            nearest = line;
        }
        if line.contains('?') || line.contains('？') {
            return line.to_string();
        }
    }
    nearest.to_string()
}

/// 从底往上找第一行满足 `hit` 的非空行；途中正文超过 TRAILING_SLACK 行即放弃。
fn find_from_bottom(lines: &[&str], hit: impl Fn(&str) -> bool) -> Option<usize> {
    let mut trailing = 0;
    for (i, line) in lines.iter().enumerate().rev() {
        if line.is_empty() {
            continue;
        }
        if hit(line) {
            return Some(i);
        }
        trailing += 1;
        if trailing > TRAILING_SLACK {
            return None;
        }
    }
    None
}

fn parse_numbered(lines: &[&str]) -> Option<PendingDecision> {
    let end = find_from_bottom(lines, |l| parse_option(l).is_some())?;
    // 往上收连续选项行（不跨空行）。
    let mut start = end + 1;
    let mut parsed = Vec::new();
    while start > 0 {
        match parse_option(lines[start - 1]) {
            Some(option) => {
                parsed.push(option);
                start -= 1;
            }
            None => break,
        }
    }
    parsed.reverse();
    let consecutive = parsed.iter().enumerate().all(|(k, o)| o.index == k + 1);
    if parsed.len() < 2 || !consecutive || !parsed.iter().any(|o| o.selected) {
        return None;
    }
    Some(PendingDecision {
        prompt: prompt_above(lines, start),
        options: parsed.into_iter().map(|o| o.text).collect(),
        numbered: true,
    })
}

fn parse_cursor_list(lines: &[&str]) -> Option<PendingDecision> {
    let footer = find_from_bottom(lines, is_confirm_footer)?;
    let end = lines[..footer].iter().rposition(|l| !l.is_empty())?;
    let start = lines[..=end]
        .iter()
        .rposition(|l| l.is_empty())
        .map_or(0, |i| i + 1);
    let block = &lines[start..=end];
    // 带编号的归上面那条路，免得同一屏被两条路解析出两份措辞。
    if block.len() < 2 || block.iter().any(|l| parse_option(l).is_some()) {
        return None;
    }
    let mut options = Vec::with_capacity(block.len());
    let mut cursors = 0;
    for line in block {
        let first = line.chars().next()?;
        if CURSOR_MARKERS.contains(&first) {
            cursors += 1;
            let text = line[first.len_utf8()..].trim();
            if text.is_empty() {
                return None;
            }
            options.push(text.to_string());
        } else {
            options.push(line.to_string());
        }
    }
    if cursors != 1 {
        return None;
    }
    Some(PendingDecision {
        prompt: prompt_above(lines, start),
        options,
        numbered: false,
    })
}
```

- [ ] **Step 4: 实现输入框判据**

放在 `crates/term/src/input_box.rs` 的测试模块之前：

```rust
//! claude 输入框与阻塞对话框的识别（Swift `ClaudeInputBox` 的移植）。
//! 只作用在**渲染完的整屏文本**上（[`crate::Screen::screen_rows`]），不吃原始字节：
//! claude 用光标定位摆词，字节流里 `1.Yes,Itrustthisfolder` 序号后面没有空格。
use crate::menu::{PendingDecision, parse_menu, parse_option, strip};

/// 输入行的提示符字形。`>` 也认（老版本 claude 用它）。
pub const INPUT_MARKERS: [char; 3] = ['❯', '›', '>'];

/// 从底往上找多少行（输入框下面压着框线与提示条，长输入还会折行）。
pub const INPUT_SCAN_DEPTH: usize = 15;

/// `bodyTailVisible` 比对正文末尾多少个非空白字符。
pub const BODY_TAIL_CHARS: usize = 64;

fn normalize(rows: &[String]) -> Vec<String> {
    rows.iter().map(|r| r.replace('\0', " ")).collect()
}

/// 这一屏是不是摆着一个需要人回答的对话框（信任文件夹 / 命令审批 / 选登录方式）。
pub fn blocking_dialog(rows: &[String]) -> Option<PendingDecision> {
    parse_menu(&normalize(rows).join("\n"))
}

/// 输入行提示符后面的内容；`None` = 屏上没有可投递的输入框（还没就绪，或正在等人回答）。
///
/// 有对话框就一律 `None`：没编号的信任框 `❯  No, exit` 单看一行跟「人在输入框里打了字」
/// 一模一样，判据只能落在整屏这一层。
pub fn input_row(rows: &[String]) -> Option<String> {
    if blocking_dialog(rows).is_some() {
        return None;
    }
    let rows = normalize(rows);
    let start = rows.len().saturating_sub(INPUT_SCAN_DEPTH);
    for line in rows[start..].iter().rev() {
        let stripped = strip(line);
        let Some(first) = stripped.chars().next() else {
            continue;
        };
        if !INPUT_MARKERS.contains(&first) {
            continue;
        }
        if parse_option(stripped).is_some() {
            return None;
        }
        return Some(stripped[first.len_utf8()..].trim().to_string());
    }
    None
}

/// 超长正文会把提示符滚出屏幕：去掉空白后比对 `prompt` 末尾 64 个字符是否出现在屏上。
pub fn body_tail_visible(prompt: &str, rows: &[String]) -> bool {
    let compact: Vec<char> = prompt.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return false;
    }
    let needle: String = compact[compact.len().saturating_sub(BODY_TAIL_CHARS)..]
        .iter()
        .collect();
    let screen: String = normalize(rows)
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    screen.contains(&needle)
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `57 passed`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/src/menu.rs crates/term/src/input_box.rs crates/term/src/lib.rs
git commit -m "crewd-term: port menu parser (numbered + cursor-list) and claude input-box checks

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: `PendingDecisionTracker`

`SessionPendingDecision.swift:302-327` 的移植。两条来源（屏幕优先、字节尾窗其次）一个出口；同指纹稳定 `STABLE_WINDOW`（3s）才报 `Appeared`；已在跟踪/已报的指纹粘住；两条都认不出才报 `Cleared`。时间用调用方传入的 `Instant`（裁定 3），计算用 `saturating_duration_since`，所以 `now` 倒退时只会「还没到」，不会提前报。

**Files:**
- Create: `crewd/crates/term/src/tracker.rs`
- Modify: `crewd/crates/term/src/lib.rs`

**Interfaces:**
- Consumes: Task 3 `AnsiPlainTextTail::{new, feed, tail}`；Task 4 `parse_menu`、`blocking_dialog`、`PendingDecision::fingerprint`。
- Produces:
  - `pub const STABLE_WINDOW: Duration = Duration::from_secs(3);`（M2b 的投递状态机「对话框稳定」直接引它，别再写第二个数）
  - `pub enum DecisionEvent { Appeared(PendingDecision), Cleared }`
  - `pub struct PendingDecisionTracker`：`new()`、`with_stable(Duration)`、`feed(&mut self, &[u8])`、`pending(&self) -> Option<&PendingDecision>`、`poll(&mut self, now: Instant, screen: Option<&[String]>) -> Option<DecisionEvent>`

- [ ] **Step 1: 写失败的测试**

`crates/term/src/tracker.rs` 先只放测试模块（场景照搬 Swift `SessionPendingDecisionTests` 的 Tracker 段，外加 Review Focus 4）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn permission_menu() -> String {
        [
            "╭──────────────────────────────────────────────╮",
            "│ Bash command                                 │",
            "│                                              │",
            "│   rm -rf build                               │",
            "│                                              │",
            "│ Do you want to proceed?                      │",
            "│ ❯ 1. Yes                                     │",
            "│   2. No, and tell Claude what to do (esc)    │",
            "╰──────────────────────────────────────────────╯",
        ]
        .join("\n")
    }

    fn unnumbered_trust_rows() -> Vec<String> {
        [
            "Quick safety check: Is this a project you created or one you trust?",
            "",
            "Security guide",
            "",
            "❯  No, exit",
            "   Yes, I trust this folder",
            "",
            "Enter to confirm · Esc to cancel",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    #[test]
    fn menu_must_be_stable_before_firing() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        assert_eq!(t.poll(t0, None), None);
        assert_eq!(t.poll(t0 + secs(2.0), None), None);
        match t.poll(t0 + secs(3.1), None) {
            Some(DecisionEvent::Appeared(d)) => assert_eq!(d.prompt, "Do you want to proceed?"),
            other => panic!("稳定超过窗口后应报出待决策，实际 {other:?}"),
        }
        assert_eq!(t.pending().map(|d| d.options.len()), Some(2));
    }

    #[test]
    fn redraw_of_same_menu_does_not_reset_the_clock() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        assert_eq!(t.poll(t0, None), None);
        t.feed(format!("\n{}", permission_menu()).as_bytes());
        assert_eq!(t.poll(t0 + secs(1.0), None), None);
        assert!(matches!(
            t.poll(t0 + secs(3.1), None),
            Some(DecisionEvent::Appeared(_))
        ));
    }

    #[test]
    fn fires_only_once_per_menu() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        t.poll(t0, None);
        assert!(t.poll(t0 + secs(4.0), None).is_some());
        assert_eq!(t.poll(t0 + secs(5.0), None), None);
        assert_eq!(t.poll(t0 + secs(60.0), None), None);
    }

    #[test]
    fn clears_when_menu_goes_away() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(permission_menu().as_bytes());
        t.poll(t0, None);
        assert!(t.poll(t0 + secs(4.0), None).is_some());
        t.feed(
            "\n⏺ Bash(rm -rf build)\n  ⎿  (No content)\n⏺ 删好了，继续。\n继续读文件…\n".as_bytes(),
        );
        assert_eq!(t.poll(t0 + secs(5.0), None), Some(DecisionEvent::Cleared));
        assert_eq!(t.poll(t0 + secs(6.0), None), None, "清过一次就别再清");
        assert_eq!(t.pending(), None);
    }

    #[test]
    fn same_menu_fires_again_after_being_cleared() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        t.feed(permission_menu().as_bytes());
        t.poll(now, None);
        now += secs(4.0);
        assert!(t.poll(now, None).is_some());
        t.feed("\n⏺ 干完了，继续下一步，正在读文件…\n再读一个文件…\n还在读…\n".as_bytes());
        now += secs(1.0);
        assert_eq!(t.poll(now, None), Some(DecisionEvent::Cleared));
        t.feed(format!("\n{}", permission_menu()).as_bytes());
        now += secs(1.0);
        assert_eq!(t.poll(now, None), None);
        now += secs(4.0);
        assert!(matches!(
            t.poll(now, None),
            Some(DecisionEvent::Appeared(_))
        ));
    }

    #[test]
    fn streaming_output_never_fires() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        for i in 0..10 {
            t.feed(format!("⏺ 正在处理第 {i} 个文件…\n").as_bytes());
            now += secs(1.0);
            assert_eq!(t.poll(now, None), None);
        }
    }

    #[test]
    fn screen_source_fires_when_byte_tail_is_blind() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now();
        t.feed(b"\x1b[2GQuick\x1b[8Gsafety\x1b[15Gcheck");
        let screen = unnumbered_trust_rows();
        assert_eq!(t.poll(t0, Some(&screen)), None);
        match t.poll(t0 + STABLE_WINDOW + secs(0.1), Some(&screen)) {
            Some(DecisionEvent::Appeared(d)) => {
                assert_eq!(d.options, ["No, exit", "Yes, I trust this folder"]);
                assert!(!d.numbered);
            }
            other => panic!("屏幕上稳稳挂着信任框却没报，实际 {other:?}"),
        }
    }

    #[test]
    fn screen_source_clears_when_dialog_leaves_the_screen() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        let screen = unnumbered_trust_rows();
        t.poll(now, Some(&screen));
        now += STABLE_WINDOW + secs(0.1);
        assert!(t.poll(now, Some(&screen)).is_some());
        now += secs(1.0);
        let after = vec![
            "⏺ 好的，开始干活了。".to_string(),
            "正在读文件…".to_string(),
        ];
        assert_eq!(t.poll(now, Some(&after)), Some(DecisionEvent::Cleared));
    }

    #[test]
    fn two_sources_do_not_fight_or_flicker() {
        let mut t = PendingDecisionTracker::new();
        let mut now = Instant::now();
        t.feed(permission_menu().as_bytes());
        let screen: Vec<String> = permission_menu().split('\n').map(str::to_string).collect();
        t.poll(now, Some(&screen));
        now += STABLE_WINDOW + secs(0.1);
        assert!(matches!(
            t.poll(now, Some(&screen)),
            Some(DecisionEvent::Appeared(_))
        ));
        for _ in 0..10 {
            now += secs(1.0);
            assert_eq!(t.poll(now, Some(&screen)), None, "同一个菜单不许反复报");
        }
        now += secs(1.0);
        let half = vec!["…重绘中…".to_string()];
        assert_eq!(t.poll(now, Some(&half)), None, "半成品画面不该把状态清掉");
    }

    #[test]
    fn clock_going_backwards_does_not_fire_early() {
        let mut t = PendingDecisionTracker::new();
        let t0 = Instant::now() + secs(10.0);
        t.feed(permission_menu().as_bytes());
        t.poll(t0, None);
        assert_eq!(t.poll(t0 - secs(5.0), None), None);
        assert!(t.poll(t0 + STABLE_WINDOW, None).is_some());
    }
}
```

`crates/term/src/lib.rs` 整个换成：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod input_box;
mod menu;
mod screen;
mod side;
mod tail;
mod tracker;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use input_box::{
    BODY_TAIL_CHARS, INPUT_MARKERS, INPUT_SCAN_DEPTH, blocking_dialog, body_tail_visible, input_row,
};
pub use menu::{
    BOX_CHARS, CURSOR_MARKERS, PROMPT_SCAN_DEPTH, PendingDecision, RATE_LIMIT_MENU_PHRASES,
    TRAILING_SLACK, is_confirm_footer, parse_menu, strip,
};
pub use screen::{CursorPos, Screen, ScreenConfig};
pub use tail::AnsiPlainTextTail;
pub use tracker::{DecisionEvent, PendingDecisionTracker, STABLE_WINDOW};
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-term`
Expected: 编译失败，``cannot find type `PendingDecisionTracker` ``。

- [ ] **Step 3: 写实现**

放在 `crates/term/src/tracker.rs` 的测试模块之前：

```rust
//! 「有个菜单在等人」的出现/消失跟踪（Swift `PendingDecisionTracker` 的移植）。
//!
//! 两条来源、一个出口：渲染完的屏幕（优先，权威措辞）和去 ANSI 的字节尾窗。
//! 同一指纹稳定 [`STABLE_WINDOW`] 才报 `Appeared`；已在跟踪/已报的那个指纹粘住，
//! 免得两条来源措辞略不同就来回抢；两条都认不出才报 `Cleared`。
//! 时间由调用方传入（`now`），不读时钟——单测用假时间。
use std::time::{Duration, Instant};

use crate::input_box::blocking_dialog;
use crate::menu::{PendingDecision, parse_menu};
use crate::tail::AnsiPlainTextTail;

/// 菜单在屏上稳住多久才算真在等人。全仓唯一的这个阈值（M2b 的投递状态机直接引它）。
pub const STABLE_WINDOW: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionEvent {
    Appeared(PendingDecision),
    Cleared,
}

#[derive(Debug)]
pub struct PendingDecisionTracker {
    tail: AnsiPlainTextTail,
    stable: Duration,
    candidate: Option<(PendingDecision, Instant)>,
    reported: Option<PendingDecision>,
}

impl Default for PendingDecisionTracker {
    fn default() -> Self {
        Self::with_stable(STABLE_WINDOW)
    }
}

impl PendingDecisionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stable(stable: Duration) -> Self {
        Self {
            tail: AnsiPlainTextTail::new(),
            stable,
            candidate: None,
            reported: None,
        }
    }

    /// 喂 PTY 原始字节（进字节尾窗）。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.tail.feed(bytes);
    }

    /// 当前正在等的那个（`None` = 没在等）。
    pub fn pending(&self) -> Option<&PendingDecision> {
        self.reported.as_ref()
    }

    /// 定期调用。`screen` = 渲染完的那一屏（[`crate::Screen::screen_rows`]）；
    /// `None` = 拿不到画面，只看字节尾窗。
    pub fn poll(&mut self, now: Instant, screen: Option<&[String]>) -> Option<DecisionEvent> {
        let seen: Vec<PendingDecision> = [
            screen.and_then(blocking_dialog),
            parse_menu(self.tail.tail()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let sticky = self
            .reported
            .as_ref()
            .or(self.candidate.as_ref().map(|(d, _)| d))
            .map(PendingDecision::fingerprint);
        let chosen = seen
            .iter()
            .find(|d| Some(d.fingerprint()) == sticky)
            .or(seen.first())
            .cloned();

        let Some(decision) = chosen else {
            self.candidate = None;
            return self.reported.take().map(|_| DecisionEvent::Cleared);
        };
        let since = match &self.candidate {
            Some((current, since)) if current.fingerprint() == decision.fingerprint() => *since,
            _ => {
                self.candidate = Some((decision, now));
                return None;
            }
        };
        if now.saturating_duration_since(since) < self.stable {
            return None;
        }
        if self.reported.as_ref().map(PendingDecision::fingerprint) == Some(decision.fingerprint())
        {
            return None;
        }
        self.reported = Some(decision.clone());
        Some(DecisionEvent::Appeared(decision))
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `67 passed`。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/src/tracker.rs crates/term/src/lib.rs
git commit -m "crewd-term: PendingDecisionTracker with caller-supplied time

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 夹具回归——13 条期望

把 Swift `ClaudeInputBoxFixtureTests` 的期望搬过来，跑在 Task 1–5 的成品上。对照表（左 = 期望，右 = 测试）：

| # | 期望 | 测试 |
|---|---|---|
| 1–5 | `tui-claude.bin`：见到对话框；见到输入框；对话框先于输入框；对话框在场时 `input_row` 为 `None`；空输入框里出现过 `Try "…"` | `claude_walks_from_trust_dialog_to_ready_input_box` |
| 6–7 | 旧框选项 trust→exit 且 `numbered=true`；默认高亮在 trust | `claude_old_trust_dialog_is_numbered_trust_then_exit` |
| 8 | 新框 exit 在前、`numbered=false`、问句提到 trust/safety | `todays_unnumbered_trust_dialog_is_recognised` |
| 9 | 新框那屏 `input_row` 为 `None` | `todays_trust_dialog_is_not_a_ready_input_box` |
| 10 | 新框默认高亮在 exit | `todays_trust_dialog_cursor_sits_on_exit` |
| 11 | 新框分块喂：见到对话框且期间从不给输入行 | `todays_trust_dialog_chunked_never_offers_an_input_row` |
| 12–13 | `tui-claude-ready.bin`：从不出对话框；见到输入框 | `ready_session_never_looks_like_a_pending_decision` |

另加：信任框在屏 → 3s 后 `Appeared`、答掉后 `Cleared`（Swift 那条 P0）；4 个夹具逐字节喂与整块喂结果相同（Review Focus 1）；claude 启动查询（OSC 11、DA1）与 `?1004h` 都得到了回答；claude 不用 alt 屏（快照编码器的前提）。

这是回归网，不是新功能：测试写完应直接通过。**若有失败，那是真的行为差**——按 superpowers:systematic-debugging 查根因、修 Task 1–5 的实现，绝不放宽期望。

**Files:**
- Create: `crewd/crates/term/tests/fixtures/{tui-claude.bin,tui-claude-trust.bin,tui-claude-ready.bin,tui-shell.bin}`、`crewd/crates/term/tests/claude_fixtures.rs`

**Interfaces:**
- Consumes: 公开 API——`Screen::{new, feed, screen_rows, cursor, take_replies}`、`ScreenConfig::default()`、`blocking_dialog`、`input_row`、`strip`、`CURSOR_MARKERS`、`PendingDecision`、`PendingDecisionTracker::{new, feed, poll}`、`DecisionEvent`、`STABLE_WINDOW`。
- Produces: `crates/term/tests/fixtures/*.bin`（Task 8 的 `include_bytes!` 用）。

- [ ] **Step 1: 拷夹具**

```bash
mkdir crates/term/tests crates/term/tests/fixtures
cp ../Tests/Fixtures/tui-claude.bin ../Tests/Fixtures/tui-claude-trust.bin ../Tests/Fixtures/tui-claude-ready.bin ../Tests/Fixtures/tui-shell.bin crates/term/tests/fixtures/
ls -l crates/term/tests/fixtures
```

Expected: 4 个文件，大小 7620 / 1122 / 2681 / 10182 字节。（用 `mkdir` 不带 `-p`：目录已存在就停下来看看是什么，别覆盖。）

- [ ] **Step 2: 写夹具测试**

`crates/term/tests/claude_fixtures.rs`：

```rust
//! 拿录下来的 claude TUI 字节（`tests/fixtures/*.bin`，80×25，Swift 版
//! `AgentTuiFixtureRecorder` 录的，原样拷进来）验读屏与对话框判据。
//! 期望照搬 Swift `ClaudeInputBoxFixtureTests` 那 13 条；验收标准是这些期望，
//! 不是与 SwiftTerm 逐字节一致（SwiftTerm 在 64 字节边界切开 OSC 标题时有已知 bug）。
use std::time::Instant;

use crewd_term::{
    CURSOR_MARKERS, DecisionEvent, PendingDecision, PendingDecisionTracker, STABLE_WINDOW, Screen,
    ScreenConfig, blocking_dialog, input_row, strip,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    // 入库的夹具缺了就失败，绝不静默跳过。
    std::fs::read(&path).unwrap_or_else(|e| panic!("缺夹具 {path}: {e}"))
}

fn screen() -> Screen {
    Screen::new(ScreenConfig::default())
}

/// 每喂 64 字节回调一次当前屏（生产里 PTY 输出就是这样一块块到的）。
fn walk(name: &str, mut on_step: impl FnMut(usize, &Screen, &[u8])) {
    let bytes = fixture(name);
    let mut s = screen();
    for (k, chunk) in bytes.chunks(64).enumerate() {
        s.feed(chunk);
        on_step((k + 1) * 64, &s, chunk);
    }
}

fn whole(name: &str) -> Vec<String> {
    let mut s = screen();
    s.feed(&fixture(name));
    s.screen_rows()
}

/// 屏上第一行以选择光标开头的行（剥框后）。
fn cursor_line(rows: &[String]) -> String {
    rows.iter()
        .find(|r| {
            strip(r)
                .chars()
                .next()
                .is_some_and(|c| CURSOR_MARKERS.contains(&c))
        })
        .cloned()
        .unwrap_or_default()
}

struct Walk {
    saw_dialog: bool,
    saw_ready: bool,
    dialog_came_first: bool,
    input_while_dialog: Vec<usize>,
    input_rows: Vec<String>,
    first_dialog: Option<(PendingDecision, String)>,
}

fn walk_dialog_and_input(name: &str) -> Walk {
    let mut w = Walk {
        saw_dialog: false,
        saw_ready: false,
        dialog_came_first: false,
        input_while_dialog: Vec::new(),
        input_rows: Vec::new(),
        first_dialog: None,
    };
    walk(name, |offset, s, _| {
        let rows = s.screen_rows();
        let dialog = blocking_dialog(&rows);
        let input = input_row(&rows);
        if let Some(d) = &dialog {
            w.saw_dialog = true;
            if w.first_dialog.is_none() {
                w.first_dialog = Some((d.clone(), cursor_line(&rows)));
            }
            if input.is_some() {
                w.input_while_dialog.push(offset);
            }
        }
        if let Some(input) = input {
            if !w.saw_ready {
                w.saw_ready = true;
                w.dialog_came_first = w.saw_dialog;
            }
            if w.input_rows.last() != Some(&input) {
                w.input_rows.push(input);
            }
        }
    });
    w
}

// ---- tui-claude.bin（2026-08-26：编号式信任框 → 横幅 → 输入框）----

#[test]
fn claude_walks_from_trust_dialog_to_ready_input_box() {
    let w = walk_dialog_and_input("tui-claude.bin");
    assert!(w.saw_dialog, "首屏的信任对话框必须认得出来");
    assert!(w.saw_ready, "答完对话框后的输入框必须认得出来");
    assert!(w.dialog_came_first, "顺序应是先对话框、后输入框");
    assert!(
        w.input_while_dialog.is_empty(),
        "对话框在场时被判成输入框就绪（brief 会被当按键投进去）：喂到 {:?} 字节",
        w.input_while_dialog
    );
    assert!(
        w.input_rows.iter().any(|r| r.contains("Try")),
        "空输入框里应出现过 `Try \"…\"` 示例提示：{:?}",
        w.input_rows
    );
}

#[test]
fn claude_old_trust_dialog_is_numbered_trust_then_exit() {
    let w = walk_dialog_and_input("tui-claude.bin");
    let (d, cursor) = w.first_dialog.expect("信任对话框没认出来");
    assert_eq!(d.options.len(), 2, "{d:?}");
    assert!(d.options[0].contains("trust"), "{d:?}");
    assert!(d.options[1].contains("exit"), "{d:?}");
    assert!(d.numbered, "8-26 那版有编号：{d:?}");
    assert!(
        cursor.contains("trust"),
        "8-26 默认高亮在「同意」：{cursor}"
    );
}

// ---- tui-claude-trust.bin（2026-09-06：无编号、No, exit 在前且默认高亮）----

#[test]
fn todays_unnumbered_trust_dialog_is_recognised() {
    let rows = whole("tui-claude-trust.bin");
    let d = blocking_dialog(&rows).unwrap_or_else(|| panic!("没认出信任框：\n{}", rows.join("\n")));
    assert_eq!(d.options.len(), 2, "{d:?}");
    assert!(d.options[0].contains("exit"), "今天第一项是退出：{d:?}");
    assert!(d.options.iter().any(|o| o.contains("trust")), "{d:?}");
    assert!(!d.numbered, "真屏幕上没有编号：{d:?}");
    assert!(
        d.prompt.contains("trust") || d.prompt.contains("safety"),
        "问句得说清在问什么：{}",
        d.prompt
    );
}

#[test]
fn todays_trust_dialog_is_not_a_ready_input_box() {
    let rows = whole("tui-claude-trust.bin");
    assert_eq!(
        input_row(&rows),
        None,
        "信任框被判成输入框就绪：\n{}",
        rows.join("\n")
    );
}

#[test]
fn todays_trust_dialog_cursor_sits_on_exit() {
    let rows = whole("tui-claude-trust.bin");
    let cursor = cursor_line(&rows);
    assert!(cursor.contains("exit"), "今天默认高亮在「退出」：{cursor}");
}

#[test]
fn todays_trust_dialog_chunked_never_offers_an_input_row() {
    let w = walk_dialog_and_input("tui-claude-trust.bin");
    assert!(w.saw_dialog);
    assert!(
        w.input_while_dialog.is_empty(),
        "{:?}",
        w.input_while_dialog
    );
}

// ---- tui-claude-ready.bin（已信任目录，直接就绪）----

#[test]
fn ready_session_never_looks_like_a_pending_decision() {
    let w = walk_dialog_and_input("tui-claude-ready.bin");
    assert!(
        !w.saw_dialog,
        "正常就绪被判成在等人拍板：{:?}",
        w.first_dialog
    );
    assert!(
        w.saw_ready,
        "这段语料里有画好的空输入框：{:?}",
        w.input_rows
    );
}

// ---- 待决策出口：信任框在屏 → 必须报出来，答掉后必须清 ----

#[test]
fn trust_dialog_on_screen_raises_and_then_clears_a_pending_decision() {
    let mut tracker = PendingDecisionTracker::new();
    let mut s = screen();
    let t0 = Instant::now();
    for chunk in fixture("tui-claude-trust.bin").chunks(64) {
        s.feed(chunk);
        tracker.feed(chunk);
        let _ = tracker.poll(t0, Some(&s.screen_rows()));
    }
    let rows = s.screen_rows();
    match tracker.poll(t0 + STABLE_WINDOW, Some(&rows)) {
        Some(DecisionEvent::Appeared(d)) => {
            assert!(d.options.iter().any(|o| o.contains("trust")), "{d:?}");
            assert!(!d.numbered);
        }
        other => panic!("信任框稳稳挂着却没报：{other:?}\n{}", rows.join("\n")),
    }
    tracker.feed("\n⏺ 好的，开始干活了。\n正在读文件…\n还在读…\n".as_bytes());
    s.feed("\x1b[2J\x1b[H⏺ 好的，开始干活了。\r\n正在读文件…\r\n".as_bytes());
    assert_eq!(
        tracker.poll(t0 + STABLE_WINDOW * 2, Some(&s.screen_rows())),
        Some(DecisionEvent::Cleared)
    );
}

// ---- 与喂法无关 ----

#[test]
fn every_fixture_renders_the_same_whatever_the_chunking() {
    for name in [
        "tui-claude.bin",
        "tui-claude-trust.bin",
        "tui-claude-ready.bin",
        "tui-shell.bin",
    ] {
        let bytes = fixture(name);
        let mut bytewise = screen();
        for b in &bytes {
            bytewise.feed(std::slice::from_ref(b));
        }
        assert_eq!(bytewise.screen_rows(), whole(name), "{name}");
        assert_eq!(bytewise.cursor(), {
            let mut s = screen();
            s.feed(&bytes);
            s.cursor()
        });
    }
}

#[test]
fn claude_startup_queries_get_answered() {
    let mut s = screen();
    s.feed(&fixture("tui-claude.bin"));
    let replies = String::from_utf8(s.take_replies()).unwrap();
    // claude 启动时问 OSC 11（背景色，定明暗主题）和 DA1，还开了焦点上报。
    assert!(
        replies.contains("\x1b]11;rgb:0000/0000/0000\x07"),
        "{replies:?}"
    );
    assert!(replies.contains("\x1b[?6c"), "{replies:?}");
    assert!(replies.contains("\x1b[I"), "{replies:?}");
}

#[test]
fn claude_never_uses_the_alternate_screen() {
    let bytes = fixture("tui-claude.bin");
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!has(b"\x1b[?1049h"), "claude 实测不进 alt 屏");
    assert!(!has(b"\x1b[?47h"));
}
```

- [ ] **Step 3: 跑测试**

Run: `cargo test -p crewd-term --test claude_fixtures`
Expected: `11 passed`。

- [ ] **Step 4: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/tests/fixtures crates/term/tests/claude_fixtures.rs
git diff --cached --stat
git commit -m "crewd-term: claude TUI fixture regression (13 expectations from ClaudeInputBoxFixtureTests)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（`git diff --cached --stat` 应正好 5 个文件。）

---

### Task 7: 快照编码器

输出 = 一段字节，喂进一台**同尺寸**的干净 `Screen` 就得到逐格相同的终端。顺序：

1. `ESC c`；在 alt 屏则 `ESC[?1049h`（主屏内容拿不到，见已知缺口测试）。
2. 回滚 + 屏幕逐行。每格按画笔（fg/bg/SGR 标志/下划线色/超链接）变化时**从 0 重建** SGR，超链接用 OSC 8。行间：上一行带 `WRAPLINE` 就不发 `\r\n`（整行写满，由下一格触发终端自己折），否则先把底色复位再 `\r\n`（BCE：上滚出的新行会拿当前底色填）。续行至少写一格。跳过 `WIDE_CHAR_SPACER`；行尾 `LEADING_WIDE_CHAR_SPACER` 在下一行以宽字符开头时跳过（写那个宽字符会自动再造出来）。`\t` 格：按属性写空格、CHA 回来、写 HT、CHA 到下一格。
3. 滚动区域不是整屏则 DECSTBM。
4. 原点模式先开（会把光标拉到区域原点），再 CUP；pending-wrap 则重写光标所在的最后一格（停在宽字符上就重写那个宽字符）。
5. 模式：`?1`、`ESC =`、`?1000/1002/1003`、`?1005/1006`、`?1004`、`?2004`、关掉的 `?1007/?1042/?7`、`CSI 4 h`、`CSI 20 h`、隐藏光标 `?25l`——都不动光标、不清 pending-wrap。
6. 最后把画笔设成光标模板的画笔（程序接着写的字要跟原来一个颜色）。

**Files:**
- Create: `crewd/crates/term/src/snapshot.rs`
- Modify: `crewd/crates/term/src/screen.rs`（加 `pub(crate) fn term()`）、`crewd/crates/term/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 `Screen::{cursor, cols, lines, history_size, screen_rows, resize}`、`ScreenConfig`；Task 2 `Screen::scroll_region`。
- Produces:
  - `pub struct Snapshot { pub cols: usize, pub lines: usize, pub bytes: Vec<u8> }`（`Clone + Debug + Eq`）
  - `Screen::snapshot(&self) -> Snapshot`（纯读）
  - crate 内：`Screen::term(&self) -> &Term<Replies>`；`pub(crate) struct Pen { fg, bg, flags, underline, link }` + `Pen::of(&Cell)`；`pub(crate) fn sgr(&Pen) -> String`；`pub(crate) mod tests` 里的 `assert_round_trip(&Screen, &str)` 与 `assert_same(&Screen, &Screen, &str)`（Task 8 复用）

- [ ] **Step 1: 写失败的测试**

`crates/term/src/snapshot.rs` 先只放测试模块。注意它是 `pub(crate) mod tests`，两个断言函数是 `pub(crate)`——Task 8 的语料往返要复用：

```rust
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::screen::ScreenConfig;

    fn screen(cols: usize, lines: usize) -> Screen {
        Screen::new(ScreenConfig {
            cols,
            lines,
            ..ScreenConfig::default()
        })
    }

    fn fed(cols: usize, lines: usize, bytes: &[u8]) -> Screen {
        let mut s = screen(cols, lines);
        s.feed(bytes);
        s
    }

    /// 快照喂进一台同尺寸的新终端，逐格、光标、模式、滚动区域、画笔都得一样。
    pub(crate) fn assert_round_trip(src: &Screen, what: &str) {
        let snap = src.snapshot();
        let dst = fed(snap.cols, snap.lines, &snap.bytes);
        assert_same(src, &dst, what);
    }

    pub(crate) fn assert_same(a: &Screen, b: &Screen, what: &str) {
        let (ta, tb) = (a.term(), b.term());
        assert_eq!((a.cols(), a.lines()), (b.cols(), b.lines()), "{what}：尺寸");
        assert_eq!(a.history_size(), b.history_size(), "{what}：回滚行数");
        let first = -(a.history_size() as i32);
        for line in first..a.lines() as i32 {
            let (ra, rb) = (&ta.grid()[Line(line)], &tb.grid()[Line(line)]);
            for col in 0..a.cols() {
                let (ca, cb) = (&ra[Column(col)], &rb[Column(col)]);
                assert_eq!(ca, cb, "{what}：第 {line} 行第 {col} 格");
            }
        }
        assert_eq!(a.cursor(), b.cursor(), "{what}：光标");
        assert_eq!(ta.mode(), tb.mode(), "{what}：模式");
        assert_eq!(a.scroll_region(), b.scroll_region(), "{what}：滚动区域");
        assert_eq!(
            Pen::of(&ta.grid().cursor.template),
            Pen::of(&tb.grid().cursor.template),
            "{what}：当前画笔"
        );
    }

    #[test]
    fn starts_with_a_hard_reset() {
        let snap = fed(80, 25, b"hi").snapshot();
        assert!(snap.bytes.starts_with(b"\x1bc"));
        assert_eq!((snap.cols, snap.lines), (80, 25));
    }

    #[test]
    fn plain_text_round_trips() {
        assert_round_trip(&fed(80, 25, b"hello\r\nworld"), "纯文本");
        assert_round_trip(&fed(80, 25, b""), "空屏");
    }

    #[test]
    fn sgr_family_round_trips() {
        let bytes = concat!(
            "\x1b[1mB\x1b[2mD\x1b[3mI\x1b[4mU\x1b[4:2mUU\x1b[4:3mC\x1b[4:4mO\x1b[4:5mA",
            "\x1b[7mR\x1b[8mH\x1b[9mS\x1b[0m",
            "\x1b[31mr\x1b[91mbr\x1b[38;5;1mi1\x1b[38;5;200mi\x1b[38;2;1;2;3mt",
            "\x1b[44mb\x1b[104mbb\x1b[48;5;17mib\x1b[48;2;9;8;7mtb",
            "\x1b[4;58;5;3mu\x1b[58;2;4;5;6mv\x1b[59mw\x1b[0m."
        );
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "SGR");
    }

    #[test]
    fn named_and_indexed_colors_stay_distinct() {
        let s = fed(80, 25, b"\x1b[31mA\x1b[38;5;1mB");
        let snap = s.snapshot();
        let text = String::from_utf8_lossy(&snap.bytes);
        assert!(text.contains("\x1b[0;31mA"), "{text:?}");
        assert!(text.contains("\x1b[0;38;5;1mB"), "{text:?}");
    }

    #[test]
    fn wide_zero_width_and_tabs_round_trip() {
        let bytes =
            "我在，attach\r\ne\u{301}x 👩\u{200d}💻\r\na\tb\x1b[44m\tc\x1b[0m\r\n\x1b[78G中文";
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "宽字符/组合字符/制表符");
    }

    #[test]
    fn erased_cells_keep_their_background() {
        let bytes = b"\x1b[41m\x1b[2K\x1b[0mx\r\n\x1b[42m   \x1b[0m\r\n\x1b[43m\x1b[5X";
        assert_round_trip(&fed(80, 25, bytes), "带底色擦除");
    }

    #[test]
    fn background_does_not_bleed_into_scrolled_lines() {
        let mut bytes = String::new();
        for n in 0..30 {
            bytes.push_str(&format!("\x1b[41m{n}\x1b[0m\r\n"));
        }
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "换行上滚不带底色");
    }

    #[test]
    fn sparse_screen_and_cursor_round_trip() {
        let bytes = b"\x1b[5;10Hmid\x1b[20;70Hlow\x1b[12;33H";
        assert_round_trip(&fed(80, 25, bytes), "稀疏屏");
    }

    #[test]
    fn soft_wraps_keep_their_flag() {
        let long = format!("{}END\r\nnext", "x".repeat(170));
        let s = fed(80, 25, long.as_bytes());
        assert!(
            s.term().grid()[Line(0)][Column(79)]
                .flags
                .contains(Flags::WRAPLINE)
        );
        assert_round_trip(&s, "软换行");
    }

    #[test]
    fn blank_wrapped_continuation_is_not_lost() {
        // 续行整行空白：没有字符去触发那一折就会丢一整行（Swift 版第一版的 bug）。
        let bytes = format!("{}{}\r\nafter", "x".repeat(80), " ".repeat(80));
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "空白续行");
    }

    #[test]
    fn wide_char_that_did_not_fit_wraps_the_same_way() {
        let bytes = format!("{}中", "x".repeat(79));
        let s = fed(80, 25, bytes.as_bytes());
        assert!(
            s.term().grid()[Line(0)][Column(79)]
                .flags
                .contains(Flags::LEADING_WIDE_CHAR_SPACER)
        );
        assert_round_trip(&s, "行尾放不下的宽字符");
    }

    #[test]
    fn wrap_on_last_row_while_scrollback_grows() {
        let mut bytes = String::new();
        for n in 0..40 {
            bytes.push_str(&format!("line {n}\r\n"));
        }
        bytes.push_str(&"y".repeat(200));
        assert_round_trip(&fed(80, 25, bytes.as_bytes()), "底行软换行 + 回滚");
    }

    #[test]
    fn scrollback_is_restored_in_full() {
        let mut s = screen(80, 25);
        for n in 0..10_050 {
            s.feed(format!("\x1b[3{}mline {n}\r\n", n % 8).as_bytes());
        }
        assert_eq!(s.history_size(), 10_000);
        assert_round_trip(&s, "满回滚");
    }

    #[test]
    fn pending_wrap_cursor_is_reproduced() {
        let s = fed(80, 25, "x".repeat(80).as_bytes());
        assert!(s.cursor().pending_wrap);
        assert_round_trip(&s, "pending-wrap");
        let s = fed(80, 25, format!("{}中", "x".repeat(78)).as_bytes());
        assert!(s.cursor().pending_wrap);
        assert_round_trip(&s, "pending-wrap 停在宽字符上");
    }

    #[test]
    fn modes_are_restored() {
        let bytes = b"\x1b[?1h\x1b=\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[?1007l\x1b[?1042l\x1b[?7l\x1b[4h\x1b[20h\x1b[?25l";
        assert_round_trip(&fed(80, 25, bytes), "模式");
        assert_round_trip(
            &fed(80, 25, b"\x1b[?1000h\x1b[?1005h"),
            "鼠标点击 + UTF8 编码",
        );
        assert_round_trip(&fed(80, 25, b"\x1b[?1003h"), "鼠标任意移动");
    }

    #[test]
    fn scroll_region_and_origin_mode_are_restored() {
        assert_round_trip(&fed(80, 25, b"top\x1b[3;20r\x1b[10;5Hx"), "滚动区域");
        assert_round_trip(&fed(80, 25, b"\x1b[3;20r\x1b[?6h\x1b[4;7Hy"), "原点模式");
    }

    #[test]
    fn current_pen_and_hyperlink_are_restored() {
        let bytes = b"\x1b]8;id=a;https://example.com\x1b\\link\x1b]8;;\x1b\\ \x1b[1;35;48;5;22m";
        assert_round_trip(&fed(80, 25, bytes), "画笔 + 超链接");
        let open = b"\x1b]8;;https://example.com/x;y\x1b\\still linked";
        assert_round_trip(&fed(80, 25, open), "未关闭的超链接");
    }

    #[test]
    fn alternate_screen_content_is_restored() {
        let s = fed(80, 25, b"main\x1b[?1049halt screen\x1b[3;3Hz");
        assert_round_trip(&s, "alt 屏");
    }

    /// 已知缺口（钉成显式断言）：处在 alt 屏时拍的快照不带主屏内容——alacritty 不公开
    /// 非活动缓冲区。claude 从不用 alt 屏（见 claude_fixtures.rs）。哪天这条红了，
    /// 说明 alacritty 开了口子，回来收掉这个缺口。
    #[test]
    fn known_gap_main_screen_is_lost_while_in_alternate_screen() {
        let s = fed(80, 25, b"main\x1b[?1049halt");
        let snap = s.snapshot();
        let mut dst = fed(snap.cols, snap.lines, &snap.bytes);
        dst.feed(b"\x1b[?1049l");
        assert_eq!(dst.screen_rows(), Vec::<String>::new());
    }

    #[test]
    fn round_trip_survives_resizes() {
        let text = format!("{}\r\n短行\r\n{}", "w".repeat(150), "中".repeat(70));
        let mut s = fed(80, 25, text.as_bytes());
        for (cols, lines) in [(60, 25), (160, 25), (100, 10), (80, 25)] {
            assert!(s.resize(cols, lines));
            assert_round_trip(&s, &format!("resize {cols}x{lines}"));
        }
    }

    #[test]
    fn snapshot_does_not_change_the_source() {
        let s = fed(80, 25, b"\x1b[31mred\x1b[3;20r");
        let before = s.snapshot();
        let again = s.snapshot();
        assert_eq!(before, again);
    }
}
```

`crates/term/src/lib.rs` 整个换成：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod input_box;
mod menu;
mod screen;
mod side;
mod snapshot;
mod tail;
mod tracker;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use input_box::{
    BODY_TAIL_CHARS, INPUT_MARKERS, INPUT_SCAN_DEPTH, blocking_dialog, body_tail_visible, input_row,
};
pub use menu::{
    BOX_CHARS, CURSOR_MARKERS, PROMPT_SCAN_DEPTH, PendingDecision, RATE_LIMIT_MENU_PHRASES,
    TRAILING_SLACK, is_confirm_footer, parse_menu, strip,
};
pub use screen::{CursorPos, Screen, ScreenConfig};
pub use snapshot::Snapshot;
pub use tail::AnsiPlainTextTail;
pub use tracker::{DecisionEvent, PendingDecisionTracker, STABLE_WINDOW};
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-term`
Expected: 编译失败，``no method named `snapshot` found``、``no method named `term` found``。

- [ ] **Step 3: 给编码器开一个读格子的口**

在 `crates/term/src/screen.rs` 里，`line_text` 函数末尾的

```rust
        out.truncate(trimmed);
        out
    }
```

**后面**紧接着插入：

```rust
    /// 给快照编码器读格子用。不对 crate 外公开——外面不该依赖 alacritty 的类型。
    pub(crate) fn term(&self) -> &Term<Replies> {
        &self.term
    }
```

- [ ] **Step 4: 写编码器**

放在 `crates/term/src/snapshot.rs` 的测试模块之前：

```rust
//! 快照：把权威终端还原成一段字节，喂进一台**同尺寸**的干净终端就得到同一副样子
//! （回滚 + 屏幕逐格、软换行、滚动区域、模式、光标、当前画笔）。
//! 消费方必须先按 `cols × lines` 建好/resize 终端，再喂 `bytes`。
//!
//! 为什么不录原始字节重放：TUI 靠整屏重绘刷新，几 MB 原始字节里绝大部分是覆盖同一块
//! 屏幕的重绘，换不来 10000 行历史；环形缓冲区的头还会切在转义序列中间。
//!
//! 已知不还原（claude 都用不到，记在计划的遗留里）：处在 alt 屏时的主屏内容
//! （alacritty 不公开非活动缓冲区）、DECSC 存的光标、自定义制表位、G0–G3 字符集、
//! 窗口标题、kitty 键盘模式栈。
use std::fmt::Write as _;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use crate::screen::Screen;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub cols: usize,
    pub lines: usize,
    pub bytes: Vec<u8>,
}

/// 写字时会被复制进格子的那部分属性（= alacritty 光标模板里决定格子长相的部分）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pen {
    pub(crate) fg: Color,
    pub(crate) bg: Color,
    pub(crate) flags: Flags,
    pub(crate) underline: Option<Color>,
    pub(crate) link: Option<Hyperlink>,
}

/// SGR 能表达的格子标志。WRAPLINE / 宽字符标志由写字过程自己产生，不在画笔里。
const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

impl Pen {
    pub(crate) fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags & STYLE_FLAGS,
            underline: cell.underline_color(),
            link: cell.hyperlink(),
        }
    }

    fn reset() -> Self {
        Self::of(&Cell::default())
    }
}

impl Screen {
    /// 拍一张快照。纯读，不改终端状态。
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term();
        let grid = term.grid();
        let mode = *term.mode();
        let cols = grid.columns();
        let lines = grid.screen_lines();
        let mut out = Encoder {
            bytes: Vec::new(),
            pen: Pen::reset(),
        };

        // 1. 硬复位：之后滚动区域、模式、字符集、画笔全是已知初值。
        out.raw("\x1bc");
        if mode.contains(TermMode::ALT_SCREEN) {
            out.raw("\x1b[?1049h");
        }

        // 2. 回滚 + 屏幕，逐行。
        let first = -(grid.history_size() as i32);
        let last = lines as i32 - 1;
        let mut prev_wrapped = false;
        for line in first..=last {
            let row = &grid[Line(line)];
            let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            if line > first && !prev_wrapped {
                // 换行若引起上滚，新行会拿当前背景色填（BCE）——先把背景复位。
                if out.pen.bg != Color::Named(NamedColor::Background) {
                    out.raw("\x1b[49m");
                    out.pen.bg = Color::Named(NamedColor::Background);
                }
                out.raw("\r\n");
            }
            let next_starts_wide = line < last
                && grid[Line(line + 1)][Column(0)]
                    .flags
                    .contains(Flags::WIDE_CHAR);
            out.row(row, cols, wrapped, prev_wrapped, next_starts_wide);
            prev_wrapped = wrapped;
        }

        // 3. 滚动区域（DECSTBM 会把光标拉回原点，所以排在内容之后、定位之前）。
        let (top, bottom) = self.scroll_region();
        if (top, bottom) != (0, lines) {
            out.raw(&format!("\x1b[{};{}r", top + 1, bottom));
        }

        // 4. 光标。原点模式要在定位之前打开（打开它会把光标拉回区域原点）。
        let cursor = self.cursor();
        let row_offset = if mode.contains(TermMode::ORIGIN) {
            out.raw("\x1b[?6h");
            top
        } else {
            0
        };
        if cursor.pending_wrap {
            // 「写满最后一列、还没换行」只能靠真的再写一次那一格造出来。
            let row = &grid[Line(cursor.line as i32)];
            let col = if row[Column(cols - 1)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
            {
                cols - 2
            } else {
                cols - 1
            };
            out.raw(&format!(
                "\x1b[{};{}H",
                cursor.line.saturating_sub(row_offset) + 1,
                col + 1
            ));
            out.cell(&row[Column(col)]);
        } else {
            out.raw(&format!(
                "\x1b[{};{}H",
                cursor.line.saturating_sub(row_offset) + 1,
                cursor.col + 1
            ));
        }

        // 5. 模式（都不动光标、不清 pending-wrap）。
        if mode.contains(TermMode::APP_CURSOR) {
            out.raw("\x1b[?1h");
        }
        if mode.contains(TermMode::APP_KEYPAD) {
            out.raw("\x1b=");
        }
        if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            out.raw("\x1b[?1000h");
        }
        if mode.contains(TermMode::MOUSE_DRAG) {
            out.raw("\x1b[?1002h");
        }
        if mode.contains(TermMode::MOUSE_MOTION) {
            out.raw("\x1b[?1003h");
        }
        if mode.contains(TermMode::UTF8_MOUSE) {
            out.raw("\x1b[?1005h");
        }
        if mode.contains(TermMode::SGR_MOUSE) {
            out.raw("\x1b[?1006h");
        }
        if mode.contains(TermMode::FOCUS_IN_OUT) {
            out.raw("\x1b[?1004h");
        }
        if mode.contains(TermMode::BRACKETED_PASTE) {
            out.raw("\x1b[?2004h");
        }
        if !mode.contains(TermMode::ALTERNATE_SCROLL) {
            out.raw("\x1b[?1007l");
        }
        if !mode.contains(TermMode::URGENCY_HINTS) {
            out.raw("\x1b[?1042l");
        }
        if !mode.contains(TermMode::LINE_WRAP) {
            out.raw("\x1b[?7l");
        }
        if mode.contains(TermMode::INSERT) {
            out.raw("\x1b[4h");
        }
        if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
            out.raw("\x1b[20h");
        }
        if !cursor.visible {
            out.raw("\x1b[?25l");
        }

        // 6. 当前画笔：快照之后程序接着写的字要跟原来一个颜色。
        out.set_pen(&Pen::of(&grid.cursor.template));

        Snapshot {
            cols,
            lines,
            bytes: out.bytes,
        }
    }
}

struct Encoder {
    bytes: Vec<u8>,
    pen: Pen,
}

impl Encoder {
    fn raw(&mut self, s: &str) {
        self.bytes.extend_from_slice(s.as_bytes());
    }

    fn row(
        &mut self,
        row: &alacritty_terminal::grid::Row<Cell>,
        cols: usize,
        wrapped: bool,
        continuation: bool,
        next_starts_wide: bool,
    ) {
        let blank = Cell::default();
        let mut end = if wrapped {
            cols
        } else {
            (0..cols)
                .rev()
                .find(|&c| row[Column(c)] != blank)
                .map_or(0, |c| c + 1)
        };
        // 软换行的续行至少写一格：那一格才真正触发上一行的换行。
        if continuation && end == 0 {
            end = 1;
        }
        for col in 0..end {
            let cell = &row[Column(col)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            // 行尾放不下的宽字符留下的占位格：下一行开头写那个宽字符时会自动再造出来。
            if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                && col == cols - 1
                && wrapped
                && next_starts_wide
            {
                continue;
            }
            if cell.c == '\t' {
                // 制表符在 alacritty 里存成格子里的 '\t'（空白格上才会写）。先按属性写个
                // 空格，回到这一格补一个 HT 让它记成 '\t'，再把光标挪回下一格。
                self.set_pen(&Pen::of(cell));
                self.raw(&format!(" \x1b[{}G\t", col + 1));
                if col + 1 < cols {
                    self.raw(&format!("\x1b[{}G", col + 2));
                }
                continue;
            }
            self.cell(cell);
        }
    }

    fn cell(&mut self, cell: &Cell) {
        self.set_pen(&Pen::of(cell));
        let mut buf = [0u8; 4];
        self.bytes
            .extend_from_slice(cell.c.encode_utf8(&mut buf).as_bytes());
        if let Some(zerowidth) = cell.zerowidth() {
            for c in zerowidth {
                self.bytes
                    .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }

    fn set_pen(&mut self, pen: &Pen) {
        if (&pen.fg, &pen.bg, pen.flags, &pen.underline)
            != (
                &self.pen.fg,
                &self.pen.bg,
                self.pen.flags,
                &self.pen.underline,
            )
        {
            let sgr = sgr(pen);
            self.raw(&sgr);
        }
        if pen.link != self.pen.link {
            match &pen.link {
                Some(link) => {
                    let osc = format!("\x1b]8;id={};{}\x1b\\", link.id(), link.uri());
                    self.raw(&osc);
                }
                None => self.raw("\x1b]8;;\x1b\\"),
            }
        }
        self.pen = pen.clone();
    }
}

/// 一个画笔的完整 SGR。总是从 0 重来，不做差量（差量正是这类代码最爱漏复位的地方）。
pub(crate) fn sgr(pen: &Pen) -> String {
    let mut s = String::from("\x1b[0");
    let f = pen.flags;
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::DOUBLE_UNDERLINE, "4:2"),
        (Flags::UNDERCURL, "4:3"),
        (Flags::DOTTED_UNDERLINE, "4:4"),
        (Flags::DASHED_UNDERLINE, "4:5"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if f.contains(flag) {
            s.push(';');
            s.push_str(code);
        }
    }
    color(&mut s, pen.fg, 30);
    color(&mut s, pen.bg, 40);
    if let Some(underline) = pen.underline {
        color(&mut s, underline, 50);
    }
    s.push('m');
    s
}

/// `base` 30 前景、40 背景、50 下划线色。Named 0–7 → base+n，8–15 → base+60+(n-8)
/// （下划线色没有这两档，走 58;5;n）；Indexed → 38/48/58;5;n；Spec → ;2;r;g;b。
/// 默认前景/背景不写（SGR 0 已复位）。
fn color(s: &mut String, c: Color, base: u16) {
    let extended = base + 8;
    match c {
        Color::Named(n) => {
            let n = n as usize;
            if n < 16 {
                if base == 50 {
                    let _ = write!(s, ";58;5;{n}");
                } else if n < 8 {
                    let _ = write!(s, ";{}", base as usize + n);
                } else {
                    let _ = write!(s, ";{}", base as usize + 60 + n - 8);
                }
            }
        }
        Color::Indexed(i) => {
            let _ = write!(s, ";{extended};5;{i}");
        }
        Color::Spec(rgb) => {
            let _ = write!(s, ";{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b);
        }
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `88 passed`（单测）+ `11 passed`（claude_fixtures）。

- [ ] **Step 6: 验一把尺子会红**

逐条临时改坏、跑 `cargo test -p crewd-term --lib snapshot`、看到红、再改回（用编辑器撤销，**别用 `git checkout`**——它会连未提交的实现一起丢掉）：
- `if continuation && end == 0 {` 改成 `if false {` → `blank_wrapped_continuation_is_not_lost` 红
- `if out.pen.bg != Color::Named(NamedColor::Background) {` 改成 `if false {` → `background_does_not_bleed_into_scrolled_lines` 红
- `if cursor.pending_wrap {` 改成 `if false {` → `pending_wrap_cursor_is_reproduced` 红
- `out.set_pen(&Pen::of(&grid.cursor.template));` 删掉 → `current_pen_and_hyperlink_are_restored`、`erased_cells_keep_their_background` 红

全部改回后再跑一次：`88 passed`。

- [ ] **Step 7: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/src/snapshot.rs crates/term/src/screen.rs crates/term/src/lib.rs
git commit -m "crewd-term: snapshot encoder with cell-exact round-trip tests

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 真语料往返 + 底行悬空折行

用 4 个夹具验快照：不整齐分批（1/7/64/3/128/17/256/5 循环，转义序列会被切在两批之间）喂，**每批之后**都往返一次；改尺寸（60×25、160×25、80×10、80×25）后往返；还原后两边一起 resize 比 reflow；快照之后接着喂剩余字节两边一致（只在 `\n` 之后切，原因见遗留第 1 条）。

`80×10` 那次会红：竖向缩小后，最底一行的 `WRAPLINE` 指向一行已经被挤出屏幕的续行，逐行写只能停在 pending-wrap，标志造不出来。修法：在 `2..lines` 的滚动区域里真的折一次（区域不含第 0 行，上滚出去的第 1 行不进回滚），`SD` 滚回来，再补写第 1 行（第 1 行自己也折行时，重写一次第 2 行行首那格把标志造回来）。不足 3 行时做不到，记为已知缺口。

**Files:**
- Create: `crewd/crates/term/src/snapshot_fixtures.rs`
- Modify: `crewd/crates/term/src/snapshot.rs`、`crewd/crates/term/src/lib.rs`、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`

**Interfaces:**
- Consumes: Task 6 的 `tests/fixtures/*.bin`；Task 7 的 `snapshot::tests::{assert_round_trip, assert_same}`、`Encoder::{set_pen, raw, cell, row}`、`Pen::reset()`；`Screen::{new, feed, snapshot, resize}`。
- Produces: `Screen::snapshot` 在最底一行悬空折行时也逐格还原（`lines >= 3`）。公开 API 不变。

- [ ] **Step 1: 写失败的测试**

`crates/term/src/snapshot_fixtures.rs`（整个文件）：

```rust
//! 真 TUI 语料的快照往返：分批喂（批大小故意不整齐，转义序列会被切在两批之间），
//! 每批之后都拍一次快照并逐格比对；再验改尺寸前后、以及快照之后接着喂剩余字节。
use crate::screen::{Screen, ScreenConfig};
use crate::snapshot::tests::{assert_round_trip, assert_same};

const FIXTURES: [(&str, &[u8]); 4] = [
    (
        "tui-claude.bin",
        include_bytes!("../tests/fixtures/tui-claude.bin"),
    ),
    (
        "tui-claude-trust.bin",
        include_bytes!("../tests/fixtures/tui-claude-trust.bin"),
    ),
    (
        "tui-claude-ready.bin",
        include_bytes!("../tests/fixtures/tui-claude-ready.bin"),
    ),
    (
        "tui-shell.bin",
        include_bytes!("../tests/fixtures/tui-shell.bin"),
    ),
];

const BATCHES: [usize; 8] = [1, 7, 64, 3, 128, 17, 256, 5];

fn screen() -> Screen {
    Screen::new(ScreenConfig::default())
}

/// 把 `bytes` 按 BATCHES 循环切块，返回每块的结束偏移。
fn batch_ends(len: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut offset = 0;
    for size in BATCHES.iter().cycle() {
        if offset >= len {
            break;
        }
        offset = (offset + size).min(len);
        ends.push(offset);
    }
    ends
}

#[test]
fn every_fixture_round_trips_after_every_batch() {
    for (name, bytes) in FIXTURES {
        let mut src = screen();
        let mut start = 0;
        let ends = batch_ends(bytes.len());
        assert!(ends.len() >= 10, "{name}：至少要在十个时刻核过");
        for end in ends {
            src.feed(&bytes[start..end]);
            start = end;
            assert_round_trip(&src, &format!("{name} 喂到第 {end} 字节"));
        }
    }
}

#[test]
fn stream_continues_identically_after_a_snapshot() {
    for (name, bytes) in FIXTURES {
        // 只在换行符之后切：快照带不走解析器的半截状态（切在 UTF-8 字符或转义序列
        // 中间时，剩下的字节在新终端里会被当成乱码）——那是 attach 协议（M2d）要对齐的事。
        let cuts = (1..bytes.len()).filter(|&i| bytes[i - 1] == b'\n');
        for cut in cuts {
            let mut src = screen();
            src.feed(&bytes[..cut]);
            let snap = src.snapshot();
            let mut dst = screen();
            dst.feed(&snap.bytes);
            src.feed(&bytes[cut..]);
            dst.feed(&bytes[cut..]);
            assert_same(&src, &dst, &format!("{name} 在第 {cut} 字节接上后喂完"));
        }
    }
}

#[test]
fn fixtures_round_trip_after_resizes() {
    for (name, bytes) in FIXTURES {
        let mut src = screen();
        src.feed(bytes);
        for (cols, lines) in [(60, 25), (160, 25), (80, 10), (80, 25)] {
            assert!(src.resize(cols, lines));
            assert_round_trip(&src, &format!("{name} resize {cols}x{lines}"));
        }
    }
}

#[test]
fn restored_screen_reflows_like_the_original() {
    for (name, bytes) in FIXTURES {
        let mut src = screen();
        src.feed(bytes);
        let snap = src.snapshot();
        let mut dst = screen();
        dst.feed(&snap.bytes);
        for cols in [60, 160, 80] {
            assert!(src.resize(cols, 25));
            assert!(dst.resize(cols, 25));
            assert_same(&src, &dst, &format!("{name} 还原后 resize 到 {cols} 列"));
        }
    }
}
```

`crates/term/src/lib.rs` 整个换成：

```rust
//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod input_box;
mod menu;
mod screen;
mod side;
mod snapshot;
#[cfg(test)]
mod snapshot_fixtures;
mod tail;
mod tracker;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use input_box::{
    BODY_TAIL_CHARS, INPUT_MARKERS, INPUT_SCAN_DEPTH, blocking_dialog, body_tail_visible, input_row,
};
pub use menu::{
    BOX_CHARS, CURSOR_MARKERS, PROMPT_SCAN_DEPTH, PendingDecision, RATE_LIMIT_MENU_PHRASES,
    TRAILING_SLACK, is_confirm_footer, parse_menu, strip,
};
pub use screen::{CursorPos, Screen, ScreenConfig};
pub use snapshot::Snapshot;
pub use tail::AnsiPlainTextTail;
pub use tracker::{DecisionEvent, PendingDecisionTracker, STABLE_WINDOW};
```

在 `crates/term/src/snapshot.rs` 的测试模块里、

```rust
    #[test]
    fn wrap_on_last_row_while_scrollback_grows() {
```

那两行**之前**插入：

```rust
    #[test]
    fn dangling_wrap_on_the_bottom_row_survives() {
        // 竖向缩小后，最底一行的 WRAPLINE 指向一行已经不在屏上的续行。
        let text = format!("top\r\nmid\r\n{}", "a".repeat(85));
        let mut s = fed(80, 25, text.as_bytes());
        s.feed(b"\x1b[1;1H");
        assert!(s.resize(80, 3));
        let bottom = &s.term().grid()[Line(2)];
        assert!(
            bottom[Column(79)].flags.contains(Flags::WRAPLINE),
            "前置条件：底行悬空折行"
        );
        assert_round_trip(&s, "底行悬空折行");

        // 第 1 行自己也折进底行：补写第 1 行时要把它的折行标志一并造回来。
        let mut s = fed(80, 25, format!("top\r\n{}", "a".repeat(250)).as_bytes());
        s.feed(b"\x1b[1;1H");
        assert!(s.resize(80, 3));
        let grid = s.term().grid();
        assert!(grid[Line(1)][Column(79)].flags.contains(Flags::WRAPLINE));
        assert!(grid[Line(2)][Column(79)].flags.contains(Flags::WRAPLINE));
        assert_round_trip(&s, "第 1 行与底行都折行");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-term --lib snapshot`
Expected: 恰好 2 个失败：`snapshot::tests::dangling_wrap_on_the_bottom_row_survives` 与 `snapshot_fixtures::fixtures_round_trip_after_resizes`（报 `tui-claude.bin resize 80x10：第 9 行第 79 格`，左边带 `WRAPLINE`、右边没有）。其余全过。

- [ ] **Step 3: 实现悬空折行的还原**

在 `crates/term/src/snapshot.rs` 的 `snapshot()` 里，

```rust
        // 3. 滚动区域（DECSTBM 会把光标拉回原点，所以排在内容之后、定位之前）。
```

这一行**之前**插入：

```rust
        // 2b. 最底一行带 WRAPLINE 却没有下一行（竖向缩小后常见）：直接写只能停在
        //     pending-wrap，标志要靠真的折下去才有。在第 2..lines 行的滚动区域里折一次
        //     （区域不含第 0 行 → 上滚的行不进回滚），SD 滚回来，再补写被挤掉的第 1 行。
        //     少于 3 行时做不到（区域至少两行），留作已知缺口。
        let bottom_row = &grid[Line(last)];
        if lines >= 3 && bottom_row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
            let col = if bottom_row[Column(cols - 1)]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
            {
                cols - 2
            } else {
                cols - 1
            };
            out.set_pen(&Pen::reset());
            out.raw(&format!("\x1b[2;{lines}r\x1b[{lines};{}H", col + 1));
            out.cell(&bottom_row[Column(col)]);
            out.set_pen(&Pen::reset());
            out.raw(" \x1b[T\x1b[r\x1b[2;1H");
            let row1 = &grid[Line(1)];
            let wrapped1 = row1[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            let row2 = &grid[Line(2)];
            let row2_wide = row2[Column(0)].flags.contains(Flags::WIDE_CHAR);
            out.row(row1, cols, wrapped1, false, row2_wide);
            if wrapped1 {
                // 再写一次第 2 行行首那格（内容不变），触发第 1 行的折行标志。
                out.cell(&row2[Column(0)]);
            }
        }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-term`
Expected: `93 passed`（单测，语料往返约 2 秒）+ `11 passed`（claude_fixtures）。

- [ ] **Step 5: 验这把尺子会红**

把刚插入的 `if lines >= 3 && bottom_row` 临时改成 `if false && bottom_row` → 跑 `cargo test -p crewd-term --lib snapshot` → 上面那 2 个红；把 `if wrapped1 {` 临时改成 `if false {` → 同样那 2 个红。都改回（编辑器撤销），再跑一次全绿。

- [ ] **Step 6: 写交付记录**

在 `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` 文件末尾追加：

```markdown
## 15. M2a 交付记录与遗留（分支 crewd-m2a，计划 `docs/superpowers/plans/2026-10-06-crewd-m2a-terminal-core.md`）

已交付：`crewd/crates/term`（crewd-term，纯计算，不起进程、不用 tokio）——alacritty_terminal 0.26 无头终端（`Processor<NoSync>`、终端回复发件箱、OSC 10/11 用配置色、每个 `?1004h` 补 `ESC[I`、旁路跟踪滚动区域）、TerminalScreenText 口径读屏、AnsiPlainTextTail（UTF-8 跨块拼接）、菜单解析（编号式 + 光标列表式）、输入框判据、PendingDecisionTracker（3s、按指纹粘住、时间由调用方传入）、快照编码器（逐格往返：回滚 + 屏、SGR + OSC 8、软换行含竖向缩小后的悬空底行、宽字符、制表符、滚动区域、模式、光标含 pending-wrap、当前画笔）。验收：4 个夹具上的 13 条期望；夹具每批往返、resize 往返、快照后接着喂两边一致。

遗留（M2b–M2d 处理）：
- 快照带不走解析器的半截状态：PTY 块切在 UTF-8 字符或转义序列中间时拍快照，接在后面的实时字节在客户端会坏掉一个字符或一个序列。M2d 定 attach 协议时二选一：只在解析器处于 ground 状态时取快照，或把半截字节随快照一起交给客户端。
- 快照不还原：处在 alt 屏时的主屏（alacritty 不公开非活动缓冲区；测试钉成显式断言）、DECSC 存的光标、自定义制表位、G0–G3 字符集、窗口标题、kitty 键盘模式栈；不足 3 行时的悬空底行折行；reflow 后行尾 `LEADING_WIDE_CHAR_SPACER` 而下一行不以宽字符开头时的该标志。夹具里只有 claude 启动最开头的一对 DECSC/DECRC（字节 0–5），其余几项都没出现。
- DA1/DA2 的回答串与 SwiftTerm 不同（alacritty 回 `ESC[?6c`）。这只在录制字节上验过，没有和真 claude 交互过——M2b 接上 PTY 后真机核一次 claude 的启动与主题选择。
- 群消息渲染（`SessionDecisionNotice`，按 `numbered` 分叉、绝不编造编号）归 M3。
```

- [ ] **Step 7: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/term/src/snapshot_fixtures.rs crates/term/src/snapshot.rs crates/term/src/lib.rs ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git commit -m "crewd-term: corpus snapshot round-trips; restore dangling bottom-row wrap

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 覆盖对照（M2a 范围 → 任务）

| 范围项 | 任务 |
|---|---|
| `Screen`：`Processor<NoSync>`、回复发件箱（PtyWrite / ColorRequest 用配置色 / TextAreaSizeRequest）、resize、回滚 10_000、缺省 80×25 | 1 |
| `?1004h` 补 `ESC[I`；滚动区域旁路跟踪 | 2 |
| 读屏（跳 spacer、未写=空格、去尾空格、去尾空行） | 1 |
| AnsiPlainTextTail（4K 窗、小写镜像） | 3 |
| 菜单解析（编号式 + 光标列表式、问句、限流排除、指纹）+ `input_row` / `body_tail_visible` / `blocking_dialog` | 4 |
| PendingDecisionTracker（3s、按指纹粘住、注入时间） | 5 |
| 13 条夹具期望 | 6 |
| 快照编码器（`ESC c`、回滚+屏、SGR、软换行、滚动区域、模式、光标含 pending-wrap、隐藏光标）+ 合成往返 | 7 |
| 夹具多点往返 + resize 往返 | 8 |

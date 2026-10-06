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

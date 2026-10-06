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

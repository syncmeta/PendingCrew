//! crewd 的无头终端核心：把 PTY 字节喂进 alacritty_terminal，读屏、认对话框、拍快照。
//! 纯计算：不起进程、不碰 tokio、不读时钟（时间一律由调用方传入）。
mod screen;
mod side;
mod tail;

pub use alacritty_terminal::vte::ansi::Rgb;
pub use screen::{CursorPos, Screen, ScreenConfig};
pub use tail::AnsiPlainTextTail;

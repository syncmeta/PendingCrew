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

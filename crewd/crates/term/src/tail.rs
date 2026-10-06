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

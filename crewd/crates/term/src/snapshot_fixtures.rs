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

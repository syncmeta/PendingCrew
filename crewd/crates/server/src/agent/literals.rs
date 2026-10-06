//! 尺子：给 agent 看的文字只在 md 里（spec §22.3 验收「源码里没有工具描述 / 世界观字面量」）。
//! 扫 agent 模块与 collab 两个文件的**非测试代码里的字符串字面量**（注释不算），出现 CJK 就红；
//! `testkit.rs` 是测试夹具（不进产品代码），不扫。
//! 另外把源码里出现的每个 `"r.…"` / `"trace.…"` / `"inject.…"` 键都核一遍 md 里有。
#![cfg(test)]

use std::path::{Path, PathBuf};

use crewd_collab::Locale;

fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    let mut stack = vec![root.join("src/agent")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && !p.ends_with("literals.rs")
                && !p.ends_with("testkit.rs")
            {
                out.push(p);
            }
        }
    }
    for f in ["worldview.rs", "turn_trace.rs"] {
        out.push(root.join("../collab/src").join(f));
    }
    assert!(out.len() >= 4, "扫描范围不对：{out:?}");
    out
}

/// 非测试部分：第一个 `#[cfg(test)]` 之前（各文件的测试模块都放在末尾）。
fn non_test(src: &str) -> &str {
    src.find("#[cfg(test)]").map_or(src, |i| &src[..i])
}

/// 取出全部字符串字面量的内容（普通 / 原始 / 字节串），跳过行注释、块注释与字符字面量。
fn string_literals(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        match b[i] {
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            'r' if matches!(b.get(i + 1), Some('"') | Some('#'))
                && (i == 0 || !b[i - 1].is_alphanumeric() && b[i - 1] != '_') =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                let start = j;
                loop {
                    if j >= b.len() {
                        break;
                    }
                    if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                        break;
                    }
                    j += 1;
                }
                out.push(b[start..j.min(b.len())].iter().collect());
                i = j + 1 + hashes;
            }
            '"' => {
                let mut j = i + 1;
                let mut s = String::new();
                while j < b.len() && b[j] != '"' {
                    if b[j] == '\\' {
                        j += 1;
                    }
                    if j < b.len() {
                        s.push(b[j]);
                    }
                    j += 1;
                }
                out.push(s);
                i = j + 1;
            }
            '\'' => {
                // 字符字面量 'x' / '\n' / '\''；生命周期 'a 不闭合，原样跳过一个字符。
                if b.get(i + 1) == Some(&'\\') {
                    let mut j = i + 2;
                    while j < b.len() && b[j] != '\'' {
                        j += 1;
                    }
                    i = j + 1;
                } else if b.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF)
}

#[test]
fn tokenizer_sees_strings_not_comments() {
    let src = "// 注释里的「中文」\nlet a = \"x\"; /* 块 */ let b = r#\"y\"#; let c = '\"'; let d: &'static str = \"z\";";
    assert_eq!(string_literals(src), ["x", "y", "z"]);
}

#[test]
fn no_agent_facing_literals_in_source() {
    for f in sources() {
        let src = std::fs::read_to_string(&f).unwrap();
        for lit in string_literals(non_test(&src)) {
            assert!(
                !lit.chars().any(is_cjk),
                "{} 的字符串字面量里有给人看的文字（应放进 prompts/*.md）：{lit:?}",
                f.display()
            );
        }
    }
}

#[test]
fn every_text_key_used_in_source_exists_in_both_locales() {
    let prefixes = ["r.", "trace.", "inject."];
    let mut used = std::collections::BTreeSet::new();
    for f in sources() {
        let src = std::fs::read_to_string(&f).unwrap();
        for lit in string_literals(&src) {
            if prefixes.iter().any(|p| lit.starts_with(p))
                && lit
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
            {
                used.insert(lit);
            }
        }
    }
    assert!(
        used.contains("r.unknown_tool"),
        "尺子没扫到已知的键：{used:?}"
    );
    for loc in [Locale::Zh, Locale::En] {
        let have = crate::agent::text::section_names(loc);
        for k in &used {
            assert!(have.iter().any(|h| h == k), "{loc:?} 的 tools md 缺 ## {k}");
        }
    }
}

//! 给 agent 与群看的文字全在 `prompts/orchestra.{zh,en}.md`（编译期 include_str!），代码里不写字面量。
//! 格式同 `crates/jev/src/prompts.rs`：第一个 `## 名字` 之前是给人看的注释；每节到下一个 `## ` 为止、
//! 去首尾空白；节名重复、节为空都是错误。占位符 `{名字}` 由 [`render`] 单遍替换。
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

/// 每一节与它用到的占位符。加一节 = 两份 md 各加一节 + 这里加一行（测试钉住三处一致）。
pub const SECTIONS: &[(&str, &[&str])] = &[
    ("restart.interrupted", &[]),
    ("resume.refused.group", &["name", "said"]),
    ("resume.refused.managed", &["name", "id", "said"]),
    ("resume.refused.diag.found", &["path"]),
    ("resume.refused.diag.missing", &[]),
    ("resume.refused.agent", &["id", "said"]),
    ("resume.failed.group", &["name", "detail"]),
    ("quota.wake", &[]),
    (
        "loop.tripped.human",
        &["target", "origin", "count", "minutes"],
    ),
    (
        "loop.chain.tripped.human",
        &["target", "origin", "chain", "hop", "limit"],
    ),
    (
        "loop.chain.unresolved.human",
        &["target", "origin", "chain"],
    ),
    ("origin.system", &[]),
    ("captain.changed.new", &[]),
    ("captain.changed.old", &["name"]),
    ("decision.session", &["session"]),
    ("hibernate.resume", &[]),
    ("hibernate.doing", &["doing"]),
    ("hibernate.doing.unknown", &[]),
    ("hibernate.finished", &[]),
    ("hibernate.interrupted", &[]),
    ("decision.first", &["name"]),
    ("decision.howto", &[]),
    ("notice.recovered", &["name"]),
    ("notice.runner", &["name", "detail"]),
    ("notice.give_up.crew", &["sessions", "error"]),
    ("notice.quota.captain", &["name", "executor"]),
    ("notice.binary_replaced", &[]),
    ("notice.input_draft", &["name"]),
    ("session.input_draft_unknown", &["name"]),
    ("brief.undelivered", &["name"]),
];

type Sections = BTreeMap<&'static str, &'static str>;

fn parse(name: &'static str, text: &'static str) -> Result<Sections, String> {
    let mut sections = BTreeMap::new();
    let mut current: Option<(&'static str, usize)> = None;
    let mut offset = 0;
    let mut close = |cur: Option<(&'static str, usize)>, end: usize| -> Result<(), String> {
        if let Some((key, start)) = cur {
            let body = text[start..end].trim();
            if body.is_empty() {
                return Err(format!("prompts/{name}.md 的 ## {key} 是空的"));
            }
            if sections.insert(key, body).is_some() {
                return Err(format!("prompts/{name}.md 的 ## {key} 重复"));
            }
        }
        Ok(())
    };
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("## ") {
            close(current.take(), offset)?;
            let key = rest.trim();
            if key.is_empty() {
                return Err(format!("prompts/{name}.md 有一个没有名字的 ##"));
            }
            current = Some((key, offset + line.len()));
        }
        offset += line.len();
    }
    close(current, text.len())?;
    Ok(sections)
}

static ZH: LazyLock<Sections> = LazyLock::new(|| {
    parse("orchestra.zh", include_str!("../prompts/orchestra.zh.md"))
        .unwrap_or_else(|e| panic!("{e}"))
});
static EN: LazyLock<Sections> = LazyLock::new(|| {
    parse("orchestra.en", include_str!("../prompts/orchestra.en.md"))
        .unwrap_or_else(|e| panic!("{e}"))
});

fn sections(lang: Lang) -> &'static Sections {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

/// 缺节 = 程序员错误（`both_files_have_exactly_the_declared_sections` 钉住），直接 panic 并说清是哪一节。
pub fn get(lang: Lang, key: &str) -> &'static str {
    match sections(lang).get(key) {
        Some(text) => text,
        None => panic!("prompts/orchestra.{lang:?}.md 缺少一节：## {key}"),
    }
}

pub fn render(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    fill(get(lang, key), vars)
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// 单遍替换 `{名字}`（名字只含小写字母与下划线）；`vars` 里没有的占位符原样留下（测试抓得到）。
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_ident(&after[..close]) => {
                let key = &after[..close];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 模板里出现的占位符名（去重、按出现顺序）。
pub fn placeholders(template: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_ident(&after[..close]) => {
                let key = after[..close].to_string();
                if !found.contains(&key) {
                    found.push(key);
                }
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn names(lang: Lang) -> BTreeSet<&'static str> {
        sections(lang).keys().copied().collect()
    }

    #[test]
    fn both_files_have_exactly_the_declared_sections() {
        let declared: BTreeSet<&str> = SECTIONS.iter().map(|(k, _)| *k).collect();
        assert_eq!(names(Lang::Zh), declared, "zh");
        assert_eq!(names(Lang::En), declared, "en");
    }

    #[test]
    fn every_section_uses_exactly_its_declared_placeholders() {
        for lang in [Lang::Zh, Lang::En] {
            for (key, vars) in SECTIONS {
                let used: BTreeSet<String> = placeholders(get(lang, key)).into_iter().collect();
                let want: BTreeSet<String> = vars.iter().map(|v| v.to_string()).collect();
                assert_eq!(used, want, "{lang:?} ## {key}");
            }
        }
    }

    #[test]
    fn render_fills_every_placeholder() {
        for lang in [Lang::Zh, Lang::En] {
            for (key, vars) in SECTIONS {
                let values: Vec<(&str, &str)> = vars.iter().map(|v| (*v, "X")).collect();
                let out = render(lang, key, &values);
                assert!(placeholders(&out).is_empty(), "{lang:?} ## {key}: {out}");
            }
        }
    }

    #[test]
    fn fill_is_single_pass_and_leaves_unknown_placeholders() {
        // 替换进来的值里的 {…} 不会再被展开（显示名 / claude 原话是外来文本）。
        assert_eq!(fill("{a}|{b}", &[("a", "{b}"), ("b", "x")]), "{b}|x");
        assert_eq!(fill("{zz} {a}", &[("a", "1")]), "{zz} 1");
        assert_eq!(fill("{ 不是 } {}", &[]), "{ 不是 } {}");
    }

    #[test]
    #[should_panic(expected = "缺少一节")]
    fn missing_section_is_a_loud_programmer_error() {
        get(Lang::Zh, "no.such.section");
    }

    #[test]
    fn parse_rejects_empty_and_duplicate_sections() {
        assert!(
            parse("t", "## a\n\n## b\nx\n")
                .unwrap_err()
                .contains("## a 是空的")
        );
        assert!(
            parse("t", "## a\nx\n## a\ny\n")
                .unwrap_err()
                .contains("## a 重复")
        );
        assert_eq!(parse("t", "注释\n## a\n x \n").unwrap()["a"], "x");
    }
}

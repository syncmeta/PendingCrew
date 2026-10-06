//! 给人 / agent 看的文字全在 `prompts/render.{zh,en}.md`（编译期 include_str!），代码里不写这些字面量。
//! 格式同 `crates/jev/src/prompts.rs`：第一个 `## 名字` 之前是注释；之后每节到下一个 `## ` 为止，去掉首尾空白。
use crate::Locale;
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug)]
pub struct PromptFile {
    pub name: &'static str,
    sections: BTreeMap<&'static str, &'static str>,
}

impl PromptFile {
    pub fn get(&self, section: &str) -> &'static str {
        match self.sections.get(section) {
            Some(t) => t,
            None => panic!("prompts/{}.md 缺少一节：## {section}", self.name),
        }
    }
    pub fn section_names(&self) -> Vec<&'static str> {
        self.sections.keys().copied().collect()
    }
}

pub fn parse(name: &'static str, text: &'static str) -> Result<PromptFile, String> {
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
    Ok(PromptFile { name, sections })
}

static ZH: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("render.zh", include_str!("../prompts/render.zh.md")).unwrap_or_else(|e| panic!("{e}"))
});
static EN: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("render.en", include_str!("../prompts/render.en.md")).unwrap_or_else(|e| panic!("{e}"))
});

pub fn render_file(l: Locale) -> &'static PromptFile {
    match l {
        Locale::Zh => &ZH,
        Locale::En => &EN,
    }
}

/// 节名 → 允许出现的占位符。两种语言都必须有这些节，且只用这些占位符。
pub const REQUIRED: &[(&str, &[&str])] = &[
    ("notice.approval", &["session", "tool", "summary"]),
    ("notice.restart", &["count"]),
    ("notice.restart.item", &["name"]),
    ("sender.human", &[]),
    ("sender.system", &[]),
    ("sender.session", &["name", "ext"]),
    ("sender.session.unnumbered", &["name"]),
    ("sender.captain", &["name"]),
    ("inject.header", &[]),
    ("inject.channel.lobby", &["crew"]),
    ("inject.channel.human", &["crew"]),
    ("inject.channel.topic", &["name"]),
    ("inject.channel.dm", &[]),
    ("line", &["at", "sender", "text"]),
    ("line.directed", &["targets"]),
    ("line.directed.unknown", &[]),
    ("line.reply", &["reply"]),
    ("wake.header", &[]),
    ("wake.message", &[]),
    ("wake.timer", &[]),
    ("wake.ask_answer", &["ask", "question", "answer"]),
    ("wake.ask_defaulted", &["ask", "question", "answer"]),
    ("wake.approval_allowed", &["tool", "summary"]),
    ("wake.approval_denied", &["tool", "summary"]),
    ("wake.resume_note", &["note"]),
    ("wake.restart", &[]),
    ("wake.system", &[]),
    ("wake.checkpoint", &[]),
    ("wake.stop_check", &[]),
    ("wake.note", &["note"]),
];

/// 把 `{名字}` 换成值。**一遍扫描**：只扫模板，值原样拷贝、不再被后面的变量展开（agent / 人写的正文里有
/// `{name}` 这类花括号时不能串，M3b G-F）。没给的占位符原样留着（测试保证模板只用允许的占位符）。
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => {
                let key = &after[..j];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// 人类 / crewd 的发言者标签留给它们自己（终审 I5）：session 的标题、显示名与 topic 名不能是任一语言里的
/// `sender.human` / `sender.system`（去首尾空白、压换行后比，不分大小写），否则注入面上它的发言就像人说的。
pub(crate) fn refuse_sender_label(what: &str, value: &str) -> crate::Result<()> {
    let v = crate::render::flat(value).trim().to_lowercase();
    for l in [Locale::Zh, Locale::En] {
        let f = render_file(l);
        for label in ["sender.human", "sender.system"] {
            if v == f.get(label).to_lowercase() {
                return Err(crate::CollabError::Invalid(format!(
                    "{what} {value:?} 与发言者标签 {:?} 相同，会被当成人类 / crewd 的发言",
                    f.get(label)
                )));
            }
        }
    }
    Ok(())
}

/// 模板里出现的全部 `{名字}`。
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => {
                out.push(after[..j].to_string());
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_have_every_section_with_only_allowed_placeholders() {
        for l in [Locale::Zh, Locale::En] {
            let f = render_file(l);
            for (section, allowed) in REQUIRED {
                let t = f.get(section);
                for p in placeholders(t) {
                    assert!(
                        allowed.contains(&p.as_str()),
                        "{}.md ## {section} 用了不允许的占位符 {{{p}}}",
                        f.name
                    );
                }
            }
        }
        assert_eq!(
            render_file(Locale::Zh).section_names(),
            render_file(Locale::En).section_names(),
            "zh / en 节名集合一致"
        );
    }

    #[test]
    fn fill_replaces_named_placeholders() {
        assert_eq!(fill("{a} 和 {b}", &[("a", "甲"), ("b", "乙")]), "甲 和 乙");
        assert_eq!(placeholders("x {a} y {b}"), ["a", "b"]);
    }

    #[test]
    fn fill_does_not_expand_inside_values() {
        // 值里的花括号是正文，不是占位符（M3b G-F）：逐个 replace 会把 {b} 再换成「乙」。
        assert_eq!(
            fill("{a} / {b}", &[("a", "原文 {b}"), ("b", "乙")]),
            "原文 {b} / 乙"
        );
        assert_eq!(
            fill("{x} 留着，{a", &[("a", "甲")]),
            "{x} 留着，{a",
            "没给的与不闭合的原样留"
        );
    }

    #[test]
    #[should_panic(expected = "缺少一节")]
    fn missing_section_names_itself() {
        render_file(Locale::Zh).get("nope");
    }
}

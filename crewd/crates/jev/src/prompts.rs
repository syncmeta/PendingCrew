//! 给 Jev 看的文字全在 `prompts/*.md`（编译期 include_str!），代码里不写问题 / 说明 / 选项描述的字面量。
//! 文件格式：第一个 `## 名字` 之前的内容是给人看的注释，忽略；之后每个 `## 名字` 开一节，
//! 节内容 = 到下一个 `## ` 为止的文本去掉首尾空白。节名重复、节内容为空都是错误。
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug)]
pub struct PromptFile {
    pub name: &'static str,
    sections: BTreeMap<&'static str, &'static str>,
}

impl PromptFile {
    /// 缺节 = 程序员错误（`every_prompt_file_has_its_sections` 测试钉住），直接 panic 并说清是哪一节。
    pub fn get(&self, section: &str) -> &'static str {
        match self.sections.get(section) {
            Some(text) => text,
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

fn load(name: &'static str, text: &'static str) -> PromptFile {
    parse(name, text).unwrap_or_else(|e| panic!("{e}"))
}

pub static ROUTE_TODO: LazyLock<PromptFile> =
    LazyLock::new(|| load("route_todo", include_str!("../prompts/route_todo.md")));
pub static WAKE: LazyLock<PromptFile> =
    LazyLock::new(|| load("wake", include_str!("../prompts/wake.md")));
pub static ATTRIBUTE: LazyLock<PromptFile> =
    LazyLock::new(|| load("attribute", include_str!("../prompts/attribute.md")));
pub static URGENCY: LazyLock<PromptFile> =
    LazyLock::new(|| load("urgency", include_str!("../prompts/urgency.md")));
pub static VOCAB: LazyLock<PromptFile> =
    LazyLock::new(|| load("vocab", include_str!("../prompts/vocab.md")));

/// 每个文件必须有的节。决策构造器只读这里列出的节。
pub const REQUIRED: &[(&str, &[&str])] = &[
    (
        "route_todo",
        &[
            "instructions",
            "option.captain",
            "option.new_session",
            "option.new_crew",
        ],
    ),
    ("wake", &["question", "true", "false"]),
    ("attribute", &["instructions", "option.none"]),
    (
        "urgency",
        &[
            "instructions",
            "level.0",
            "level.1",
            "level.2",
            "level.3",
            "level.4",
        ],
    ),
    (
        "vocab",
        &[
            "status.busy",
            "status.idle",
            "recency.recent",
            "recency.middle",
            "recency.stale",
            "kind.message",
            "kind.todo",
            "kind.work_item",
            "kind.channel",
            "wait.minutes",
            "wait.hours",
            "wait.day",
            "wait.days",
            "mentions.none",
            "mentions.some",
            "mentions.many",
        ],
    ),
];

pub fn by_name(name: &str) -> Option<&'static PromptFile> {
    match name {
        "route_todo" => Some(&ROUTE_TODO),
        "wake" => Some(&WAKE),
        "attribute" => Some(&ATTRIBUTE),
        "urgency" => Some(&URGENCY),
        "vocab" => Some(&VOCAB),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prompt_file_has_its_sections() {
        for (name, required) in REQUIRED {
            let file = by_name(name).unwrap_or_else(|| panic!("REQUIRED 里有未知文件 {name}"));
            for section in *required {
                assert!(
                    file.section_names().contains(section),
                    "prompts/{name}.md 缺 ## {section}"
                );
            }
        }
    }

    #[test]
    fn prompt_texts_carry_no_digits_that_could_be_mistaken_for_data() {
        // jaggedness #2/#3：数字、日期交给代码。提示词自己也不写数字，免得被当成状态里的事实。
        for (name, required) in REQUIRED {
            let file = by_name(name).unwrap();
            for section in *required {
                let text = file.get(section);
                assert!(
                    !text.chars().any(|c| c.is_ascii_digit()),
                    "prompts/{name}.md ## {section} 含数字：{text}"
                );
            }
        }
    }

    #[test]
    fn parser_rejects_empty_and_duplicate_sections() {
        assert!(parse("t", "## a\n\n## b\nx\n").is_err());
        assert!(parse("t", "## a\nx\n## a\ny\n").is_err());
        assert!(parse("t", "## \nx\n").is_err());
        let ok = parse("t", "注释\n## a\n第一行\n第二行\n\n## b\ny").unwrap();
        assert_eq!(ok.get("a"), "第一行\n第二行");
        assert_eq!(ok.get("b"), "y");
        assert_eq!(ok.section_names(), ["a", "b"]);
    }

    #[test]
    #[should_panic(expected = "prompts/t.md 缺少一节：## nope")]
    fn missing_section_names_itself() {
        parse("t", "## a\nx").unwrap().get("nope");
    }
}

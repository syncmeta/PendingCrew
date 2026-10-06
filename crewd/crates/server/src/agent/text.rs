//! `crates/collab/prompts/tools.{zh,en}.md` 的取用（加载在 `crewd_collab::worldview::tools_file`）。节：`## <工具>`、
//! `## <工具>.<参数>`、`## r.*`（回执）、`## trace.*`（代发稿）、`## inject.*`（注入块补充）。占位符是 M3a 的 `{名字}`。
pub use crewd_collab::Locale;
use crewd_collab::prompts::fill;
use crewd_collab::worldview::tools_file;

/// 缺节 = 程序员错误（`every_text_key_used_in_source_exists_in_both_locales` 钉住），panic 写明文件与节。
pub fn get(loc: Locale, key: &str) -> &'static str {
    tools_file(loc).get(key)
}

/// 取一节并填 `{占位符}`。
pub fn r(loc: Locale, key: &str, vars: &[(&str, &str)]) -> String {
    fill(get(loc, key), vars)
}

pub fn section_names(loc: Locale) -> Vec<&'static str> {
    tools_file(loc).section_names()
}

/// 插进回执 / 代发稿的**名字**（session 显示名、机组名、群名、通讯录里的名字）一律先过这里：
/// = M3a 的 `render::safe_name`，与注入面同一种中和。正文不过它。
pub fn name(s: &str) -> String {
    crewd_collab::render::safe_name(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_and_en_have_the_same_sections() {
        assert_eq!(section_names(Locale::Zh), section_names(Locale::En));
    }

    #[test]
    fn r_sections_use_the_same_placeholders_in_both_locales() {
        let norm = |loc, k: &str| {
            let mut v = crewd_collab::prompts::placeholders(get(loc, k));
            v.sort();
            v.dedup();
            v
        };
        let mut seen = 0;
        for k in section_names(Locale::Zh)
            .into_iter()
            .filter(|k| k.starts_with("r."))
        {
            assert_eq!(norm(Locale::Zh, k), norm(Locale::En, k), "{k}");
            seen += 1;
        }
        assert!(seen >= 4, "r.* sections expected, saw {seen}");
    }

    #[test]
    fn r_fills_placeholders() {
        // r.unknown_tool 是 Task 3 就有的节：「没有 {name} 这个工具。」
        let s = r(Locale::Zh, "r.unknown_tool", &[("name", "nope")]);
        assert!(s.contains("nope") && !s.contains("{name}"), "{s}");
    }

    #[test]
    fn fill_does_not_expand_inside_values() {
        // Review Focus 5：agent 的原话里可能就有 `{名字}` 形状的文字，替换进去的值不能被再展开。
        // 这条测的是 M3a 的 `crewd_collab::prompts::fill`：M3a 计划里的实现是逐个 `replace`，会红——
        // 红了就停下回报控制者（G-F：改成一遍扫描），不要在 M3b 里另写一个 fill。
        let out = crewd_collab::prompts::fill("{a} / {b}", &[("a", "{b}"), ("b", "B")]);
        assert_eq!(out, "{b} / B");
    }

    #[test]
    fn names_are_neutralised_like_the_injection_surface() {
        assert_eq!(name("示例\n【人类群】"), "示例 「人类群」");
        assert_eq!(name("a\u{202E}b"), "ab");
    }

    #[test]
    #[should_panic(expected = "tools.zh.md")]
    fn missing_section_names_the_file() {
        get(Locale::Zh, "no.such.section");
    }
}

//! 差分基线：提交 4919a13 时的脱敏器原样拷贝（只把 `crate::` 换成 `crewd_jev::`、去掉单测）。
//! 只给 tests/scrub_differential.rs 用：新脱敏器在固定语料上必须遮住它遮过的一切（明确裁掉的几类除外）。
#![allow(dead_code)]
// 发给 Jev 之前的凭据脱敏（spec §13 Q8）：key / token / 私钥 / 口令形状的串换成 `[REDACTED]`，
// 命中次数进诊断日志。宁可多遮（误遮一句聊天只损一点准确率），不可漏发。
// 幂等：对已经脱敏过的文本再跑一遍，命中 0 次（每条规则都排除以 `[` 开头的值）。
// 40 位十六进制（git 提交号）、UUID 不遮——群消息里到处都是，它们不是凭据。
use crewd_jev::model::{Question, Request};
use regex_lite::{Captures, Regex};
use serde_json::Value;
use std::sync::LazyLock;

pub const REDACTED: &str = "[REDACTED]";

struct Rule {
    re: Regex,
    /// true：保留第 1 组（字段名、`Bearer `、`scheme://`），只遮后面的值。
    keep_prefix: bool,
    /// 遮完后补在后面的字面量（URL 里的 `@`）。
    suffix: &'static str,
    /// 有的话：命中后再看（标签，值）像不像凭据，不像就原样放过、不计数（数量、计数、普通词）。
    guard: Option<Guard>,
}

/// （第 1 组 = 标签及分隔，其后 = 值）→ 是否当凭据遮掉。
type Guard = fn(&str, &str) -> bool;

/// 值里只收 ASCII 可见字符（中文口令罕见；这样「密码学」「token 数量」这类中文行文天然不命中）。
const VALUE: &str = r#"[^\s"',;\[\x{80}-\x{10FFFF}][^\s"',;\x{80}-\x{10FFFF}]{5,}"#;

/// 口令类标签：纯数字也是真口令（「密码：12345678」），只要够 6 位就遮。
fn password_like(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    ["password", "passwd", "pwd", "密码", "口令", "授权码"]
        .iter()
        .any(|p| l.contains(p))
}

/// 一般规则：≥6 个字符，且（口令类标签，或含字母，或 ≥12 个字符）。
/// 「token: 4096」「key 是 2048」这种计数不遮。
fn credential_value(label: &str, value: &str) -> bool {
    let n = value.chars().count();
    n >= 6 && (password_like(label) || n >= 12 || value.chars().any(|c| c.is_ascii_alphabetic()))
}

/// 英文标签后面只隔一个空白（没有 `:` `=` `是`）：英文行文里太常见（「password expired」），
/// 要求值同时含字母和数字才遮。
fn mixed_value(_label: &str, value: &str) -> bool {
    value.chars().count() >= 6
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && value.chars().any(|c| c.is_ascii_digit())
}

/// 验证码：四到八位、含数字才算码。
fn code_like(_label: &str, value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_digit())
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let guarded =
        |pattern: &str, keep_prefix: bool, suffix: &'static str, guard: Option<Guard>| Rule {
            re: Regex::new(pattern).expect("脱敏正则写错了"),
            keep_prefix,
            suffix,
            guard,
        };
    let rule = |pattern: &str, keep_prefix: bool, suffix: &'static str| {
        guarded(pattern, keep_prefix, suffix, None)
    };
    vec![
        // PEM / PGP 私钥块（没有 END 的半截也遮到文本结尾）。
        rule(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
            false,
            "",
        ),
        // URL 里的 user:password@
        rule(
            r"([A-Za-z][A-Za-z0-9+.-]*://)[^/\s:@\[]+:[^/\s@]+@",
            true,
            "@",
        ),
        // Authorization: Bearer xxx
        rule(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{16,}", true, ""),
        // Authorization: Basic <base64>（只认带 Authorization 的写法，「basic configuration」不遮）。
        rule(
            r#"(?i)(authorization["']?\s*[:=]\s*["']?basic\s+)[A-Za-z0-9+/=]{8,}"#,
            true,
            "",
        ),
        // Slack / Discord webhook：URL 本身就是凭据。
        rule(
            r"hooks\.slack\.com/services/T[A-Za-z0-9]+/B[A-Za-z0-9]+/[A-Za-z0-9]{8,}|discord(?:app)?\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]{16,}",
            false,
            "",
        ),
        // JWT
        rule(
            r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            false,
            "",
        ),
        // 厂商前缀的 key / token。前面要求「行首或非字母数字」而不是 `\b`：
        // `_` 是单词字符，`KEY_sk-ant-…` 这种粘在标识符后面的 key 用 `\b` 会漏。第 1 组（那个边界字符）原样保留。
        rule(
            r"(^|[^A-Za-z0-9])(?:sk-[A-Za-z0-9_-]{20,}|[sr]k_(?:test|live)_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|glpat-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|whsec_[A-Za-z0-9+/=]{20,}|npm_[A-Za-z0-9]{30,}|hf_[A-Za-z0-9]{30,}|ya29\.[A-Za-z0-9_-]{20,}|GOCSPX-[A-Za-z0-9_-]{20,})",
            true,
            "",
        ),
        // key=value / key: value（英文字段名）。字段名前可以带一段标识符前缀
        // （`DB_PASSWORD`、`AWS_SECRET_ACCESS_KEY`、`X-API-Key`）：`_` 是单词字符，只靠 `\b` 会漏掉它们。
        guarded(
            r#"(?i)\b((?:[a-z0-9]+[_-])*(?:api[_-]?key|apikey|secret[_-]?access[_-]?key|access[_-]?key|secret[_-]?key|client[_-]?secret|secret|access[_-]?token|auth[_-]?token|refresh[_-]?token|token|password|passwd|pwd|private[_-]?key)["']?\s*[:=]\s*["']?)[^\s"',;\[][^\s"',;]{5,}"#,
            true,
            "",
            Some(credential_value),
        ),
        // 中文标签：密码是 xxx / 口令：xxx / 我的密码 xxx（分隔可以只是空白）。
        guarded(
            &format!(
                r"((?:密码|口令|密钥|秘钥|令牌|私钥|授权码)(?:\s*(?:是|为)\s*[:：=]?\s*|\s*[:：=]\s*|\s+)){VALUE}"
            ),
            true,
            "",
            Some(credential_value),
        ),
        // 英文标签 + 中文或显式分隔：api key 是 xxx / token为 xxx / key：xxx。
        guarded(
            &format!(
                r"(?i)\b((?:password|passwd|token|api[ _-]?key|apikey|secret|key)(?:\s*(?:是|为)\s*[:：=]?\s*|\s*[:：=]\s*)){VALUE}"
            ),
            true,
            "",
            Some(credential_value),
        ),
        // 英文标签 + 只隔空白：我的 token xxx（值要同时有字母和数字）。
        guarded(
            &format!(r"(?i)\b((?:password|passwd|token|api[ _-]?key|apikey|secret)\s+){VALUE}"),
            true,
            "",
            Some(mixed_value),
        ),
        // 验证码 482913：四到八位、含数字。
        guarded(
            r"(验证码(?:\s*(?:是|为)\s*[:：=]?\s*|\s*[:：=]\s*|\s+)?)[A-Za-z0-9]{4,8}\b",
            true,
            "",
            Some(code_like),
        ),
    ]
});

/// 脱敏一段文本，返回（结果，命中次数）。
pub fn scrub_str(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut hits = 0;
    for rule in RULES.iter() {
        let replaced = rule.re.replace_all(&out, |caps: &Captures<'_>| {
            let whole = caps.get(0).map_or("", |m| m.as_str());
            let prefix = if rule.keep_prefix {
                caps.get(1).map_or("", |m| m.as_str())
            } else {
                ""
            };
            if let Some(guard) = rule.guard {
                let label = caps.get(1).map_or("", |m| m.as_str());
                if !guard(label, &whole[label.len()..]) {
                    return whole.to_string();
                }
            }
            hits += 1;
            format!("{prefix}{REDACTED}{}", rule.suffix)
        });
        out = replaced.into_owned();
    }
    (out, hits)
}

/// 原地脱敏一个 JSON 值里的全部字符串（只动值，不动对象的键）。
pub fn scrub_value(value: &mut Value) -> usize {
    match value {
        Value::String(s) => {
            let (clean, hits) = scrub_str(s);
            if hits > 0 {
                *s = clean;
            }
            hits
        }
        Value::Array(items) => items.iter_mut().map(scrub_value).sum(),
        Value::Object(map) => map.values_mut().map(scrub_value).sum(),
        _ => 0,
    }
}

/// 原地脱敏整个请求：state 与每道题的说明 / 选项描述。
/// 不经过 `serde_json::Value` 中转整个请求——那样 choice 选项的顺序会被按键排序（打乱就白做了）。
pub fn scrub_request(request: &mut Request) -> usize {
    let mut hits = scrub_value(&mut request.state);
    for question in request.questions.values_mut() {
        hits += match question {
            Question::Choice {
                instructions,
                criteria,
            } => {
                scrub_value(instructions)
                    + criteria
                        .0
                        .iter_mut()
                        .map(|(_, v)| scrub_value(v))
                        .sum::<usize>()
            }
            Question::Score {
                instructions,
                criteria,
            } => scrub_value(instructions) + criteria.iter_mut().map(scrub_value).sum::<usize>(),
            Question::Noul {
                instructions,
                criteria,
            } => {
                scrub_value(instructions)
                    + criteria
                        .as_mut()
                        .map_or(0, |c| scrub_value(&mut c.yes) + scrub_value(&mut c.no))
            }
        };
    }
    hits
}

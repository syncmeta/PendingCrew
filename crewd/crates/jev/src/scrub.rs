//! 发给 Jev 之前的凭据脱敏（spec §13 Q8）：key / token / 私钥 / 口令形状的串换成 `[REDACTED]`，
//! 命中次数进诊断日志。宁可多遮（误遮一句聊天只损一点准确率），不可漏发。
//! 幂等：对已经脱敏过的文本再跑一遍，命中 0 次（规则不会把 `[REDACTED]` 当成值，标签后的值不许以 `[` 开头）。
//! 40 位十六进制（git 提交号）、UUID 不遮——群消息里到处都是，它们不是凭据。
//!
//! 两层：
//! 1. 形状规则（正则）：私钥块、URL 账号密码、Bearer / Basic、webhook、JWT、厂商前缀。
//! 2. 标签扫描（代码）：「标签 + 分隔 + 值」——`DB_PASSWORD=…`、`密码是 …`、`api key 是 "…"`、
//!    `{"密码": "…"}`、`my password is …`、`验证码 482913`。保留标签与分隔，只遮值。
//!    被放过的候选（值不像凭据）不吞文字：从标签末尾继续找，后面真正的值照样能找到。
use crate::model::{Question, Request};
use regex_lite::{Captures, Regex};
use serde_json::Value;
use std::sync::LazyLock;

pub const REDACTED: &str = "[REDACTED]";

struct Rule {
    re: Regex,
    /// true：保留第 1 组（`Bearer `、`scheme://`、边界字符），只遮后面的值。
    keep_prefix: bool,
    /// 遮完后补在后面的字面量（URL 里的 `@`）。
    suffix: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, keep_prefix: bool, suffix: &'static str| Rule {
        re: Regex::new(pattern).expect("脱敏正则写错了"),
        keep_prefix,
        suffix,
    };
    vec![
        // PEM / PGP 私钥块（没有 END 的半截也遮到文本结尾）。
        rule(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
            false,
            "",
        ),
        // URL 里的 user:password@（密码里可以有 `@`：遮到主机前最后一个 `@`）。
        rule(
            r"([A-Za-z][A-Za-z0-9+.-]*://)[^/\s:@\[]+:[^/\s]*@",
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
    ]
});

/// 凭据标签。英文名可带标识符前缀 / 后缀（`DB_PASSWORD`、`SECRET_KEY_BASE`、`X-API-Key`）；
/// 单独的 `key` 不算（`primary key`、`缓存的 key` 太常见），`pass` 只在带前缀时算（`DB_PASS`）。
static LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)",
        r"(?:[a-z0-9]+[_-])*",
        r"(?:password|passwd|pwd|secret[ _-]?access[ _-]?key|access[ _-]?key|secret[ _-]?key",
        r"|private[ _-]?key|client[ _-]?secret|api[ _-]?key|apikey|access[ _-]?token|auth[ _-]?token",
        r"|refresh[ _-]?token|token|secret)",
        r"(?:[_-][a-z0-9]+)*",
        r"|(?:[a-z0-9]+[_-])+pass(?:[_-][a-z0-9]+)*",
        r"|密码|密碼|口令|密钥|密鑰|秘钥|金鑰|令牌|權杖|私钥|私鑰|授权码|授權碼|验证码|驗證碼",
    ))
    .expect("标签正则写错了")
});

/// 值最长扫这么多字符：防「token:token:token:…」这种输入把扫描拖成平方级。
const MAX_VALUE_CHARS: usize = 1024;

fn is_code_label(label: &str) -> bool {
    label == "验证码" || label == "驗證碼"
}

/// 口令类标签：纯数字也是真口令（「密码：12345678」）。
fn password_like(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    l.contains("password")
        || l.contains("passwd")
        || l.contains("pwd")
        || l.ends_with("pass")
        || l.contains("pass_")
        || l.contains("pass-")
        || ["密码", "密碼", "口令", "授权码", "授權碼"]
            .iter()
            .any(|p| l.contains(p))
}

/// 路径、URL 不是凭据值（URL 里的账号密码另有规则）。
fn path_or_url(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with('~')
        || value.starts_with('.')
        || value.contains("://")
}

/// 显式分隔（`:` `=` `是` `is`…）后面的值像不像凭据：
/// 口令类标签 ≥6 个字符即可（纯数字也算）；其他标签要有字母，且（有数字 / 大小写混合 / 有符号 / ≥12 个字符）。
/// 「token: 409600」「这个 token 是 bearer 类型」不遮。
fn credential_value(label: &str, value: &str) -> bool {
    let n = value.chars().count();
    if n < 6 || path_or_url(value) {
        return false;
    }
    if password_like(label) {
        return true;
    }
    let letter = value.chars().any(|c| c.is_ascii_alphabetic());
    let digit = value.chars().any(|c| c.is_ascii_digit());
    let upper = value.chars().any(|c| c.is_ascii_uppercase());
    let lower = value.chars().any(|c| c.is_ascii_lowercase());
    let symbol = value.chars().any(|c| "!@#$%^&*+=~?".contains(c));
    letter && (digit || (upper && lower) || symbol || n >= 12)
}

/// 只隔空白（或标签后直接跟值）：行文里太常见（「password expired」「令牌 refresh」），要求值里既有字母又有数字。
fn mixed_value(value: &str) -> bool {
    value.chars().count() >= 6
        && !path_or_url(value)
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && value.chars().any(|c| c.is_ascii_digit())
}

/// 验证码：四到八位字母数字、含数字，且后面不是量词 / 单位（「2024 年」「1000 条/天」）。
fn code_value(value: &str, rest: &str) -> bool {
    let n = value.chars().count();
    let unit = rest
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| "年月日号条天个次位元人分秒/".contains(c));
    (4..=8).contains(&n)
        && value.chars().all(|c| c.is_ascii_alphanumeric())
        && value.chars().any(|c| c.is_ascii_digit())
        && !unit
}

/// 一个待遮的值：字节区间。
struct Found {
    start: usize,
    end: usize,
}

/// 从标签末尾（`after`）起解析「分隔 + 值」，像凭据就返回值的区间。
fn value_after(text: &str, label: &str, after: usize) -> Option<Found> {
    let rest = &text[after..];
    let mut i = 0;
    let mut explicit = false;
    let mut seen_sep = false;
    // JSON 键的收尾引号：{"密码": "…"}
    if rest.starts_with('"') || rest.starts_with('\'') {
        i += 1;
    }
    loop {
        let r = &rest[i..];
        let Some(c) = r.chars().next() else { break };
        if c.is_whitespace() {
            seen_sep = true;
        } else if ":：=＝是为為".contains(c) {
            seen_sep = true;
            explicit = true;
        } else if (r.starts_with("is") || r.starts_with("IS") || r.starts_with("Is"))
            && i > 0
            && rest[..i].ends_with(char::is_whitespace)
            && !r[2..].starts_with(|c: char| c.is_ascii_alphanumeric())
        {
            seen_sep = true;
            explicit = true;
            i += 2;
            continue;
        } else {
            break;
        }
        i += c.len_utf8();
        if i > 64 {
            return None;
        }
    }
    // 值的开引号（ASCII 与中文引号）。
    let r = &rest[i..];
    if let Some(q) = r.chars().next().filter(|c| "\"'“‘「『`".contains(*c)) {
        i += q.len_utf8();
    }
    let start = after + i;
    let value: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii() && !c.is_ascii_whitespace() && !"\"'`,;".contains(*c))
        .take(MAX_VALUE_CHARS)
        .collect();
    let end = start + value.len();
    // 已经遮过的、或残留的分隔符开头：不是值（幂等的关键）。
    if value.is_empty() || value.starts_with(['[', ':', '=']) {
        return None;
    }
    let ok = if is_code_label(label) {
        code_value(&value, &text[end..])
    } else if !seen_sep {
        // 标签后面直接跟值：只认中文标签（「wifi密码12345678」）。按只隔空白处理；
        // 口令类标签后紧跟 ≥6 位纯数字也算（数字口令）。
        !label.is_ascii()
            && (mixed_value(&value)
                || (password_like(label)
                    && value.len() >= 6
                    && value.chars().all(|c| c.is_ascii_digit())))
    } else if explicit {
        credential_value(label, &value)
    } else {
        mixed_value(&value)
    };
    ok.then_some(Found { start, end })
}

/// 标签扫描：找出所有「标签 + 分隔 + 值」里的值区间。
fn labelled_values(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut pos = 0;
    while let Some(m) = LABEL.find_at(text, pos) {
        let label = m.as_str();
        let ascii = label.is_ascii();
        // 英文标签两头都不能粘着字母数字（`mytoken`、`tokens`、`max_tokens` 都不算）。
        let left_ok = !ascii
            || !text[..m.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let right_ok = !ascii
            || !text[m.end()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if left_ok
            && right_ok
            && let Some(f) = value_after(text, label, m.end())
        {
            pos = f.end;
            found.push(f);
            continue;
        }
        // 放过的候选不吞文字：从标签末尾继续找（`token secret abc123…` 里的 secret 还要看）。
        pos = m.end();
    }
    found
}

/// 一轮最多重复几次：遮掉一处可能让紧挨着的下一处才露出边界（两把 key 首尾相粘）。
const MAX_ROUNDS: usize = 8;

/// 脱敏一段文本，返回（结果，命中次数）。重复整轮直到某一轮 0 命中，所以结果再脱一遍不变。
pub fn scrub_str(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut hits = 0;
    for _ in 0..MAX_ROUNDS {
        let (next, h) = scrub_round(&out);
        if h == 0 {
            break;
        }
        hits += h;
        out = next;
    }
    (out, hits)
}

/// 一轮：先形状规则，再标签扫描。
fn scrub_round(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut hits = 0;
    for rule in RULES.iter() {
        let replaced = rule.re.replace_all(&out, |caps: &Captures<'_>| {
            hits += 1;
            let prefix = if rule.keep_prefix {
                caps.get(1).map_or("", |m| m.as_str())
            } else {
                ""
            };
            format!("{prefix}{REDACTED}{}", rule.suffix)
        });
        out = replaced.into_owned();
    }
    let found = labelled_values(&out);
    if !found.is_empty() {
        hits += found.len();
        let mut next = String::with_capacity(out.len());
        let mut last = 0;
        for f in &found {
            next.push_str(&out[last..f.start]);
            next.push_str(REDACTED);
            last = f.end;
        }
        next.push_str(&out[last..]);
        out = next;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChoiceCriteria;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn assert_scrubbed(input: &str, secret: &str, expected_hits: usize) -> String {
        let (out, hits) = scrub_str(input);
        assert!(!out.contains(secret), "没遮住 {secret:?}：{out}");
        assert!(out.contains(REDACTED), "{out}");
        assert_eq!(hits, expected_hits, "{out}");
        out
    }

    #[test]
    fn vendor_keys_and_tokens() {
        let anthropic = "sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdef";
        assert_scrubbed(&format!("我的 key 是{anthropic}，帮我测一下"), anthropic, 1);
        let openai = "sk-proj-0123456789abcdefghijABCDEFGHIJ";
        let out = assert_scrubbed(&format!("export OPENAI_API_KEY={openai}"), openai, 1);
        assert_eq!(out, "export OPENAI_API_KEY=[REDACTED]");
        let gh = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        assert_scrubbed(&format!("token 在这 {gh}"), &gh, 1);
        assert_scrubbed("AKIAIOSFODNN7EXAMPLE 是 AWS 的", "AKIAIOSFODNN7EXAMPLE", 1);
        let slack = "xoxb-123456789012-abcdefABCDEF";
        assert_scrubbed(slack, slack, 1);
    }

    #[test]
    fn assignments_keep_the_name_and_hide_the_value() {
        let out = assert_scrubbed("password=hunter2hunter", "hunter2hunter", 1);
        assert_eq!(out, "password=[REDACTED]");
        let out = assert_scrubbed(r#"{"api_key": "abcdef123456"}"#, "abcdef123456", 1);
        assert_eq!(out, r#"{"api_key": "[REDACTED]"}"#);
        let out = assert_scrubbed(
            "Authorization: Bearer abcdefghijklmnop1234",
            "abcdefghijklmnop1234",
            1,
        );
        assert_eq!(out, "Authorization: Bearer [REDACTED]");
        let out = assert_scrubbed("密码：Abc12345!", "Abc12345!", 1);
        assert_eq!(out, "密码：[REDACTED]");
        assert_scrubbed("服务器口令是 qwerty789 记一下", "qwerty789", 1);
    }

    #[test]
    fn urls_private_keys_and_jwts() {
        let out = assert_scrubbed(
            "DATABASE_URL=postgres://admin:s3cretpw@db.example.com:5432/app",
            "s3cretpw",
            1,
        );
        assert_eq!(
            out,
            "DATABASE_URL=postgres://[REDACTED]@db.example.com:5432/app"
        );
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let out = assert_scrubbed(
            &format!("贴一下：\n{pem}\n就这个"),
            "b3BlbnNzaC1rZXktdjEAAAAA",
            1,
        );
        assert_eq!(out, "贴一下：\n[REDACTED]\n就这个");
        assert_scrubbed(
            "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg（被截断）",
            "MIIEvQIBADANBg",
            1,
        );
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        assert_scrubbed(&format!("token {jwt}"), "dozjgNryP4J3", 1);
    }

    #[test]
    fn ordinary_chat_is_left_alone() {
        for text in [
            "commit 46866a0c9b1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a 修好了",
            "max_tokens=4096, tokens: 1200",
            "密码是多少？",
            "把 token 的过期时间改成两小时",
            "sk-learn 很好用",
            "https://github.com/foo/bar/pull/12",
            "session 3f9c2a7e-1b4d-4e8a-9c3f-2a7e1b4d4e8a 卡住了",
            "password reset 的邮件模板改一下",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    /// 环境变量 / 配置文件写法：字段名前面带一段标识符（`DB_`、`AWS_`、`MY_SERVICE_`），
    /// `_` 是单词字符，字段名前不再有 `\b`，必须照样遮住。
    #[test]
    fn env_style_assignments_keep_the_name_and_hide_the_value() {
        let aws = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        for (input, secret, expected) in [
            (
                "DB_PASSWORD=hunter2hunter2",
                "hunter2hunter2",
                "DB_PASSWORD=[REDACTED]".to_string(),
            ),
            (
                &*format!("export AWS_SECRET_ACCESS_KEY={aws}"),
                aws,
                "export AWS_SECRET_ACCESS_KEY=[REDACTED]".to_string(),
            ),
            (
                &*format!("aws_secret_access_key = {aws}"),
                aws,
                "aws_secret_access_key = [REDACTED]".to_string(),
            ),
            (
                "JWT_SECRET=supersecretvalue123",
                "supersecretvalue123",
                "JWT_SECRET=[REDACTED]".to_string(),
            ),
            (
                "MY_SERVICE_API_KEY=abcdef1234567890",
                "abcdef1234567890",
                "MY_SERVICE_API_KEY=[REDACTED]".to_string(),
            ),
            (
                "GITHUB_TOKEN=abcdefabcdef123456",
                "abcdefabcdef123456",
                "GITHUB_TOKEN=[REDACTED]".to_string(),
            ),
            (
                "curl -H 'X-API-Key: abcdef0123456789abcdef'",
                "abcdef0123456789abcdef",
                "curl -H 'X-API-Key: [REDACTED]'".to_string(),
            ),
        ] {
            let out = assert_scrubbed(input, secret, 1);
            assert_eq!(out, expected);
            assert_eq!(scrub_str(&out), (out.clone(), 0), "第二遍不该再命中");
        }
    }

    #[test]
    fn more_vendor_shapes_webhooks_pgp_and_basic_auth() {
        let cases: [(&str, &str); 14] = [
            (
                "STRIPE_SECRET_KEY=sk_test_51Habcdefghijklmnop",
                "51Habcdefghijklmnop",
            ),
            ("sk_test_51HabcdefghijklmnopqrstuV", "51Habcdefghijklmnop"),
            ("sk_live_51HabcdefghijklmnopqrstuV", "51Habcdefghijklmnop"),
            ("rk_test_51HabcdefghijklmnopqrstuV", "51Habcdefghijklmnop"),
            (
                "whsec_abcdefghijklmnopqrstuvwxyz012345",
                "abcdefghijklmnopqrstuvwxyz",
            ),
            (
                "npm_abcdefghijklmnopqrstuvwxyz0123456789",
                "abcdefghijklmnopqrstuvwxyz",
            ),
            (
                "hf_abcdefghijklmnopqrstuvwxyzABCDEFGH",
                "abcdefghijklmnopqrstuvwxyz",
            ),
            (
                "ya29.a0AfH6SMBabcdefghijklmnopqrstuvwxyz",
                "a0AfH6SMBabcdefghij",
            ),
            (
                "GOCSPX-abcdefghijklmnopqrstuvwxyz12",
                "abcdefghijklmnopqrstuvwxyz",
            ),
            (
                "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX",
                "XXXXXXXXXXXXXXXXXXXXXXXX",
            ),
            (
                "https://discord.com/api/webhooks/123456789012345678/abcdefghijklmnopqrstuvwxyzABCDEFGHIJ",
                "abcdefghijklmnopqrstuvwxyzABCDEFGHIJ",
            ),
            (
                "-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBFabcdefgh\n-----END PGP PRIVATE KEY BLOCK-----",
                "lQOYBFabcdefgh",
            ),
            (
                "Authorization: Basic dXNlcjpwYXNzd29yZDEyMw==",
                "dXNlcjpwYXNzd29yZDEyMw",
            ),
            (
                "KEY_sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdef",
                "AbCdEf0123456789xyzXYZ",
            ),
        ];
        for (input, secret) in cases {
            let out = assert_scrubbed(input, secret, 1);
            assert_eq!(scrub_str(&out), (out.clone(), 0), "第二遍不该再命中：{out}");
        }
        let out = assert_scrubbed(
            "-H 'Authorization: Basic dXNlcjpwYXNzd29yZDEyMw=='",
            "dXNlcjpwYXNzd29yZDEyMw",
            1,
        );
        assert_eq!(out, "-H 'Authorization: Basic [REDACTED]'");
        let out = assert_scrubbed(
            "KEY_sk-ant-api03-AbCdEf0123456789xyzXYZ_-abcdef",
            "AbCdEf0123456789xyzXYZ",
            1,
        );
        assert_eq!(out, "KEY_[REDACTED]");
    }

    #[test]
    fn vendor_lookalikes_in_chat_stay_untouched() {
        for text in [
            "basic configuration 先别动",
            "hooks.slack.com 的文档看一下",
            "hf_hub 下载模型太慢",
            "ya29 是什么前缀",
            "npm_config_registry 换成镜像",
            "the secret sauce is caching",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    /// 中文标签（或英文标签 + 中文分隔）的凭据：保留标签，遮值。
    #[test]
    fn chinese_labelled_credentials_keep_the_label_and_hide_the_value() {
        for (input, expected) in [
            ("我的密码 hunter2hunter", "我的密码 [REDACTED]"),
            ("密码：Abc123456", "密码：[REDACTED]"),
            ("数据库密码是 p@ssw0rd99", "数据库密码是 [REDACTED]"),
            ("api key 是 sk-xxxxxxxxxx", "api key 是 [REDACTED]"),
            ("token为 abcdefabcdef12", "token为 [REDACTED]"),
            ("秘钥是：AbCd1234efgh 别外传", "秘钥是：[REDACTED] 别外传"),
            ("授权码=QwErTy7788，明天过期", "授权码=[REDACTED]，明天过期"),
            (
                "验证码 482913，五分钟内有效",
                "验证码 [REDACTED]，五分钟内有效",
            ),
            (
                "我的 token ghx9Kd02LmQ 你先用",
                "我的 token [REDACTED] 你先用",
            ),
            // 口令类标签：纯数字也是真口令。
            ("WiFi 密码：88886666", "WiFi 密码：[REDACTED]"),
            ("password=12345678", "password=[REDACTED]"),
        ] {
            let (out, hits) = scrub_str(input);
            assert_eq!((out.as_str(), hits), (expected, 1), "{input}");
            assert_eq!(scrub_str(&out), (out.clone(), 0), "第二遍不该再命中：{out}");
        }
        // 私钥标签后面跟 PEM 块：PEM 规则先遮整块，中文规则不再二次命中。
        let (out, hits) = scrub_str(
            "私钥: -----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg\n-----END PRIVATE KEY-----",
        );
        assert_eq!((out.as_str(), hits), ("私钥: [REDACTED]", 1));
    }

    /// 真实发送路径会脱两遍（门面一遍、客户端一遍）：第二遍必须 0 命中、结果不变。
    #[test]
    fn realistic_double_scrub_is_a_no_op() {
        for input in [
            "密码为:Abc12345",
            "密码 :Abc12345",
            "token为:abcdef123456",
            "api key 是:abcdef123456",
            "密码是=hunter2hunter",
            "口令\n:hunter2hunter",
            "密码为：Abc12345",
            "wifi 密码是:88886666",
            "密码＝＝Abc12345",
        ] {
            let (once, hits) = scrub_str(input);
            assert_eq!(hits, 1, "{input} -> {once}");
            assert!(once.ends_with(REDACTED), "{input} -> {once}");
            assert_eq!(scrub_str(&once), (once.clone(), 0), "{input}");
        }
    }

    /// 两把厂商 key 首尾相粘：第一把遮掉后第二把前面才出现边界。必须一遍里都遮掉，第二遍 0 命中。
    #[test]
    fn glued_vendor_keys_are_all_redacted_in_one_pass() {
        for input in [
            "AKIA0123456789ABCDEFsk_test_abcdefghijklmnopqrstuv",
            "授权码AKIA0123456789ABCDEFAKIA0123456789ABCDEF",
            "AKIA0123456789ABCDEFAKIA0123456789ABCDEFAKIA0123456789ABCDEF",
        ] {
            let (once, hits) = scrub_str(input);
            assert!(
                !once.contains("AKIA0") && !once.contains("sk_test_"),
                "{once}"
            );
            assert!(hits >= 2, "{input} -> {once} ({hits})");
            assert_eq!(scrub_str(&once), (once.clone(), 0), "{input}");
        }
    }

    /// 性质测试：标签 / 分隔 / 值的碎片随机拼出来的串，脱两遍与脱一遍相同，第二遍 0 命中。
    #[test]
    fn fuzzed_inputs_scrub_idempotently() {
        let frags = [
            "password",
            "密码",
            "密碼",
            "口令",
            "token",
            "key",
            "api key",
            "secret",
            "验证码",
            "授权码",
            "DB_",
            "_KEY",
            "is",
            " is ",
            " ",
            "  ",
            "\u{3000}",
            ":",
            "：",
            "=",
            "＝",
            "是",
            "为",
            "為",
            "\"",
            "'",
            "“",
            "「",
            "[",
            "]",
            "REDACTED",
            "[REDACTED]",
            "hunter2",
            "abc123",
            "123456",
            "88886666",
            "2024",
            "年",
            "条",
            "sk-",
            "sk_test_",
            "abcdefghijklmnopqrstuv",
            "Bearer ",
            "https://",
            "u:p@",
            "@",
            "/",
            "-----BEGIN PRIVATE KEY-----",
            "eyJ",
            ".",
            "_",
            "-",
            "，",
            "\n",
            "x",
            "9",
            "ghp_",
            "AKIA",
            "0123456789ABCDEF",
            "AKIA0123456789ABCDEF",
        ];
        let mut seed: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            seed >> 33
        };
        let mut bad = Vec::new();
        for _ in 0..60_000 {
            let n = 2 + (next() % 12) as usize;
            let s: String = (0..n)
                .map(|_| frags[next() as usize % frags.len()])
                .collect();
            let (once, _) = scrub_str(&s);
            let (twice, hits) = scrub_str(&once);
            if twice != once || hits != 0 {
                bad.push((s, once, twice, hits));
            }
        }
        assert!(
            bad.is_empty(),
            "{} 条不幂等，例如 {:?}",
            bad.len(),
            &bad[..bad.len().min(5)]
        );
    }

    /// 中文标签只隔空白时，与英文同样要求值里既有字母又有数字。
    #[test]
    fn chinese_label_plus_space_needs_a_mixed_value() {
        for text in [
            "令牌 refresh 逻辑",
            "密钥 rotation 怎么做",
            "私钥 format 不对",
            "口令 policy 要改",
            "密码 expired 了",
            "密码 hashing 用 bcrypt",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    /// 单独的 `key` 不是凭据标签；值也要像凭据（光一个 bearer 不是）。
    #[test]
    fn bare_key_and_plain_words_are_not_credentials() {
        for text in [
            "缓存的 key 是 session_id",
            "primary key: user_id",
            "sort key: created_at",
            "hot-key: ctrl+shift",
            "key: release-notes",
            "i18n 的 key 是 crew.title",
            "这个 token 是 bearer 类型",
            "secret: true",
            "token: 1234567890123",
            "pwd: /Users/someone/project",
            "the key is to write tests first",
            "token limit is 200000",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    /// 被放过的候选不能吞掉后面的文字：真正的值还得被找到。
    #[test]
    fn a_rejected_candidate_does_not_hide_the_real_value() {
        for (input, expected) in [
            (
                "the secret password hunter2hunter",
                "the secret password [REDACTED]",
            ),
            ("token secret ghx9Kd02LmQ", "token secret [REDACTED]"),
            ("api key secret abc123def456", "api key secret [REDACTED]"),
            ("密码 secret Abc12345", "密码 secret [REDACTED]"),
        ] {
            assert_eq!(scrub_str(input), (expected.to_string(), 1), "{input}");
        }
    }

    #[test]
    fn quoted_fullwidth_json_traditional_and_suffixed_forms() {
        for (input, expected) in [
            ("密码是\"hunter2hunter\"", "密码是\"[REDACTED]\""),
            ("密码：'hunter2hunter'", "密码：'[REDACTED]'"),
            ("api key 是 \"abcdef123456\"", "api key 是 \"[REDACTED]\""),
            ("密码：“hunter2hunter”", "密码：“[REDACTED]”"),
            ("密码是「hunter2hunter」", "密码是「[REDACTED]」"),
            (r#"{"密码": "Abc12345"}"#, r#"{"密码": "[REDACTED]"}"#),
            ("密码\u{3000}Abc12345", "密码\u{3000}[REDACTED]"),
            ("密码＝Abc12345", "密码＝[REDACTED]"),
            ("PASSWORD＝Abc12345", "PASSWORD＝[REDACTED]"),
            ("my password is hunter2hunter", "my password is [REDACTED]"),
            (
                "the password is: hunter2hunter",
                "the password is: [REDACTED]",
            ),
            (
                "SECRET_KEY_BASE=abcdef0123456789abcdef",
                "SECRET_KEY_BASE=[REDACTED]",
            ),
            ("API_KEY_ID=abcdef0123456789", "API_KEY_ID=[REDACTED]"),
            ("DB_PASS=hunter2hunter", "DB_PASS=[REDACTED]"),
            ("密碼是 Abc12345", "密碼是 [REDACTED]"),
            ("金鑰：Abc12345xyz", "金鑰：[REDACTED]"),
            ("權杖為 abc123def456", "權杖為 [REDACTED]"),
            (
                "https://user:p@ss@host.example/x",
                "https://[REDACTED]@host.example/x",
            ),
            ("验证码：482913", "验证码：[REDACTED]"),
            ("验证码482913", "验证码[REDACTED]"),
            ("wifi密码12345678", "wifi密码[REDACTED]"),
        ] {
            let (out, hits) = scrub_str(input);
            assert_eq!((out.as_str(), hits), (expected, 1), "{input}");
            assert_eq!(scrub_str(&out), (out.clone(), 0), "{input}");
        }
    }

    #[test]
    fn verification_code_needs_a_code_not_a_year_or_a_count() {
        for text in [
            "验证码 2024 年上线",
            "验证码 1000 条/天",
            "验证码 3 个",
            "验证码 1200 次",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    #[test]
    fn chinese_prose_and_counts_stay_untouched() {
        for text in [
            "密码学是一门学科",
            "请输入密码",
            "token 数量是 4096",
            "token 数是 4096",
            "token: 409600",
            "密码是多少？",
            "验证码是多少",
            "验证码发了吗",
            "key insight 是先写测试",
            "password expired 了",
            "令牌桶限流算法",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    #[test]
    fn identifier_prefixes_do_not_make_ordinary_words_secrets() {
        for text in [
            "max_tokens=4096, tokens: 1200",
            "MAX_TOKENS=4096",
            "reset_password 页面的文案改一下",
            "secret_santa 名单发群里了",
            "token_count: 12",
        ] {
            let (out, hits) = scrub_str(text);
            assert_eq!((out.as_str(), hits), (text, 0), "误遮了：{text}");
        }
    }

    #[test]
    fn scrubbing_twice_finds_nothing_new() {
        let input = "password=hunter2hunter sk-ant-api03-AbCdEf0123456789xyzXYZ postgres://a:b@h 密码：Abc12345! Bearer abcdefghijklmnop1234";
        let (once, first) = scrub_str(input);
        assert_eq!(first, 5);
        let (twice, second) = scrub_str(&once);
        assert_eq!((twice, second), (once, 0));
    }

    #[test]
    fn request_is_scrubbed_in_place_without_reordering_options() {
        let secret = "sk-ant-api03-AbCdEf0123456789xyzXYZ";
        let mut questions = BTreeMap::new();
        questions.insert(
            "route".to_string(),
            Question::Choice {
                instructions: json!("谁来做"),
                criteria: ChoiceCriteria(vec![
                    ("zeta".into(), json!({"work": [format!("轮换 {secret}")]})),
                    ("alpha".into(), Value::Null),
                ]),
            },
        );
        let mut req = Request {
            model: "jev-latest".into(),
            state: json!({"todo": format!("key 是 {secret}"), "n": 3}),
            questions,
        };
        assert_eq!(scrub_request(&mut req), 2);
        let wire = serde_json::to_string(&req).unwrap();
        assert!(!wire.contains(secret));
        assert!(wire.find("\"zeta\"").unwrap() < wire.find("\"alpha\"").unwrap());
    }
}

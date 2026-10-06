//! 发给 Jev 之前的凭据脱敏（spec §13 Q8）：key / token / 私钥 / 口令形状的串换成 `[REDACTED]`，
//! 命中次数进诊断日志。宁可多遮（误遮一句聊天只损一点准确率），不可漏发。
//! 幂等：对已经脱敏过的文本再跑一遍，命中 0 次（每条规则都排除以 `[` 开头的值）。
//! 40 位十六进制（git 提交号）、UUID 不遮——群消息里到处都是，它们不是凭据。
use crate::model::{Question, Request};
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
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, keep_prefix: bool, suffix: &'static str| Rule {
        re: Regex::new(pattern).expect("脱敏正则写错了"),
        keep_prefix,
        suffix,
    };
    vec![
        // PEM 私钥块（没有 END 的半截也遮到文本结尾）。
        rule(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
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
        // JWT
        rule(
            r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
            false,
            "",
        ),
        // 厂商前缀的 key / token。
        rule(
            r"\b(?:sk-[A-Za-z0-9_-]{20,}|sk_live_[A-Za-z0-9]{16,}|rk_live_[A-Za-z0-9]{16,}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|glpat-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35})",
            false,
            "",
        ),
        // key=value / key: value（英文字段名）。字段名前可以带一段标识符前缀
        // （`DB_PASSWORD`、`AWS_SECRET_ACCESS_KEY`、`X-API-Key`）：`_` 是单词字符，只靠 `\b` 会漏掉它们。
        rule(
            r#"(?i)\b((?:[a-z0-9]+[_-])*(?:api[_-]?key|apikey|secret[_-]?access[_-]?key|access[_-]?key|secret[_-]?key|client[_-]?secret|secret|access[_-]?token|auth[_-]?token|refresh[_-]?token|token|password|passwd|pwd|private[_-]?key)["']?\s*[:=]\s*["']?)[^\s"',;\[][^\s"',;]{5,}"#,
            true,
            "",
        ),
        // 中文：密码是 xxx / 口令：xxx
        rule(
            r"((?:密码|口令|密钥|私钥|令牌)\s*(?:是|为|[:：=])\s*)[^\s，。,;；\[]{4,}",
            true,
            "",
        ),
    ]
});

/// 脱敏一段文本，返回（结果，命中次数）。
pub fn scrub_str(text: &str) -> (String, usize) {
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

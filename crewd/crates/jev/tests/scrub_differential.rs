//! 差分：新脱敏器（标签扫描）对比 4919a13 的正则脱敏器（tests/scrub_baseline）。
//! 固定语料（标签 × 分隔 × 值 × 结尾，种子固定）上，旧的遮住的值新的也必须遮住，
//! 除非落在复审明确裁掉的几类里（见 `ruled_out`）——那几类是有意不遮的（误遮代价 > 漏遮代价）。
mod scrub_baseline;

use crewd_jev::scrub::scrub_str;

const LABELS: &[&str] = &[
    "password",
    "Password",
    "PASSWORD",
    "passwd",
    "pwd",
    "token",
    "Token",
    "api key",
    "api_key",
    "API-KEY",
    "apikey",
    "secret",
    "client_secret",
    "access_token",
    "auth_token",
    "refresh_token",
    "private_key",
    "DB_PASSWORD",
    "AWS_SECRET_ACCESS_KEY",
    "X-API-Key",
    "JWT_SECRET",
    "key",
    "my password",
    "the token",
    "密码",
    "口令",
    "密钥",
    "秘钥",
    "令牌",
    "私钥",
    "授权码",
    "验证码",
    "我的密码",
    "数据库密码",
    "wifi密码",
    "WiFi 密码",
    "\"password\"",
    "'token'",
    "{\"api_key\"",
];
const SEPS: &[&str] = &[
    "=", ": ", ":", " = ", "：", " ", "  ", "是", "是 ", " 是 ", "为", "为 ", "是：", "\t", "\n",
    " :", "=\"", ": \"", ": '", "='", "\":\"", "\": \"", "是\"", "：“", "", " is ", "＝",
    "\u{3000}",
];
const SECRETS: &[&str] = &[
    "hunter2hunter",
    "Abc12345",
    "88886666",
    "12345678",
    "482913",
    "QwErTy7788",
    "p@ssw0rd99",
    "abcdefabcdef12",
    "s3cr3t-Value_9",
    "AbCdEfGhIjKl",
    "abcdefghijklmnop",
    "Xy9!Xy9!",
    "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    "Straße2024x",
    "hunter2ü99zz",
    "pässwörd123",
    "abc.def.ghi1",
    "a1b2c3d4",
    "9f8e7d6c5b4a",
    "AB12CD34",
    "zzzz1234)",
    "482913.",
    "hunter2hunter!",
    "hunter2hunter.",
];
const TAILS: &[&str] = &[
    "",
    " ",
    "，别外传",
    ", ok",
    "\n",
    ";",
    "。",
    " 记一下",
    "\"",
    "'",
    "”",
    "」",
    " 年",
    ")",
    ".",
];

fn is_space_sep(sep: &str) -> bool {
    sep.chars().all(char::is_whitespace)
}

/// 复审裁定里有意不遮的几类（旧脱敏器遮了，新的不遮，属于预期）。
fn ruled_out(label: &str, sep: &str, secret: &str, tail: &str) -> Option<&'static str> {
    let core = secret.trim_end_matches(['.', ')', '!']);
    let has_digit = core.chars().any(|c| c.is_ascii_digit());
    let has_letter = core.chars().any(char::is_alphabetic);
    let password_like = {
        let l = label.to_lowercase();
        ["password", "passwd", "pwd", "密码", "口令", "授权码"]
            .iter()
            .any(|p| l.contains(p))
    };
    if label == "key" {
        return Some("单独的 key 不是凭据标签");
    }
    if label.contains("验证码") {
        let code = core.len() <= 8 && core.chars().all(|c| c.is_ascii_alphanumeric()) && has_digit;
        if !code {
            return Some("验证码后面不是 4–8 位含数字的码");
        }
        if tail
            .trim_start()
            .starts_with(['年', '条', '天', '个', '次'])
        {
            return Some("验证码后面跟着量词 / 单位");
        }
    }
    if !password_like && core.chars().all(|c| c.is_ascii_digit()) {
        // 4919a13 的值能吞进后面的中文（「88886666，别外传」凑够 12 个字符）才遮了它；
        // 复审裁定：非口令类标签后的纯数字是计数，不遮。
        return Some("非口令类标签后的纯数字");
    }
    if is_space_sep(sep) || sep.contains("is") {
        // 只隔空白 / `is`：要字母 + 数字，或口令类标签后的纯数字。
        let numeric_password = password_like && core.chars().all(|c| c.is_ascii_digit());
        if !(numeric_password || (has_letter && has_digit)) {
            return Some("只隔空白或 is：值缺字母或数字");
        }
    }
    None
}

#[test]
fn the_label_scanner_hides_everything_the_regex_scrubber_hid() {
    let mut seed: u64 = 0xdead_beef_cafe_f00d;
    let mut next = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    };
    let mut regressions = Vec::new();
    let mut compared = 0;
    for _ in 0..30_000 {
        let label = LABELS[next() % LABELS.len()];
        let sep = SEPS[next() % SEPS.len()];
        let secret = SECRETS[next() % SECRETS.len()];
        let tail = TAILS[next() % TAILS.len()];
        let input = format!("{label}{sep}{secret}{tail}");
        let core = secret.trim_end_matches(['.', ')', '!']);
        let (old, _) = scrub_baseline::scrub_str(&input);
        if old.contains(core) {
            continue; // 旧的也没遮，不比
        }
        compared += 1;
        let (new, _) = scrub_str(&input);
        if new.contains(core) && ruled_out(label, sep, secret, tail).is_none() {
            regressions.push((input, old, new));
        }
    }
    assert!(compared > 5_000, "可比的样本太少：{compared}");
    assert!(
        regressions.is_empty(),
        "{} 条旧的遮了、新的漏了，例如：{:#?}",
        regressions.len(),
        &regressions[..regressions.len().min(8)]
    );
}

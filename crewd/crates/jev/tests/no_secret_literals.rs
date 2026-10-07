//! 守卫：仓库里不许出现「长得像真密钥」的字面量。
//!
//! 脱敏（scrub）的测试夹具是合成值，但 GitHub push protection 只看形状、不看真假，
//! 每次推送都会被拦下、要人工标成误报。夹具改用 `concat!("sk_t", "est_…")` 拼出来，
//! 源码里永远没有完整形状；运行时拼出的字节与原字面量逐字节相同。
//!
//! 本测试扫 `crewd/` 与 `docs/` 下会被推送的文本文件（git 跟踪的 + 未忽略的新文件），出现完整形状就红。
//! 规则本身是正则源码（`sk_(?:live|test)_[A-Za-z0-9]{16,}`），不会命中自己，所以不需要排除名单；
//! `patterns_do_catch_the_shapes` 用运行时拼出的样本证明尺子真的会红。

use regex_lite::Regex;
use std::path::{Path, PathBuf};

/// 与 scrub.rs 里厂商前缀规则的长度门槛一致。
const SHAPES: &[(&str, &str)] = &[
    ("stripe", r"(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}"),
    ("anthropic/openai", r"sk-(?:ant|proj)-[A-Za-z0-9_-]{16,}"),
    ("github token", r"gh[pousr]_[A-Za-z0-9]{30,}"),
    ("github pat", r"github_pat_[A-Za-z0-9_]{30,}"),
    ("gitlab pat", r"glpat-[A-Za-z0-9_-]{20,}"),
    ("slack token", r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    ("aws access key", r"AKIA[0-9A-Z]{16}"),
    ("google api key", r"AIza[0-9A-Za-z_-]{35}"),
    ("hugging face", r"hf_[A-Za-z0-9]{30,}"),
    (
        "slack webhook",
        r"hooks\.slack\.com/services/[A-Za-z0-9]+/[A-Za-z0-9]+/[A-Za-z0-9]+",
    ),
    (
        "discord webhook",
        r"discord\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]{20,}",
    ),
    ("stripe webhook secret", r"whsec_[A-Za-z0-9+/=]{20,}"),
    ("npm token", r"npm_[A-Za-z0-9]{30,}"),
    ("google oauth", r"ya29\.[A-Za-z0-9_-]{20,}"),
    ("google client secret", r"GOCSPX-[A-Za-z0-9_-]{20,}"),
    (
        "private key block",
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----(?:\\n|\n)[A-Za-z0-9+/]{8,}",
    ),
];

fn compiled() -> Vec<(&'static str, Regex)> {
    SHAPES
        .iter()
        .map(|(name, pat)| (*name, Regex::new(pat).unwrap()))
        .collect()
}

fn repo_root() -> PathBuf {
    // crewd/crates/jev -> 仓库根
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                walk(&path, out);
            }
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

fn scan(files: &[PathBuf], root: &Path) -> Vec<String> {
    let shapes = compiled();
    let mut hits = Vec::new();
    for file in files {
        // 非 UTF-8（图片、二进制）直接跳过。
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for (name, re) in &shapes {
                if re.is_match(line) {
                    let rel = file.strip_prefix(root).unwrap_or(file);
                    hits.push(format!("{}:{}  [{name}]", rel.display(), i + 1));
                }
            }
        }
    }
    hits
}

/// 只扫会被推上去的文件：git 跟踪的 + 未跟踪但没被忽略的（下一次 commit 会带上）。
/// 被 .gitignore 忽略的（docs/private、target 等本机私有目录）永远不会进推送，不扫——
/// 那里存着旧评审包，里面原样留有历史上的夹具。没有 git 时退回整树遍历。
fn pushable_files(root: &Path) -> Vec<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "crewd",
            "docs",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => o
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| root.join(String::from_utf8_lossy(p).as_ref()))
            .filter(|p| p.is_file())
            .collect(),
        _ => {
            let mut files = Vec::new();
            walk(&root.join("crewd"), &mut files);
            walk(&root.join("docs"), &mut files);
            files
        }
    }
}

#[test]
fn no_secret_shaped_literals_in_crewd_or_docs() {
    let root = repo_root();
    let files = pushable_files(&root);
    assert!(
        files.len() > 50,
        "扫到的文件太少（{}），目录判断错了？",
        files.len()
    );
    let hits = scan(&files, &root);
    assert!(
        hits.is_empty(),
        "出现了完整的密钥形状字面量（push protection 会拦）。\
         夹具请拆开拼：concat!(\"sk_t\", \"est_…\")\n{}",
        hits.join("\n")
    );
}

/// 尺子自检：运行时拼出每种形状的样本，必须全部命中；否则守卫的绿没有意义。
#[test]
fn patterns_do_catch_the_shapes() {
    let a36 = "a1B2c3D4e5".repeat(4);
    let samples = [
        format!("{}{}", "sk_", "live_51HabcdefghijklmnopqrstuV"),
        format!("{}{}", "rk_", "test_51HabcdefghijklmnopqrstuV"),
        format!("{}{}", "sk-", "ant-api03-AbCdEf0123456789xyzXYZ"),
        format!("{}{}", "sk-", "proj-0123456789abcdefghijABCDEFGHIJ"),
        format!("{}{}", "ghp_", a36),
        format!("{}{}", "github_pat_", a36),
        format!("{}{}", "glpat", "-abcdefghijklmnopqrstuv"),
        format!("{}{}", "xox", "b-123456789012-abcdefABCDEF"),
        format!("{}{}", "AKIA", "0123456789ABCDEF"),
        format!("{}{}", "AIza", "SyA1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q"),
        format!("{}{}", "hf_", "abcdefghijklmnopqrstuvwxyzABCDEFGH"),
        ["https://hooks.slack", ".com/services/T000/B000/XXXXXXXX"].concat(),
        [
            "https://discord",
            ".com/api/webhooks/1234/abcdefghijklmnopqrstuvwx",
        ]
        .concat(),
        format!("{}{}", "whsec", "_abcdefghijklmnopqrstuvwxyz012345"),
        format!("{}{}", "npm", "_abcdefghijklmnopqrstuvwxyz0123456789"),
        format!("{}{}", "ya29", ".a0AfH6SMBabcdefghijklmnopqrstuvwxyz"),
        format!("{}{}", "GOCSPX", "-abcdefghijklmnopqrstuvwxyz12"),
        ["-----BEGIN ", "PRIVATE KEY-----\\nMIIEvQIBADANBg"].concat(),
        ["-----BEGIN OPENSSH ", "PRIVATE KEY-----\nb3BlbnNzaC1r"].concat(),
    ];
    let shapes = compiled();
    for s in &samples {
        assert!(
            shapes.iter().any(|(_, re)| re.is_match(s)),
            "守卫没认出这个形状：{s}"
        );
    }
    // 每条规则至少被一个样本触发过（防止某条规则写坏了却没人发现）。
    for (name, re) in &shapes {
        assert!(
            samples.iter().any(|s| re.is_match(s)),
            "规则 {name} 没有任何样本能触发"
        );
    }
    // 拆开的拼接写法本身不命中。
    assert!(
        !shapes
            .iter()
            .any(|(_, re)| re.is_match(r#"concat!("sk_t", "est_51HabcdefghijklmnopqrstuV")"#))
    );
}
